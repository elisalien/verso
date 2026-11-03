//! OSC, MIDI, audio and tempo in the app: polling inputs, MIDI learn, the
//! Control window and modulation.

use std::time::Instant;

use eframe::egui::{self, Rect, Stroke, StrokeKind, Vec2};
use verso_control::{AudioInput, Levels, MidiInputs, OscAccess, OscServer, TapTempo};
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
    /// Accept OSC from other computers, not only this one.
    pub osc_network: bool,
    /// Comma-separated addresses allowed to send OSC; empty allows all.
    pub osc_allowed: String,
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
    /// Where timecode comes from.
    pub tc_source: TcSource,
    pub mtc: verso_control::timecode::MtcDecoder,
    pub mtc_clock: verso_control::timecode::TimecodeClock,
    /// Current timecode and its position in seconds (extrapolated).
    pub timecode: Option<(verso_control::timecode::Timecode, f64)>,
    /// Position at the previous frame, to fire cues passed since.
    last_tc_seconds: Option<f64>,
    /// Text of the cue being added: an OSC address.
    pub cue_address: String,
    /// Ableton Link session, while joined.
    #[cfg(feature = "link")]
    pub link: Option<verso_control::link::Link>,
    /// Tempo last agreed with the Link session, to tell local changes apart.
    link_bpm: f32,
}

