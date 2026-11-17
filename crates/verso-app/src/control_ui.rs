//! OSC, MIDI, audio and tempo in the app: polling inputs, MIDI learn, the
//! Control window and modulation.

use std::time::Instant;

use eframe::egui::{self, Rect, Stroke, StrokeKind, Vec2};
use verso_control::{
    AnalysisSettings, AudioInput, Levels, MidiInputs, OscAccess, OscServer, SPECTRUM_BANDS,
    SPECTRUM_HIGH_HZ, SPECTRUM_LOW_HZ, TapTempo,
};
use verso_core::control::{Action, MidiKey, MidiMapping, Param, Range};
use verso_core::modulation::{AudioBand, ModSource, Modulator, MouseAxis, Signals, Waveform};
use verso_core::{Command, Composition};
use verso_render::ParamKind;

use crate::app::VersoApp;
use crate::{theme, widgets};

pub const DEFAULT_OSC_PORT: u16 = 7000;

/// How often the MIDI ports are listed again, so a controller plugged in
/// after Verso started is picked up without any click.
const MIDI_SCAN_SECONDS: f64 = 2.0;
/// Wait before a lost audio input is tried again; doubles up to the maximum.
const AUDIO_RETRY: std::time::Duration = std::time::Duration::from_secs(2);
const AUDIO_RETRY_MAX: std::time::Duration = std::time::Duration::from_secs(10);
/// Time between two checks of the default output device.
const AUDIO_CHECK: std::time::Duration = std::time::Duration::from_secs(3);

