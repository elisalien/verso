//! OSC, MIDI, audio and tempo in the app: polling inputs, MIDI learn, the
//! Control window and modulation.

use std::time::Instant;

use eframe::egui::{self, Rect, Stroke, StrokeKind, Vec2};
use verso_control::{AudioInput, Levels, MidiInputs, OscServer, TapTempo};
use verso_core::control::{Action, MidiKey, MidiMapping, Param, Range};
use verso_core::modulation::{AudioBand, ModSource, Modulator, Signals, Waveform};
use verso_core::{Command, Composition};
use verso_render::ParamKind;

use crate::app::VersoApp;
use crate::{theme, widgets};

pub const DEFAULT_OSC_PORT: u16 = 7000;

/// Live control inputs and their settings. Not saved with the composition.
pub struct Controls {
    pub osc: Option<OscServer>,
    pub osc_port: u16,
    pub midi: Option<MidiInputs>,
    pub audio: Option<AudioInput>,
    pub audio_devices: Vec<String>,
    pub audio_choice: Option<String>,
    pub levels: Levels,
    /// Follow the tempo detected in the audio input.
    pub auto_bpm: bool,
    pub tap: TapTempo,
    /// Musical time, advanced at the composition tempo.
    pub beats: f64,
    /// While on, clicking a control picks it and the next MIDI message maps it.
    pub learn: bool,
    pub learn_target: Option<Action>,
    pub last_message: String,
    pub window_open: bool,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            osc: None,
            osc_port: DEFAULT_OSC_PORT,
            midi: None,
            audio: None,
            audio_devices: Vec::new(),
            audio_choice: None,
            levels: Levels::default(),
            auto_bpm: false,
            tap: TapTempo::default(),
            beats: 0.0,
            learn: false,
            learn_target: None,
            last_message: String::new(),
            window_open: false,
        }
    }
}

impl VersoApp {
    /// Range of `param` and its current value, effect parameters included.
    pub(crate) fn param_range(&self, comp: &Composition, param: &Param) -> Option<(Range, f32)> {
        if let Some(range) = param.builtin_range() {
            return Some((range, param.get(comp)?));
        }
        let Param::Effect {
            target,
            index,
            name,
        } = param
        else {
            return None;
        };
        let effect = comp.effects(*target)?.get(*index)?;
        let def = self.renderer.library().get(&effect.id)?;
        let param_def = def.param(name)?;
        let range = match param_def.kind {
            ParamKind::Float { min, max, .. } => Range {
                min,
                max,
                toggle: false,
            },
            ParamKind::Bool { .. } => Range::TOGGLE,
            ParamKind::Color { .. } | ParamKind::Point { .. } => return None,
        };
        let value = match def.value(effect, param_def) {
            verso_core::ParamValue::Float(v) => v,
            verso_core::ParamValue::Bool(b) => f32::from(u8::from(b)),
            _ => return None,
        };
        Some((range, value))
    }

    /// Runs `action` with a control value in 0..1.
    fn run_action(&mut self, action: &Action, unit: f32) {
        let range = match action {
            Action::Set(param) => self.param_range(&self.comp, param).map(|(r, _)| r),
            _ => None,
        };
        if let Some(command) = action.command(unit, range) {
            self.send(command);
        }
    }

    /// Handles OSC and MIDI messages and advances the beat clock.
    pub(crate) fn poll_controls(&mut self, dt: f64) {
        let osc_events = self
            .controls
            .osc
            .as_ref()
            .map(|o| o.poll())
            .unwrap_or_default();
        for event in osc_events {
            self.controls.last_message = format!(
                "OSC {} {}",
                event.address,
                event.value.map(|v| format!("{v:.3}")).unwrap_or_default()
            );
            match Action::parse_osc(&event.address, event.value) {
                Some((action, value)) => self.run_action(&action, value),
                None => self.status = format!("Unknown OSC address {}", event.address),
            }
        }

        let midi_events = self
            .controls
            .midi
            .as_ref()
            .map(|m| m.poll())
            .unwrap_or_default();
        for event in midi_events {
            let Some((key, value)) = MidiKey::parse(&event.bytes) else {
                continue;
            };
            self.controls.last_message =
                format!("MIDI {} = {value:.2} ({})", key.label(), event.port);
            if self.controls.learn
                && let Some(action) = self.controls.learn_target.take()
            {
                self.status = format!("Mapped {} to {}", key.label(), action.label());
                self.send(Command::MapMidi(MidiMapping { key, action }));
                continue;
            }
            let actions: Vec<_> = self
                .comp
                .midi
                .iter()
                .filter(|m| m.key == key)
                .map(|m| m.action.clone())
                .collect();
            for action in actions {
                self.run_action(&action, value);
            }
        }

        if let Some(audio) = &self.controls.audio {
            self.controls.levels = audio.levels();
            if self.controls.auto_bpm
                && let Some(bpm) = self.controls.levels.bpm
                && (bpm - self.comp.bpm).abs() > 0.5
            {
                self.send(Command::SetBpm(bpm));
            }
        } else {
            self.controls.levels = Levels::default();
        }
        self.controls.beats += dt * self.comp.bpm as f64 / 60.0;
    }