/// Source of SMPTE timecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TcSource {
    #[default]
    Off,
    /// MIDI Time Code from any MIDI input.
    Mtc,
    /// Linear Timecode on an audio input channel (0-based).
    Ltc(usize),
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            osc: None,
            osc_port: DEFAULT_OSC_PORT,
            osc_network: false,
            osc_allowed: String::new(),
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
            tc_source: TcSource::Off,
            mtc: Default::default(),
            mtc_clock: Default::default(),
            timecode: None,
            last_tc_seconds: None,
            cue_address: "/column/1/launch".into(),
            #[cfg(feature = "link")]
            link: None,
            link_bpm: 0.0,
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
            self.send_live(command);
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
            match Action::parse(&event.address) {
                // A message without a value acts like a button press.
                Some(action) => self.run_action(&action, event.value.unwrap_or(1.0)),
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
            if self.controls.tc_source == TcSource::Mtc
                && let Some(tc) = self.controls.mtc.push(&event.bytes)
            {
                self.controls.mtc_clock.set(tc);
                continue;
            }
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
                self.send_live(Command::SetBpm(bpm));
            }
        } else {
            self.controls.levels = Levels::default();
        }
        #[cfg(feature = "link")]
        let link = self.controls.link.as_mut();
        #[cfg(not(feature = "link"))]
        let link: Option<&mut NoLink> = None;
        match link {
            Some(link) => {
                let (bpm, beats) = link.capture();
                let bpm = bpm as f32;
                if (self.comp.bpm - self.controls.link_bpm).abs() > 0.01 {
                    // Changed here (tap, fader, MIDI): lead the session.
                    link.set_tempo(self.comp.bpm as f64);
                    self.controls.link_bpm = self.comp.bpm;
                } else if (bpm - self.comp.bpm).abs() > 0.01 {
                    // Changed by another peer: follow.
                    self.controls.link_bpm = bpm;
                    self.send_live(Command::SetBpm(bpm));
                }
                // Beats follow the session phase, so LFOs and beat()
                // scripts land on the same downbeat as the other apps.
                self.controls.beats = beats;
            }
            None => self.controls.beats += dt * self.comp.bpm as f64 / 60.0,
        }
        self.update_timecode();
    }

    /// Reads the timecode source and fires the cues passed since last frame.
    fn update_timecode(&mut self) {
        use verso_control::timecode::TimecodeClock;
        let clock: Option<TimecodeClock> = match self.controls.tc_source {
            TcSource::Off => None,
            TcSource::Mtc => Some(self.controls.mtc_clock),
            TcSource::Ltc(_) => self
                .controls
                .audio
                .as_ref()
                .and_then(|a| a.ltc())
                .map(|(_, clock)| clock),
        };
        self.controls.timecode = clock.and_then(|c| Some((c.current()?, c.seconds()?)));
        let now = self.controls.timecode.map(|(_, s)| s);
        if let (Some(from), Some(to)) = (self.controls.last_tc_seconds, now) {
            for index in verso_core::control::Cue::passed(&self.comp.cues, from, to) {
                let cue = self.comp.cues[index].clone();
                self.run_action(&cue.action, cue.value);
            }
        }
        self.controls.last_tc_seconds = now;
    }

    #[cfg(not(feature = "link"))]
    pub(crate) fn toggle_link(&mut self) {
        self.status = "This build of Verso has no Ableton Link (feature `link`)".into();
    }

    #[cfg(feature = "link")]
    pub(crate) fn toggle_link(&mut self) {
        if self.controls.link.take().is_some() {
            self.status = "Left the Link session".into();
        } else {
            self.controls.link = Some(verso_control::link::Link::new(self.comp.bpm as f64));
            self.controls.link_bpm = self.comp.bpm;
            self.status = "Joined the Link session".into();
        }
    }

    /// Applies the timecode source choice (LTC needs the audio input).
    pub(crate) fn set_tc_source(&mut self, source: TcSource) {
        self.controls.tc_source = source;
        let ltc = match source {
            TcSource::Ltc(channel) => Some(channel),
            _ => None,
        };
        if ltc.is_some() && self.controls.audio.is_none() {
            self.start_audio();
        }
        if let Some(audio) = &mut self.controls.audio {
            audio.set_ltc_channel(ltc);
        }
        if source == TcSource::Mtc && self.controls.midi.is_none() {
            self.status = "MTC: open the MIDI inputs (Control › MIDI) to receive timecode".into();
        }
    }

    pub(crate) fn tap_tempo(&mut self) {
        if let Some(bpm) = self.controls.tap.tap(Instant::now()) {
            self.send_live(Command::SetBpm(bpm));
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
            theme::ACCENT.gamma_multiply(if target { 0.45 } else { 0.12 }),
        );
        painter.rect_stroke(
            rect,
            2.0,
            Stroke::new(if target { 2.0 } else { 1.0 }, theme::ACCENT),
            StrokeKind::Inside,
        );
        if let Some(key) = mapped {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                key.label(),
                egui::FontId::proportional(9.5),
                theme::TEXT,
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
                    .color(theme::TEXT_DIM),
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
            theme::BG_DEEP
        } else {
            theme::TEXT
        });
        let button = egui::Button::new(label).fill(if learn {
            theme::ACCENT
        } else {
            theme::BG_RAISED
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
            if on { theme::ACCENT } else { theme::BG_DEEP },
        );
        #[cfg(feature = "link")]
        let peers = self.controls.link.as_ref().map(|l| l.peers());
        #[cfg(not(feature = "link"))]
        let peers: Option<u64> = None;
        let link_label = match peers {
            Some(n) => egui::RichText::new(format!("LINK {n}")).color(theme::BG_DEEP),
            None => egui::RichText::new("LINK"),
        };
        let link_button = egui::Button::new(link_label).fill(if peers.is_some() {
            theme::ACCENT
        } else {
            theme::BG_RAISED
        });
        if ui
            .add(link_button)
            .on_hover_text("Ableton Link: share tempo and beat phase with apps on the network (number of peers)")
            .clicked()
        {
            self.toggle_link();
        }
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
        if let Some((tc, _)) = self.controls.timecode {
            ui.separator();
            ui.label(
                egui::RichText::new(tc.to_string())
                    .monospace()
                    .color(theme::SOLO),
            )
            .on_hover_text(format!("Timecode ({} fps)", tc.rate.name()));
        }
        if self.controls.audio.is_some() {
            ui.separator();
            let levels = self.controls.levels;
            for (label, value) in [("B", levels.bass), ("M", levels.mid), ("H", levels.high)] {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(6.0, 16.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 1.0, theme::BG_DEEP);
                let fill = Rect::from_min_max(
                    egui::pos2(rect.left(), rect.bottom() - rect.height() * value),
                    rect.right_bottom(),
                );
                ui.painter().rect_filled(fill, 1.0, theme::ACCENT);
                let _ = label;
            }
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
            ui.painter().circle_filled(
                rect.center(),
                4.5,
                theme::SOLO.gamma_multiply(levels.beat.max(0.08)),
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
        let access = if self.controls.osc_network {
            let mut allowed = Vec::new();
            for part in self.controls.osc_allowed.split([',', ' ']) {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                match part.parse() {
                    Ok(ip) => allowed.push(ip),
                    Err(_) => {
                        self.status = format!("OSC: {part} is not an IP address");
                        return;
                    }
                }
            }
            OscAccess::Network { allowed }
        } else {
            OscAccess::Local
        };
        let scope = match &access {
            OscAccess::Local => "this computer only".to_owned(),
            OscAccess::Network { allowed } if allowed.is_empty() => "the network".to_owned(),
            OscAccess::Network { allowed } => format!("{} address(es)", allowed.len()),
        };
        match OscServer::bind_with(self.controls.osc_port, access) {
            Ok(server) => {
                self.status = format!("OSC listening on port {} from {scope}", server.port());
                self.controls.osc = Some(server);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    fn start_audio(&mut self) {
        self.controls.audio = None;
        match AudioInput::start(self.controls.audio_choice.as_deref()) {
            Ok(mut input) => {
                self.status = format!("Audio: {}", input.name());
                if let TcSource::Ltc(channel) = self.controls.tc_source {
                    input.set_ltc_channel(Some(channel));
                }
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
            if let Some(server) = &self.controls.osc
                && server.dropped() > 0
            {
                ui.label(
                    egui::RichText::new(format!("{} refused", server.dropped()))
                        .small()
                        .color(theme::WARN),
                )
                .on_hover_text("Packets from senders not allowed, malformed or arriving too fast");
            }
        });
        ui.add_enabled_ui(self.controls.osc.is_none(), |ui| {
            ui.checkbox(&mut self.controls.osc_network, "Accept from other computers")
                .on_hover_text(
                    "Off: only programs on this computer can send OSC. On: anyone on the network can control the show, unless you list allowed addresses.",
                );
            if self.controls.osc_network {
                ui.horizontal(|ui| {
                    ui.label("Allowed");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.controls.osc_allowed)
                            .hint_text("all — or e.g. 192.168.1.20, 192.168.1.21")
                            .desired_width(240.0),
                    );
                });
            }
        });
        ui.label(
            egui::RichText::new(
                "Values are 0–1. Addresses: /master, /crossfader, /bpm, /clear, /column/N/launch, /layer/N/opacity|bypass|solo|eject, /layer/N/clip/M/launch, /layer/N/effect/I/<param>, /group/N/…, /composition/effect/I/<param>",
            )
            .small()
            .color(theme::TEXT_DIM),
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
            ui.label(egui::RichText::new(ports).small().color(theme::TEXT_DIM));
        }
        if let Some(target) = &self.controls.learn_target {
            ui.label(
                egui::RichText::new(format!("Move a MIDI control to map {}", target.address()))
                    .color(theme::ACCENT),
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
                ui.label(egui::RichText::new(detected).color(theme::TEXT_DIM));
            });
        }

        ui.add_space(8.0);
        self.timecode_section(ui);

        ui.add_space(8.0);
        widgets::header(ui, "Modulation");
        if self.comp.modulators.is_empty() {
            ui.label(
                egui::RichText::new(
                    "Right-click a fader or an effect parameter to add an LFO or audio modulation.",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        }
        let mut changes = Vec::new();
        let mut remove = None;
        for (index, modulator) in self.comp.modulators.iter().enumerate() {
            let mut m = modulator.clone();
            egui::Frame::NONE
                .fill(theme::BG_HEADER)
                .inner_margin(egui::Margin::same(4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = !m.bypass;
                        if widgets::toggle(ui, &mut on, "●", theme::ACCENT, "On / bypass").changed()
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
                .color(theme::TEXT_DIM),
        );
    }
}

impl VersoApp {
    fn timecode_section(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "Timecode");
        let channels = self
            .controls
            .audio
            .as_ref()
            .map(|a| a.channels())
            .unwrap_or(2);
        let label = |source: TcSource| match source {
            TcSource::Off => "Off".to_owned(),
            TcSource::Mtc => "MIDI (MTC)".to_owned(),
            TcSource::Ltc(c) => format!("Audio LTC · channel {}", c + 1),
        };
        ui.horizontal(|ui| {
            ui.label("Source");
            let mut source = self.controls.tc_source;
            egui::ComboBox::from_id_salt("tc source")
                .selected_text(label(source))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut source, TcSource::Off, label(TcSource::Off));
                    ui.selectable_value(&mut source, TcSource::Mtc, label(TcSource::Mtc));
                    for c in 0..channels.min(8) {
                        ui.selectable_value(&mut source, TcSource::Ltc(c), label(TcSource::Ltc(c)));
                    }
                });
            if source != self.controls.tc_source {
                self.set_tc_source(source);
            }
            let text = match (self.controls.tc_source, self.controls.timecode) {
                (TcSource::Off, _) => String::new(),
                (_, Some((tc, _))) => format!("{tc}  ({} fps)", tc.rate.name()),
                (_, None) => "no signal".into(),
            };
            ui.label(egui::RichText::new(text).monospace().color(theme::TEXT_DIM));
        });

        ui.label(
            egui::RichText::new(
                "Cues fire an OSC address when the timecode plays past them (jumps do not fire).",
            )
            .small()
            .color(theme::TEXT_DIM),
        );
        let mut remove = None;
        let mut edit = None;
        for (index, cue) in self.comp.cues.iter().enumerate() {
            ui.horizontal(|ui| {
                let mut seconds = cue.seconds;
                if ui
                    .add(
                        egui::DragValue::new(&mut seconds)
                            .range(0.0..=86_400.0)
                            .speed(0.04)
                            .custom_formatter(|v, _| format_seconds(v)),
                    )
                    .changed()
                {
                    edit = Some((
                        index,
                        verso_core::control::Cue {
                            seconds,
                            ..cue.clone()
                        },
                    ));
                }
                ui.label(egui::RichText::new(cue.action.address()).monospace());
                if ui.small_button("×").on_hover_text("Remove cue").clicked() {
                    remove = Some(index);
                }
            });
        }
        if let Some((index, cue)) = edit {
            self.send(Command::SetCue { index, cue });
        }
        if let Some(index) = remove {
            self.send(Command::RemoveCue(index));
        }
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.controls.cue_address)
                    .desired_width(180.0)
                    .hint_text("/column/1/launch"),
            );
            let at = self.controls.timecode.map(|(_, s)| s).unwrap_or(0.0);
            let action = Action::parse(self.controls.cue_address.trim());
            if ui
                .add_enabled(
                    action.is_some(),
                    egui::Button::new(format!("Add at {}", format_seconds(at))),
                )
                .on_hover_text("Adds a cue at the current timecode; drag its time to adjust")
                .on_disabled_hover_text("Not a Verso OSC address")
                .clicked()
                && let Some(action) = action
            {
                self.send(Command::AddCue(verso_core::control::Cue {
                    seconds: at,
                    action,
                    value: 1.0,
                }));
            }
        });
    }
}

/// `h:mm:ss.ss` for cue times.
fn format_seconds(seconds: f64) -> String {
    let total = seconds.max(0.0);
    let h = (total / 3600.0) as u64;
    let m = (total / 60.0) as u64 % 60;
    let s = total % 60.0;
    format!("{h}:{m:02}:{s:05.2}")
}

/// Stand-in for builds without Ableton Link: has no values.
#[cfg(not(feature = "link"))]
enum NoLink {}

#[cfg(not(feature = "link"))]
impl NoLink {
    fn capture(&mut self) -> (f64, f64) {
        match *self {}
    }

    fn set_tempo(&mut self, _bpm: f64) {
        match *self {}
    }
}