/// Live control inputs and their settings. Not saved with the composition.
pub struct Controls {
    pub osc: Option<OscServer>,
    pub osc_port: u16,
    /// Accept OSC from other computers, not only this one.
    pub osc_network: bool,
    /// Comma-separated addresses allowed to send OSC; empty allows all.
    pub osc_allowed: String,
    pub midi: Option<MidiInputs>,
    /// MIDI input ports present at the last scan, to notice plugged and
    /// unplugged devices.
    midi_present: Vec<String>,
    /// Seconds until the next scan of MIDI ports.
    midi_scan_in: f64,
    /// MIDI input names, listed when the device list opens.
    pub midi_devices: Vec<String>,
    pub audio: Option<AudioInput>,
    /// Input and output device names, listed when a device list opens.
    pub audio_devices: Vec<String>,
    pub output_devices: Vec<String>,
    /// Audio stream error already shown in the status bar.
    audio_error_shown: Option<String>,
    /// The input was lost (unplugged): when to try it again, and the wait
    /// before the try after that.
    audio_retry: Option<(Instant, std::time::Duration)>,
    /// Next check that the default output is still the captured one.
    audio_check_at: Instant,
    /// Test tone playing on the output, until then.
    pub test_tone: Option<(verso_control::TestTone, Instant)>,
    pub mouse: crate::mouse::MouseControl,
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
            midi_present: Vec::new(),
            midi_scan_in: MIDI_SCAN_SECONDS,
            midi_devices: Vec::new(),
            audio: None,
            audio_devices: Vec::new(),
            output_devices: Vec::new(),
            audio_error_shown: None,
            audio_retry: None,
            audio_check_at: Instant::now(),
            test_tone: None,
            mouse: Default::default(),
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
        if let Some(found) = param.blend_range(comp) {
            return Some(found);
        }
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
            match Action::parse_osc(&event.address, event.value) {
                Some((action, value)) => self.run_action(&action, value),
                None => self.status = format!("Unknown OSC address {}", event.address),
            }
        }

        self.controls.midi_scan_in -= dt;
        if self.controls.midi_scan_in <= 0.0 {
            self.controls.midi_scan_in = MIDI_SCAN_SECONDS;
            if verso_control::midi_input_ports() != self.controls.midi_present {
                self.connect_midi();
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
            if self.controls.learn {
                match self.controls.learn_target.take() {
                    Some(action) => {
                        self.status = format!("Mapped {} to {}", key.label(), action.label());
                        self.send(Command::MapMidi(MidiMapping { key, action }));
                        continue;
                    }
                    // A message with nothing picked: say what to do instead of
                    // ignoring it silently. Mapped keys still run below.
                    None if !self.comp.midi.iter().any(|m| m.key == key) => {
                        self.status = format!(
                            "MIDI Learn: {} received from {}. Click a control first, then move the MIDI control.",
                            key.label(),
                            event.port
                        );
                        continue;
                    }
                    None => {}
                }
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

        self.watch_audio();
        if let Some(audio) = &self.controls.audio {
            self.controls.levels = audio.levels();
            // Each stream error is shown once, not every frame.
            let error = audio.error();
            if error.is_some() && error != self.controls.audio_error_shown {
                let text = format!("Audio: {}", error.as_deref().unwrap_or_default());
                log::warn!("{text}");
                self.status = text;
                self.controls.audio_error_shown = error;
            }
            if self.controls.audio_error_shown.is_some() {
                // A lost input keeps its last levels: modulators would stay
                // frozen on them. Silence instead, until it is back.
                self.controls.levels = Levels::default();
                self.controls
                    .audio_retry
                    .get_or_insert((Instant::now() + AUDIO_RETRY, AUDIO_RETRY));
            }
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
            for index in verso_core::control::Cue::passed(&self.comp.timecode_cues, from, to) {
                let cue = self.comp.timecode_cues[index].clone();
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
            mouse: self.controls.mouse.signals(),
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
        // A press picks it, even if the pointer then moves a little (a
        // slider grabbed rather than clicked). Only when nothing covers it.
        let pressed = ui.rect_contains_pointer(rect) && ui.input(|i| i.pointer.primary_pressed());
        if pressed {
            self.status = format!("MIDI Learn: move a MIDI control to map {}", action.label());
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
            if ui
                .button("Add Mouse (X)")
                .on_hover_text("Follows the pointer over the Preview or Output monitor")
                .clicked()
            {
                add(ModSource::Mouse { axis: MouseAxis::X });
                ui.close();
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
        #[cfg(feature = "link")]
        let peers = self.controls.link.as_ref().map(|l| l.peers());
        #[cfg(not(feature = "link"))]
        let peers: Option<u64> = None;
        let link_label = match peers {
            Some(n) => egui::RichText::new(format!("LINK {n}")).color(theme::bg_deep()),
            None => egui::RichText::new("LINK"),
        };
        let link_button = egui::Button::new(link_label).fill(if peers.is_some() {
            theme::accent()
        } else {
            theme::bg_raised()
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
                    .color(theme::solo()),
            )
            .on_hover_text(format!("Timecode ({} fps)", tc.rate.name()));
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
        if self.controls.learn {
            // Opened again so a device plugged in since is there.
            self.connect_midi();
            if self.controls.midi.as_ref().is_some_and(|m| !m.is_empty()) {
                self.status = "MIDI Learn: click a control, then move a MIDI control".into();
            }
        }
    }

    /// Opens the MIDI input chosen in Settings (every input by default) and
    /// shows the result in the status bar.
    pub(crate) fn connect_midi(&mut self) {
        if let Some(message) = self.open_midi() {
            self.status = message;
        }
    }

    /// Opens the chosen MIDI input; returns a message worth showing, if any.
    pub(crate) fn open_midi(&mut self) -> Option<String> {
        // Closed first: some systems open an input only once at a time.
        self.controls.midi = None;
        self.controls.midi_present = verso_control::midi_input_ports();
        self.controls.midi_scan_in = MIDI_SCAN_SECONDS;
        let wanted = self.prefs.midi_input.clone();
        match MidiInputs::open(wanted.as_deref()) {
            Ok((midi, errors)) => {
                let message = match (midi.len(), errors.first(), &wanted) {
                    (_, Some(error), _) => Some(format!("MIDI: {error}")),
                    (0, None, Some(name)) => Some(format!("MIDI: {name} not found")),
                    (0, None, None) => None,
                    (_, None, _) => Some(format!("MIDI: {}", midi.ports().join(", "))),
                };
                self.controls.midi = Some(midi);
                message
            }
            Err(e) => Some(e.to_string()),
        }
    }

    /// MIDI input chooser: one device, or every device. Returns `true` when
    /// the choice changed.
    pub(crate) fn midi_input_choice(&mut self, ui: &mut egui::Ui) -> bool {
        device_choice(
            ui,
            "midi input",
            &mut self.prefs.midi_input,
            &mut self.controls.midi_devices,
            "All MIDI devices",
            verso_control::midi_input_ports,
        )
    }

    /// Ports connected now, or why none is.
    pub(crate) fn midi_ports_text(&self) -> String {
        match &self.controls.midi {
            Some(midi) if !midi.is_empty() => format!("Connected: {}", midi.ports().join(", ")),
            Some(_) if self.controls.midi_present.is_empty() => "No MIDI device found".into(),
            Some(_) => format!(
                "Not connected. Devices present: {}",
                self.controls.midi_present.join(", ")
            ),
            None => "Not connected".into(),
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

    /// Starts a lost input again (with a growing wait between tries, as a
    /// try can take a moment), and follows the default output when Windows
    /// switches to another device.
    fn watch_audio(&mut self) {
        let now = Instant::now();
        if let Some((at, wait)) = self.controls.audio_retry {
            if now < at {
                return;
            }
            self.start_audio();
            if self.controls.audio.is_some() {
                log::info!("Audio input back");
            } else {
                let wait = (wait * 2).min(AUDIO_RETRY_MAX);
                self.controls.audio_retry = Some((now + wait, wait));
            }
        } else if now >= self.controls.audio_check_at {
            self.controls.audio_check_at = now + AUDIO_CHECK;
            if self
                .controls
                .audio
                .as_ref()
                .is_some_and(|a| a.default_output_moved())
            {
                log::info!("Default output changed: following it");
                self.start_audio();
            }
        }
    }

    pub(crate) fn start_audio(&mut self) {
        self.controls.audio = None;
        self.controls.audio_error_shown = None;
        self.controls.audio_retry = None;
        match AudioInput::start(self.prefs.audio_input.as_deref()) {
            Ok(mut input) => {
                self.status = format!("Audio: {}", input.name());
                input.set_settings(self.prefs.audio);
                if let TcSource::Ltc(channel) = self.controls.tc_source {
                    input.set_ltc_channel(Some(channel));
                }
                self.controls.audio = Some(input);
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    /// Input device list. Returns whether the choice changed.
    pub(crate) fn audio_input_choice(&mut self, ui: &mut egui::Ui) -> bool {
        let controls = &mut self.controls;
        device_choice(
            ui,
            "audio input",
            &mut self.prefs.audio_input,
            &mut controls.audio_devices,
            "Default input",
            audio_sources,
        )
    }

    /// Start or Stop button for the audio input.
    pub(crate) fn audio_start_stop(&mut self, ui: &mut egui::Ui) {
        if self.controls.audio.is_none() {
            if ui
                .button("Start")
                .on_hover_text("Starts listening to the chosen input")
                .clicked()
            {
                self.start_audio();
            }
        } else if ui.button("Stop").clicked() {
            self.controls.audio = None;
            self.controls.audio_retry = None;
        }
    }

    /// Level, band and beat bars of the running audio input.
    pub(crate) fn audio_meters(&self, ui: &mut egui::Ui) {
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
    }

    /// Output device list. Returns whether the choice changed.
    pub(crate) fn audio_output_choice(&mut self, ui: &mut egui::Ui) -> bool {
        let controls = &mut self.controls;
        device_choice(
            ui,
            "audio output",
            &mut self.prefs.audio_output,
            &mut controls.output_devices,
            "Default output",
            verso_control::output_devices,
        )
    }

    /// Plays a second of test tone on the chosen output.
    pub(crate) fn play_test_tone(&mut self) {
        match verso_control::TestTone::start(self.prefs.audio_output.as_deref()) {
            Ok(tone) => {
                self.controls.test_tone =
                    Some((tone, Instant::now() + std::time::Duration::from_secs(1)));
            }
            Err(e) => self.problem(e.to_string()),
        }
    }

    /// Gain, bands and fall-off of the audio analysis, with the spectrum.
    fn analysis_settings(&mut self, ui: &mut egui::Ui) {
        let Some(audio) = &self.controls.audio else {
            return;
        };
        let spectrum = audio.spectrum();
        let mut settings = self.prefs.audio;
        spectrum_view(ui, &spectrum, &mut settings);
        egui::Grid::new("audio analysis")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Gain");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Slider::new(&mut settings.gain_db, AnalysisSettings::GAIN_DB)
                            .step_by(0.5)
                            .suffix(" dB"),
                    )
                    .on_hover_text("Raises or lowers every band before it reaches 1");
                    ui.checkbox(&mut settings.auto_gain, "Auto").on_hover_text(
                        "Each band against its recent peak, so quiet and loud \
                             inputs both use the whole range",
                    );
                });
                ui.end_row();
                ui.label("Fall-off");
                ui.add(
                    egui::Slider::new(&mut settings.falloff, AnalysisSettings::FALLOFF)
                        .logarithmic(true)
                        .suffix(" s"),
                )
                .on_hover_text("Time a level takes to fall by half");
                ui.end_row();
                ui.label("Bass up to");
                ui.add(
                    egui::DragValue::new(&mut settings.bass_mid)
                        .range(AnalysisSettings::CROSSOVER)
                        .speed(2.0)
                        .suffix(" Hz"),
                );
                ui.end_row();
                ui.label("High from");
                ui.add(
                    egui::DragValue::new(&mut settings.mid_high)
                        .range(AnalysisSettings::CROSSOVER)
                        .speed(10.0)
                        .suffix(" Hz"),
                );
                ui.end_row();
            });
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    settings != AnalysisSettings::default(),
                    egui::Button::new("Reset").small(),
                )
                .clicked()
            {
                settings = AnalysisSettings::default();
            }
        });
        let settings = settings.sanitized();
        if settings != self.prefs.audio {
            audio.set_settings(settings);
            self.prefs.audio = settings;
            self.prefs_unsaved = true;
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
                        .color(theme::warn()),
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
                "Values are 0–1. Addresses: /master, /crossfader, /bpm, /clear, /column/N/launch, /layer/N/opacity|bypass|solo|eject, /layer/N/clip/M/launch, /layer/N/effect/I/<param>, /group/N/…, /composition/effect/I/<param>. Also accepted: /composition/layers/N/clips/M/connect, /composition/columns/M/connect, /composition/layers/N/clear, /composition/layers/N/video/opacity, /composition/master, /composition/crossfader/phase, /composition/tempocontroller/tempo (BPM), /composition/disconnectall, so OSC presets made for other clip-grid software work as they are.",
            )
            .small()
            .color(theme::text_dim()),
        );

        ui.add_space(8.0);
        widgets::header(ui, "MIDI");
        ui.horizontal(|ui| {
            if self.midi_input_choice(ui) {
                self.prefs.save();
                self.connect_midi();
            }
            if ui
                .button("Reconnect")
                .on_hover_text("Opens the MIDI input again")
                .clicked()
            {
                self.connect_midi();
            }
            let mut learn = self.controls.learn;
            if ui.checkbox(&mut learn, "Learn").changed() {
                self.toggle_learn();
            }
        });
        ui.label(
            egui::RichText::new(self.midi_ports_text())
                .small()
                .color(theme::text_dim()),
        );
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
            if self.audio_input_choice(ui) {
                self.prefs.save();
                self.start_audio();
            }
            self.audio_start_stop(ui);
        });
        if self.controls.audio.is_some() {
            let levels = self.controls.levels;
            self.audio_meters(ui);
            self.analysis_settings(ui);
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
        widgets::header(ui, "Mouse");
        ui.label(
            egui::RichText::new(
                "The pointer over the Preview or Output monitor drives Mouse modulators \
                 and the iMouse input of ShaderToy shaders.",
            )
            .small()
            .color(theme::text_dim()),
        );
        let mouse = self.controls.mouse.signals();
        for axis in MouseAxis::ALL {
            ui.horizontal(|ui| {
                ui.add_sized([40.0, 14.0], egui::Label::new(axis.name()));
                ui.add(
                    egui::ProgressBar::new(mouse[axis as usize])
                        .desired_height(8.0)
                        .desired_width(240.0),
                );
            });
        }
        if ui
            .checkbox(
                &mut self.prefs.mouse_in_output,
                "Also over the output window",
            )
            .on_hover_text("For a touch screen or a second monitor used as a controller")
            .changed()
        {
            self.prefs.save();
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
                        let kinds = [
                            (
                                "LFO",
                                ModSource::Lfo {
                                    waveform: Waveform::Sine,
                                    beats: 4.0,
                                },
                            ),
                            (
                                "Audio",
                                ModSource::Audio {
                                    band: AudioBand::Bass,
                                },
                            ),
                            ("Mouse", ModSource::Mouse { axis: MouseAxis::X }),
                        ];
                        for (label, source) in kinds {
                            let same = std::mem::discriminant(&m.source)
                                == std::mem::discriminant(&source);
                            if ui.selectable_label(same, label).clicked() && !same {
                                m.source = source;
                            }
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
                            ModSource::Mouse { axis } => {
                                egui::ComboBox::from_id_salt(("mouse axis", index))
                                    .selected_text(axis.name())
                                    .width(80.0)
                                    .show_ui(ui, |ui| {
                                        for a in MouseAxis::ALL {
                                            ui.selectable_value(axis, a, a.name());
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
            ui.label(
                egui::RichText::new(text)
                    .monospace()
                    .color(theme::text_dim()),
            );
        });

        ui.label(
            egui::RichText::new(
                "Cues fire an OSC address when the timecode plays past them (jumps do not fire).",
            )
            .small()
            .color(theme::text_dim()),
        );
        let mut remove = None;
        let mut edit = None;
        for (index, cue) in self.comp.timecode_cues.iter().enumerate() {
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
            self.send(Command::SetTimecodeCue { index, cue });
        }
        if let Some(index) = remove {
            self.send(Command::RemoveTimecodeCue(index));
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
                self.send(Command::AddTimecodeCue(verso_core::control::Cue {
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

/// Position of `hz` across the spectrum display, 0 to 1 (log scale).
fn spectrum_x(hz: f32) -> f32 {
    (hz / SPECTRUM_LOW_HZ).ln() / (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).ln()
}

fn spectrum_hz(x: f32) -> f32 {
    SPECTRUM_LOW_HZ * (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(x.clamp(0.0, 1.0))
}

/// Spectrum bars over the bass, mid and high bands; dragging a band edge
/// moves it.
fn spectrum_view(
    ui: &mut egui::Ui,
    spectrum: &[f32; SPECTRUM_BANDS],
    settings: &mut AnalysisSettings,
) {
    let size = Vec2::new(ui.available_width().clamp(200.0, 320.0), 70.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::bg_deep());
    let x_of = |hz: f32| rect.left() + spectrum_x(hz) * rect.width();
    let edges = [x_of(settings.bass_mid), x_of(settings.mid_high)];
    let accent = theme::accent();
    for (i, (left, right)) in [
        (rect.left(), edges[0]),
        (edges[0], edges[1]),
        (edges[1], rect.right()),
    ]
    .into_iter()
    .enumerate()
    {
        let tint = accent.gamma_multiply([0.10, 0.05, 0.10][i]);
        painter.rect_filled(
            Rect::from_x_y_ranges(left..=right, rect.y_range()),
            0.0,
            tint,
        );
    }
    let bar = rect.width() / SPECTRUM_BANDS as f32;
    for (i, &value) in spectrum.iter().enumerate() {
        let left = rect.left() + i as f32 * bar;
        let top = rect.bottom() - value * rect.height();
        painter.rect_filled(
            Rect::from_min_max(
                egui::pos2(left + 0.5, top),
                egui::pos2(left + bar - 0.5, rect.bottom()),
            ),
            0.0,
            theme::text_dim(),
        );
    }

    // The edge being dragged stays picked until the button is released.
    let id = response.id.with("edge");
    let pointer = response.interact_pointer_pos();
    if response.drag_started()
        && let Some(pos) = pointer
    {
        let nearest = (0..2).min_by(|&a, &b| {
            (edges[a] - pos.x)
                .abs()
                .total_cmp(&(edges[b] - pos.x).abs())
        });
        ui.data_mut(|d| d.insert_temp(id, nearest));
    }
    let dragged: Option<usize> = ui.data(|d| d.get_temp(id)).flatten();
    if response.dragged()
        && let (Some(edge), Some(pos)) = (dragged, pointer)
    {
        let hz = spectrum_hz((pos.x - rect.left()) / rect.width());
        match edge {
            0 => settings.bass_mid = hz.min(settings.mid_high / 1.25),
            _ => settings.mid_high = hz.max(settings.bass_mid * 1.25),
        }
    }
    if response.drag_stopped() {
        ui.data_mut(|d| d.remove::<Option<usize>>(id));
    }
    let hovered_edge = response
        .hover_pos()
        .is_some_and(|pos| edges.iter().any(|x| (x - pos.x).abs() < 6.0));
    if hovered_edge || dragged.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    for (i, x) in edges.into_iter().enumerate() {
        let strong = dragged == Some(i);
        painter.vline(
            x,
            rect.y_range(),
            Stroke::new(if strong { 2.0 } else { 1.0 }, accent),
        );
    }
    for (label, hz) in [
        ("Bass", 20.0f32),
        ("Mid", settings.bass_mid),
        ("High", settings.mid_high),
    ] {
        painter.text(
            egui::pos2(x_of(hz.max(SPECTRUM_LOW_HZ)) + 3.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            label,
            egui::FontId::proportional(10.0),
            theme::text(),
        );
    }
    response.on_hover_text("Drag a band edge to move it");
}

/// Audio inputs, then the outputs whose sound can be captured ("System
/// sound: ..." entries, Windows only).
fn audio_sources() -> Vec<String> {
    let mut devices = verso_control::input_devices();
    devices.extend(verso_control::loopback_devices());
    devices
}

/// A device list: the system default first, then the devices found when the
/// list opens. Returns whether the choice changed.
fn device_choice(
    ui: &mut egui::Ui,
    id: &str,
    choice: &mut Option<String>,
    devices: &mut Vec<String>,
    default: &str,
    list: fn() -> Vec<String>,
) -> bool {
    let before = choice.clone();
    let selected = choice.clone().unwrap_or_else(|| default.to_owned());
    let response = egui::ComboBox::from_id_salt(id)
        .selected_text(selected)
        .width(220.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(choice, None, default);
            for name in devices.iter() {
                ui.selectable_value(choice, Some(name.clone()), name);
            }
            // A device chosen earlier but not plugged in stays shown.
            if let Some(name) = before.as_ref().filter(|n| !devices.contains(n)) {
                ui.selectable_value(choice, Some(name.clone()), format!("{name} (not found)"));
            }
        });
    // Listed again each time the list opens: devices come and go.
    if response.response.clicked() {
        *devices = list();
    }
    *choice != before
}