    pub(crate) fn tap_tempo(&mut self) {
        if let Some(bpm) = self.controls.tap.tap(Instant::now()) {
            self.send(Command::SetBpm(bpm));
        }
        // Each tap lands on a beat.
        self.controls.beats = self.controls.beats.round();
    }

    /// Copy of the composition with modulators applied, for rendering.
    pub(crate) fn modulated(&self) -> Composition {
        if self.comp.modulators.is_empty() {
            return self.comp.clone();
        }
        let signals = Signals {
            beats: self.controls.beats,
            audio: self.controls.levels.bands(),
        };
        verso_core::modulation::modulate(&self.comp, &signals, |p| self.param_range(&self.comp, p))
    }

    /// In learn mode, outlines `rect` as mappable and picks `action` when it is
    /// clicked. Returns `true` when learn mode is on, so callers skip their
    /// normal click behavior.
    pub(crate) fn learnable(&mut self, ui: &egui::Ui, rect: Rect, action: Action) -> bool {
        if !self.controls.learn {
            return false;
        }
        let target = self.controls.learn_target.as_ref() == Some(&action);
        let mapped = self
            .comp
            .midi
            .iter()
            .find(|m| m.action == action)
            .map(|m| m.key);
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            2.0,
            theme::accent().gamma_multiply(if target { 0.45 } else { 0.12 }),
        );
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(if target { 2.0 } else { 1.0 }, theme::accent()),
            StrokeKind::Inside,
        );
        if let Some(key) = mapped {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                key.label(),
                egui::FontId::proportional(9.5),
                theme::text(),
            );
        }
        let clicked = ui.input(|i| {
            i.pointer.primary_clicked()
                && i.pointer.interact_pos().is_some_and(|p| rect.contains(p))
        });
        if clicked {
            self.controls.learn_target = Some(action);
        }
        true
    }

    /// Right-click menu adding a modulator to `param`.
    pub(crate) fn modulation_menu(&mut self, response: &egui::Response, param: Param) {
        response.context_menu(|ui| {
            ui.label(
                egui::RichText::new(Action::Set(param.clone()).address())
                    .small()
                    .color(theme::text_dim()),
            );
            let mut add = |source| {
                self.send(Command::AddModulator(Modulator {
                    param: param.clone(),
                    source,
                    depth: 0.5,
                    bypass: false,
                }));
                self.controls.window_open = true;
            };
            if ui.button("Add LFO (1 bar sine)").clicked() {
                add(ModSource::Lfo {
                    waveform: Waveform::Sine,
                    beats: 4.0,
                });
                ui.close();
            }
            for band in [AudioBand::Bass, AudioBand::Level, AudioBand::Beat] {
                if ui.button(format!("Add Audio ({})", band.name())).clicked() {
                    add(ModSource::Audio { band });
                    ui.close();
                }
            }
        });
    }

    /// Top bar items: learn, tap, tempo, beat light and audio meter.
    pub(crate) fn control_bar(&mut self, ui: &mut egui::Ui) {
        let learn = self.controls.learn;
        let label = egui::RichText::new("MIDI Learn").color(if learn {
            theme::bg_deep()
        } else {
            theme::text()
        });
        let button = egui::Button::new(label).fill(if learn {
            theme::accent()
        } else {
            theme::bg_raised()
        });
        if ui
            .add(button)
            .on_hover_text("Click a control, then move a MIDI control (L)")
            .clicked()
        {
            self.toggle_learn();
        }
        if ui.button("Control…").clicked() {
            self.controls.window_open = !self.controls.window_open;
        }
        if ui.button("Scripts…").clicked() {
            self.scripts.window_open = !self.scripts.window_open;
        }
        ui.separator();

        // Beat light: on during the first quarter of each beat.
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
        let on = self.controls.beats.fract() < 0.25;
        ui.painter().circle_filled(
            rect.center(),
            4.5,
            if on {
                theme::accent()
            } else {
                theme::bg_deep()
            },
        );
        if ui.button("TAP").on_hover_text("Tap tempo (T)").clicked() {
            self.tap_tempo();
        }
        let mut bpm = self.comp.bpm;
        if ui
            .add(
                egui::DragValue::new(&mut bpm)
                    .range(20.0..=500.0)
                    .speed(0.1)
                    .suffix(" BPM"),
            )
            .changed()
        {
            self.send(Command::SetBpm(bpm));
        }
        if self.controls.audio.is_some() {
            ui.separator();
            let levels = self.controls.levels;
            for (label, value) in [("B", levels.bass), ("M", levels.mid), ("H", levels.high)] {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 1.0, theme::bg_deep());
                let fill = Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - rect.height() * value),
                    rect.right_bottom(),
                );
                ui.painter().rect_filled(fill, 1.0, theme::accent());
                let _ = label;
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                4.5,
                theme::solo().gamma_multiply(levels.beat.max(0.08)),
            );
        }
    }

    pub(crate) fn toggle_learn(&mut self) {
        self.controls.learn = !self.controls.learn;
        self.controls.learn_target = None;
        if self.controls.learn && self.controls.midi.is_none() {
            self.connect_midi();
        }
    }

    pub(crate) fn connect_midi(&mut self) {
        self.controls.midi = None;
        match MidiInputs::open_all() {
            Ok((midi, errors)) => {
                self.status = match (midi.len(), errors.first()) {
                    (_, Some(error)) => format!("MIDI: {error}"),
                    (0, None) => "No MIDI input found".to_owned(),
                    (n, None) => format!("MIDI: {n} input(s) connected"),
                };
                self.controls.midi = Some(midi);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(crate) fn start_osc(&mut self) {
        self.controls.osc = None;
        match OscServer::bind(self.controls.osc_port) {
            Ok(server) => {
                self.status = format!("OSC listening on port {}", server.port());
                self.controls.osc = Some(server);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn start_audio(&mut self) {
        self.controls.audio = None;
        match AudioInput::start(self.controls.audio_choice.as_deref()) {
            Ok(input) => {
                self.status = format!("Audio: {}", input.name());
                self.controls.audio = Some(input);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    pub(crate) fn control_window(&mut self, ctx: &egui::Context) {
        let mut open = self.controls.window_open;
        egui::Window::new("Control")
            .open(&mut open)
            .default_width(420.0)
            .default_pos([200.0, 120.0])
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.control_window_body(ui));
            });
        self.controls.window_open = open && self.controls.window_open;
    }

    fn control_window_body(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "OSC");
        ui.horizontal(|ui| {
            let listening = self.controls.osc.is_some();
            ui.label("Port");
            ui.add_enabled(
                !listening,
                egui::DragValue::new(&mut self.controls.osc_port).range(1024..=65535),
            );
            if !listening && ui.button("Listen").clicked() {
                self.start_osc();
            }
            if listening && ui.button("Stop").clicked() {
                self.controls.osc = None;
            }
        });
        ui.label(
            egui::RichText::new(
                "Values are 0–1. Addresses: /master, /crossfader, /bpm, /clear, /column/N/launch, /layer/N/opacity|bypass|solo|eject, /layer/N/clip/M/launch, /layer/N/effect/I/<param>, /group/N/…, /composition/effect/I/<param>. Also accepted: /composition/layers/N/clips/M/connect, /composition/columns/M/connect, /composition/layers/N/clear, /composition/layers/N/video/opacity, /composition/master, /composition/crossfader/phase, /composition/tempocontroller/tempo (BPM), /composition/disconnectall, so OSC presets made for other clip-grid software work as they are.",
            )
            .small()
            .color(theme::text_dim()),
        );

        ui.add_space(8.0);
        widgets::header(ui, "MIDI");
        ui.horizontal(|ui| {
            if ui.button("Connect Inputs").clicked() {
                self.connect_midi();
            }
            let mut learn = self.controls.learn;
            if ui.checkbox(&mut learn, "Learn").changed() {
                self.toggle_learn();
            }
        });
        if let Some(midi) = &self.controls.midi {
            let ports = if midi.is_empty() {
                "No inputs".to_owned()
            } else {
                midi.ports().join(", ")
            };
            ui.label(egui::RichText::new(ports).small().color(theme::text_dim()));
        }
        if let Some(target) = &self.controls.learn_target {
            ui.label(
                egui::RichText::new(format!("Move a MIDI control to map {}", target.address()))
                    .color(theme::accent()),
            );
        }
        let mut unmap = None;
        egui::Grid::new("midi mappings")
            .num_columns(3)
            .striped(true)
            .show(ui, |ui| {
                for mapping in &self.comp.midi {
                    ui.label(mapping.key.label());
                    ui.label(mapping.action.address());
                    if widgets::square_button(ui, "X", "Remove mapping").clicked() {
                        unmap = Some(mapping.key);
                    }
                    ui.end_row();
                }
            });
        if let Some(key) = unmap {
            self.send(Command::UnmapMidi(key));
        }

        ui.add_space(8.0);
        widgets::header(ui, "Audio");
        ui.horizontal(|ui| {
            let selected = self
                .controls
                .audio_choice
                .clone()
                .unwrap_or_else(|| "Default input".into());
            egui::ComboBox::from_id_salt("audio device")
                .selected_text(selected)
                .width(220.0)
                .show_ui(ui, |ui| {
                    if self.controls.audio_devices.is_empty() {
                        self.controls.audio_devices = verso_control::input_devices();
                    }
                    ui.selectable_value(&mut self.controls.audio_choice, None, "Default input");
                    for name in self.controls.audio_devices.clone() {
                        ui.selectable_value(
                            &mut self.controls.audio_choice,
                            Some(name.clone()),
                            name,
                        );
                    }
                });
            if self.controls.audio.is_none() {
                if ui.button("Start").clicked() {
                    self.start_audio();
                }
            } else if ui.button("Stop").clicked() {
                self.controls.audio = None;
            }
        });
        if self.controls.audio.is_some() {
            let levels = self.controls.levels;
            for (name, value) in [
                ("Level", levels.level),
                ("Bass", levels.bass),
                ("Mid", levels.mid),
                ("High", levels.high),
                ("Beat", levels.beat),
            ] {
                ui.horizontal(|ui| {
                    ui.add_sized([40.0, 14.0], egui::Label::new(name));
                    ui.add(
                        egui::ProgressBar::new(value)
                            .desired_height(8.0)
                            .desired_width(240.0),
                    );
                });
            }
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.controls.auto_bpm, "Follow detected tempo");
                let detected = levels
                    .bpm
                    .map(|b| format!("{b:.1} BPM"))
                    .unwrap_or_else(|| "listening…".into());
                ui.label(egui::RichText::new(detected).color(theme::text_dim()));
            });
        }

        ui.add_space(8.0);
        widgets::header(ui, "Modulation");
        if self.comp.modulators.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Right-click a fader or an effect parameter to add an LFO or audio modulation.",
                )
                .small()
                .color(theme::text_dim()),
            );
        }
        let mut changes = Vec::new();
        let mut remove = None;
        for (index, modulator) in self.comp.modulators.iter().enumerate() {
            let mut m = modulator.clone();
            egui::Frame::NONE
                .fill(theme::bg_header())
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = !m.bypass;
                        if widgets::toggle(ui, &mut on, "●", theme::accent(), "On / bypass")
                            .changed()
                        {
                            m.bypass = !on;
                        }
                        ui.label(
                            egui::RichText::new(Action::Set(m.param.clone()).address()).strong(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::square_button(ui, "X", "Remove").clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                    ui.horizontal(|ui| {
                        let is_lfo = matches!(m.source, ModSource::Lfo { .. });
                        if ui.selectable_label(is_lfo, "LFO").clicked() && !is_lfo {
                            m.source = ModSource::Lfo {
                                waveform: Waveform::Sine,
                                beats: 4.0,
                            };
                        }
                        if ui.selectable_label(!is_lfo, "Audio").clicked() && is_lfo {
                            m.source = ModSource::Audio {
                                band: AudioBand::Bass,
                            };
                        }
                        match &mut m.source {
                            ModSource::Lfo { waveform, beats } => {
                                egui::ComboBox::from_id_salt(("waveform", index))
                                    .selected_text(waveform.name())
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for w in Waveform::ALL {
                                            ui.selectable_value(waveform, w, w.name());
                                        }
                                    });
                                ui.add(
                                    egui::DragValue::new(beats)
                                        .range(0.125..=64.0)
                                        .speed(0.05)
                                        .suffix(" beats"),
                                );
                            }
                            ModSource::Audio { band } => {
                                egui::ComboBox::from_id_salt(("band", index))
                                    .selected_text(band.name())
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for b in AudioBand::ALL {
                                            ui.selectable_value(band, b, b.name());
                                        }
                                    });
                            }
                        }
                        ui.label("Depth");
                        ui.add(egui::Slider::new(&mut m.depth, -1.0..=1.0));
                    });
                });
            if m != *modulator {
                changes.push((index, m));
            }
        }
        for (index, modulator) in changes {
            self.send(Command::SetModulator { index, modulator });
        }
        if let Some(index) = remove {
            self.send(Command::RemoveModulator(index));
        }

        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(&self.controls.last_message)
                .small()
                .monospace()
                .color(theme::text_dim()),
        );
    }
}
