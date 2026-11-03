//! Advanced Output window, laid out like Arena's: screens and slices on the
//! left, the selected screen and slice settings below them, and an editor
//! with two views, the input selection on the composition and the output
//! transformation on the screen (see `crate::ao_editor`).
//!
//! When live, every screen is rendered (see `crate::outputs`): display
//! screens go full screen on their monitor, NDI screens publish a source.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui;
use verso_core::output::{DeviceKind, InputSource, OutputSetup, Screen, Slice, rect};
use verso_io::resolume_ao;
use verso_render::Pattern;

use crate::app::VersoApp;
use crate::dialogs::FileRequest;
use crate::outputs::Monitor;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EditorView {
    /// Slices on the composition: what each one cuts.
    #[default]
    Input,
    /// Slices on the screen: where each one goes.
    Output,
}

#[derive(Default)]
pub struct AdvancedOutput {
    pub window_open: bool,
    /// Screens are rendered and sent to their devices.
    pub live: bool,
    pub setup: OutputSetup,
    /// Preset name, written into exported files.
    name: String,
    /// Text of the imported preset: export patches it, keeping what Verso
    /// does not model.
    source: Option<String>,
    warnings: Vec<String>,
    pub(crate) screen: Option<u64>,
    pub(crate) slice: Option<u64>,
    pub(crate) view: EditorView,
    /// Show and move the points of the warp grid in the output view.
    pub(crate) edit_grid: bool,
    /// Tint the selected slice on the live screens, to find it on the
    /// projectors.
    pub(crate) highlight: bool,
    /// Test pattern shown instead of the slices, by screen id.
    pub(crate) patterns: HashMap<u64, Pattern>,
    /// Every screen shows its number until then.
    identify_until: Option<Instant>,
    pub(crate) drag: Option<crate::ao_editor::Drag>,
    /// Points of a polygon being drawn, in the space of the current view.
    pub(crate) drawing: Option<Vec<[f32; 2]>>,
    /// Monitors found the last time the list was refreshed.
    pub(crate) monitors: Option<Vec<Monitor>>,
}

/// How long Identify shows the screen numbers.
const IDENTIFY: Duration = Duration::from_secs(5);

impl AdvancedOutput {
    /// Patterns the screens show this frame: Identify on every screen while
    /// it lasts, otherwise the chosen test patterns.
    pub(crate) fn active_patterns(&self) -> Vec<(u64, Pattern)> {
        if self.identify_until.is_some_and(|t| Instant::now() < t) {
            return (self.setup.screens.iter().enumerate())
                .map(|(i, s)| {
                    (
                        s.id,
                        Pattern::Identify {
                            number: i as u32 + 1,
                        },
                    )
                })
                .collect();
        }
        self.setup
            .screens
            .iter()
            .filter_map(|s| Some((s.id, *self.patterns.get(&s.id)?)))
            .collect()
    }

    /// Slice to tint on the live screens, when Highlight Selection is on.
    pub(crate) fn highlighted_slice(&self) -> Option<u64> {
        self.slice.filter(|_| self.highlight && self.window_open)
    }

    pub(crate) fn selected_screen(&self) -> Option<&Screen> {
        let id = self.screen?;
        self.setup.screens.iter().find(|s| s.id == id)
    }

    pub(crate) fn selected_screen_mut(&mut self) -> Option<&mut Screen> {
        let id = self.screen?;
        self.setup.screens.iter_mut().find(|s| s.id == id)
    }

    /// Selects the first screen when the selection is gone.
    fn fix_selection(&mut self) {
        if self.selected_screen().is_none() {
            self.screen = self.setup.screens.first().map(|s| s.id);
        }
        let slice = self.slice;
        let found = self
            .selected_screen()
            .is_some_and(|s| s.slices.iter().any(|l| Some(l.id) == slice));
        if !found {
            self.slice = None;
        }
    }

    /// Adds `slice` to the selected screen, creating a screen if none is,
    /// and selects it.
    pub(crate) fn add_slice(&mut self, slice: Slice) {
        if self.selected_screen().is_none() {
            self.add_screen();
        }
        let id = slice.id;
        if let Some(screen) = self.selected_screen_mut() {
            screen.slices.insert(0, slice);
            self.slice = Some(id);
        }
    }

    fn add_screen(&mut self) {
        let size = self.setup.canvas;
        let name = format!("Screen {}", self.setup.screens.len() + 1);
        let id = self.setup.add_screen(&name, size);
        self.screen = Some(id);
        self.slice = None;
    }

    /// Size of the selected screen, in pixels.
    pub(crate) fn screen_size(&self) -> (u32, u32) {
        self.selected_screen()
            .map_or(self.setup.canvas, |s| s.size(self.setup.canvas))
    }
}

impl VersoApp {
    pub(crate) fn advanced_output_window(&mut self, ctx: &egui::Context) {
        let mut open = self.advanced.window_open;
        egui::Window::new("Advanced Output")
            .open(&mut open)
            .default_size([1080.0, 660.0])
            .default_pos([120.0, 80.0])
            .show(ctx, |ui| self.advanced_output_body(ui));
        self.advanced.window_open = open && self.advanced.window_open;
    }

    fn advanced_output_body(&mut self, ui: &mut egui::Ui) {
        if self.advanced.setup.screens.is_empty() {
            self.advanced.setup.canvas = self.renderer.size();
        }
        self.advanced.fix_selection();
        self.advanced_toolbar(ui);
        ui.separator();
        egui::Panel::left("ao side")
            .resizable(true)
            .default_size(280.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    self.screen_list(ui);
                    ui.separator();
                    self.screen_settings(ui);
                    ui.separator();
                    self.slice_settings(ui);
                    for warning in &self.advanced.warnings {
                        ui.label(
                            egui::RichText::new(warning)
                                .small()
                                .color(theme::text_dim()),
                        );
                    }
                });
            });
        egui::CentralPanel::default().show(ui, |ui| self.ao_editor(ui));
    }

    fn advanced_toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Import Resolume Preset…")
                .on_hover_text("Advanced Output preset (.xml) saved by Resolume Arena")
                .clicked()
            {
                self.file_request(FileRequest::AdvancedImport);
            }
            let can_export = !self.advanced.setup.screens.is_empty();
            if ui
                .add_enabled(can_export, egui::Button::new("Export Resolume Preset…"))
                .on_hover_text("Writes a preset Resolume Arena can load")
                .clicked()
            {
                let name = format!("{}.xml", self.advanced_name());
                self.file_request(FileRequest::AdvancedExport(name));
            }
            ui.separator();
            ui.label("Name");
            let mut name = self.advanced_name();
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(140.0))
                .changed()
            {
                self.advanced.name = name;
            }
            ui.separator();
            ui.checkbox(&mut self.advanced.live, "Live").on_hover_text(
                "Render the screens: a display screen goes full screen on its monitor, \
                 an NDI screen publishes a source",
            );
            self.pattern_controls(ui);
            ui.checkbox(&mut self.advanced.highlight, "Highlight Selection")
                .on_hover_text(
                    "Tints the selected slice with stripes on the live screens,                      to find it on the projectors. Off when this window is closed.",
                );
            let setup = &self.advanced.setup;
            ui.label(
                egui::RichText::new(format!(
                    "Composition {}×{} · {} screen(s) · {} slice(s)",
                    setup.canvas.0,
                    setup.canvas.1,
                    setup.screens.len(),
                    setup.slice_count()
                ))
                .small()
                .color(theme::text_dim()),
            );
            ui.separator();
            self.soft_edge_menu(ui);
        });
    }

    /// Test pattern for every screen, Identify, and a warning while a
    /// pattern is on, so it is not left on during a show.
    fn pattern_controls(&mut self, ui: &mut egui::Ui) {
        let ao = &mut self.advanced;
        let live = ao.live;
        let all = ao
            .setup
            .screens
            .iter()
            .map(|s| ao.patterns.get(&s.id))
            .collect::<Vec<_>>();
        let shown = match all.first() {
            Some(first) if all.iter().all(|p| p == first) => first.map_or("Off", |p| p.label()),
            None => "Off",
            _ => "Mixed",
        };
        egui::ComboBox::from_id_salt("ao pattern all")
            .selected_text(format!("Pattern: {shown}"))
            .width(120.0)
            .show_ui(ui, |ui| {
                if ui.selectable_label(shown == "Off", "Off").clicked() {
                    ao.patterns.clear();
                }
                for pattern in Pattern::CHOICES {
                    if ui
                        .selectable_label(shown == pattern.label(), pattern.label())
                        .clicked()
                    {
                        for screen in &ao.setup.screens {
                            ao.patterns.insert(screen.id, pattern);
                        }
                    }
                }
            })
            .response
            .on_hover_text(
                "Test pattern on every screen, instead of the slices: grid for \
                 geometry and focus, colors and gradient to match projectors, \
                 slices to see what goes where. Each screen can have its own.",
            );
        let identifying = ao.identify_until.is_some_and(|t| Instant::now() < t);
        if ui
            .add_enabled(live && !identifying, egui::Button::new("Identify"))
            .on_hover_text("Each screen shows its number, color and resolution for 5 seconds")
            .on_disabled_hover_text("Turn Live on to send the numbers to the screens")
            .clicked()
        {
            ao.identify_until = Some(Instant::now() + IDENTIFY);
        }
        let on = ao
            .setup
            .screens
            .iter()
            .filter(|s| ao.patterns.contains_key(&s.id))
            .count();
        if on > 0 {
            ui.label(
                egui::RichText::new(format!("Test pattern on {on} screen(s)"))
                    .small()
                    .color(theme::warn()),
            );
        }
    }

    /// Curve shared by every soft-edged slice, and how many there are.
    fn soft_edge_menu(&mut self, ui: &mut egui::Ui) {
        let setup = &mut self.advanced.setup;
        let soft = setup
            .screens
            .iter()
            .flat_map(|s| &s.slices)
            .filter(|s| s.soft_edge)
            .count();
        ui.menu_button(format!("Soft Edge ({soft})"), |ui| {
            ui.label(
                egui::RichText::new(
                    "Slices with Soft Edge on blend where their inputs overlap,                      on any screen. Turn it on in each slice's settings.",
                )
                .small()
                .color(theme::text_dim()),
            );
            let s = &mut setup.soft_edge;
            egui::Grid::new("ao soft edge")
                .num_columns(2)
                .spacing([8.0, 4.0])
                .show(ui, |ui| {
                    ui.label("Power")
                        .on_hover_text("Steepness of the ramp; 1 is linear");
                    ui.add(egui::Slider::new(&mut s.power, 0.0..=8.0));
                    ui.end_row();
                    ui.label("Luminance")
                        .on_hover_text("Light in the middle of the overlap");
                    ui.add(egui::Slider::new(&mut s.luminance, 0.0..=1.0));
                    ui.end_row();
                    ui.label("Gamma")
                        .on_hover_text("Raise if the overlap looks too bright, lower if too dark");
                    ui.add(egui::Slider::new(&mut s.gamma, 0.0..=4.0));
                    ui.end_row();
                    for (channel, label) in ["Gamma Red", "Gamma Green", "Gamma Blue"]
                        .iter()
                        .enumerate()
                    {
                        ui.label(*label);
                        ui.add(egui::Slider::new(&mut s.gamma_rgb[channel], 0.0..=4.0));
                        ui.end_row();
                    }
                });
            if ui.button("Reset Curve").clicked() {
                *s = verso_core::output::SoftEdge::default();
            }
            if soft > verso_core::output::MAX_SOFT_SLICES {
                ui.label(
                    egui::RichText::new(format!(
                        "Only the first {} soft-edged slices are blended",
                        verso_core::output::MAX_SOFT_SLICES
                    ))
                    .small()
                    .color(theme::warn()),
                );
            }
        });
    }

    fn screen_list(&mut self, ui: &mut egui::Ui) {
        let ao = &mut self.advanced;
        ui.horizontal(|ui| {
            ui.strong("Screens");
            if ui
                .small_button("+")
                .on_hover_text("A screen showing the whole composition")
                .clicked()
            {
                ao.add_screen();
            }
        });
        let mut remove = None;
        for (index, screen) in ao.setup.screens.iter_mut().enumerate() {
            let selected = ao.screen == Some(screen.id);
            ui.horizontal(|ui| {
                ui.checkbox(&mut screen.enabled, "")
                    .on_hover_text("Rendered when live");
                if ui.selectable_label(selected, &screen.name).clicked() && !selected {
                    ao.screen = Some(screen.id);
                    ao.slice = None;
                    ao.drawing = None;
                }
                ui.label(
                    egui::RichText::new(screen.device.kind.label())
                        .small()
                        .color(theme::text_dim()),
                );
                if ui
                    .small_button("✕")
                    .on_hover_text("Remove screen")
                    .clicked()
                {
                    remove = Some(index);
                }
            });
            if !selected {
                continue;
            }
            for slice in &mut screen.slices {
                ui.horizontal(|ui| {
                    ui.add_space(18.0);
                    ui.checkbox(&mut slice.enabled, "");
                    let picked = ao.slice == Some(slice.id);
                    if ui.selectable_label(picked, &slice.name).clicked() {
                        ao.slice = Some(slice.id);
                    }
                });
            }
        }
        if let Some(index) = remove {
            ao.setup.screens.remove(index);
        }
    }

    fn screen_settings(&mut self, ui: &mut egui::Ui) {
        let canvas = self.advanced.setup.canvas;
        let live = self.advanced.live;
        let receivers = self
            .advanced
            .screen
            .and_then(|id| self.outputs.screen_receivers(id));
        let ao = &mut self.advanced;
        let monitors = ao.monitors.get_or_insert_with(crate::outputs::monitors);
        let mut refresh = false;
        let Some(screen) = ao
            .screen
            .and_then(|id| ao.setup.screens.iter_mut().find(|s| s.id == id))
        else {
            ui.label(egui::RichText::new("No screen").color(theme::text_dim()));
            return;
        };
        let patterns = &mut ao.patterns;
        egui::Grid::new("ao screen")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Screen");
                ui.add(egui::TextEdit::singleline(&mut screen.name).desired_width(150.0));
                ui.end_row();

                ui.label("Test Pattern");
                let current = patterns.get(&screen.id).copied();
                egui::ComboBox::from_id_salt(("ao pattern", screen.id))
                    .selected_text(current.map_or("Off", |p| p.label()))
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(current.is_none(), "Off").clicked() {
                            patterns.remove(&screen.id);
                        }
                        for pattern in Pattern::CHOICES {
                            if ui
                                .selectable_label(current == Some(pattern), pattern.label())
                                .clicked()
                            {
                                patterns.insert(screen.id, pattern);
                            }
                        }
                    });
                ui.end_row();

                ui.label("Device");
                egui::ComboBox::from_id_salt(("ao device", screen.id))
                    .selected_text(screen.device.kind.label())
                    .show_ui(ui, |ui| {
                        for kind in [
                            DeviceKind::Display,
                            DeviceKind::Ndi,
                            DeviceKind::Virtual,
                            DeviceKind::None,
                        ] {
                            ui.selectable_value(&mut screen.device.kind, kind, kind.label());
                        }
                    });
                ui.end_row();

                match screen.device.kind {
                    DeviceKind::Display => {
                        ui.label("Monitor");
                        ui.horizontal(|ui| {
                            let current = monitors
                                .iter()
                                .find(|m| m.name == screen.device.name)
                                .map_or_else(
                                    // Imported presets name Arena's displays.
                                    || match screen.device.name.strip_prefix("Display ") {
                                        Some(_) => {
                                            format!("{} (not connected)", screen.device.name)
                                        }
                                        None => "Choose a monitor".to_owned(),
                                    },
                                    Monitor::label,
                                );
                            egui::ComboBox::from_id_salt(("ao monitor", screen.id))
                                .selected_text(current)
                                .width(170.0)
                                .show_ui(ui, |ui| {
                                    for monitor in monitors.iter() {
                                        let picked = monitor.name == screen.device.name;
                                        if ui.selectable_label(picked, monitor.label()).clicked() {
                                            screen.device.name = monitor.name.clone();
                                            screen.device.size = Some(monitor.size);
                                        }
                                    }
                                });
                            if ui
                                .small_button("Refresh")
                                .on_hover_text("Look for monitors again")
                                .clicked()
                            {
                                refresh = true;
                            }
                        });
                        ui.end_row();
                    }
                    DeviceKind::Ndi => {
                        ui.label("NDI name");
                        ui.add(
                            egui::TextEdit::singleline(&mut screen.device.name)
                                .desired_width(150.0),
                        );
                        ui.end_row();
                        if live {
                            ui.label("");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} receiver(s)",
                                    receivers.unwrap_or(0)
                                ))
                                .small()
                                .color(theme::text_dim()),
                            );
                            ui.end_row();
                        }
                    }
                    DeviceKind::Spout | DeviceKind::Dmx => {
                        ui.label("");
                        ui.label(
                            egui::RichText::new(format!(
                                "{} output is not supported yet",
                                screen.device.kind.label()
                            ))
                            .small()
                            .color(theme::warn()),
                        );
                        ui.end_row();
                    }
                    _ => {}
                }

                ui.label("Size");
                ui.horizontal(|ui| {
                    let (mut w, mut h) = screen.size(canvas);
                    let fixed = screen.device.kind == DeviceKind::Display
                        && monitors.iter().any(|m| m.name == screen.device.name);
                    let changed = ui
                        .add_enabled(!fixed, egui::DragValue::new(&mut w).range(16..=16384))
                        .changed()
                        | ui.add_enabled(!fixed, egui::DragValue::new(&mut h).range(16..=16384))
                            .changed();
                    if changed {
                        screen.device.size = Some((w, h));
                    }
                });
                ui.end_row();

                ui.label("Opacity");
                ui.add(egui::Slider::new(&mut screen.color.opacity, 0.0..=1.0));
                ui.end_row();
                ui.label("Brightness");
                ui.add(egui::Slider::new(&mut screen.color.brightness, -1.0..=1.0));
                ui.end_row();
                ui.label("Contrast");
                ui.add(egui::Slider::new(&mut screen.color.contrast, -1.0..=1.0));
                ui.end_row();
            });
        if refresh {
            ao.monitors = Some(crate::outputs::monitors());
        }
    }

    fn slice_settings(&mut self, ui: &mut egui::Ui) {
        let layers: Vec<String> = self.comp.layers.iter().map(|l| l.name.clone()).collect();
        let groups: Vec<String> = self.comp.groups.iter().map(|g| g.name.clone()).collect();
        let ao = &mut self.advanced;
        let Some(slice_id) = ao.slice else {
            ui.label(
                egui::RichText::new("Click a slice in the editor to edit it")
                    .small()
                    .color(theme::text_dim()),
            );
            return;
        };
        let Some(screen) = ao.selected_screen_mut() else {
            return;
        };
        let Some(index) = screen.slices.iter().position(|s| s.id == slice_id) else {
            return;
        };
        let mut remove = false;
        let slice = &mut screen.slices[index];
        egui::Grid::new("ao slice")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label("Slice");
                ui.add(egui::TextEdit::singleline(&mut slice.name).desired_width(150.0));
                ui.end_row();

                ui.label("Source");
                let name = |source: InputSource| match source {
                    InputSource::Composition => "Composition".to_owned(),
                    InputSource::Layer(i) => format!(
                        "Layer {}{}",
                        i + 1,
                        layers.get(i).map(|n| format!(": {n}")).unwrap_or_default()
                    ),
                    InputSource::Group(g) => format!(
                        "Group {}{}",
                        g + 1,
                        groups.get(g).map(|n| format!(": {n}")).unwrap_or_default()
                    ),
                };
                let current = slice.input_source();
                egui::ComboBox::from_id_salt(("ao source", slice.id))
                    .selected_text(name(current))
                    .width(170.0)
                    .show_ui(ui, |ui| {
                        let choices = std::iter::once(InputSource::Composition)
                            .chain((0..layers.len()).rev().map(InputSource::Layer))
                            .chain((0..groups.len()).map(InputSource::Group));
                        for source in choices {
                            if ui
                                .selectable_label(source == current, name(source))
                                .clicked()
                            {
                                slice.source = source.code();
                            }
                        }
                    });
                ui.end_row();

                ui.label("Flip");
                ui.horizontal(|ui| {
                    let mut h = slice.flip & 1 != 0;
                    let mut v = slice.flip & 2 != 0;
                    let changed = ui.checkbox(&mut h, "Horizontal").changed()
                        | ui.checkbox(&mut v, "Vertical").changed();
                    if changed {
                        slice.flip = u8::from(h) | (u8::from(v) << 1);
                    }
                });
                ui.end_row();

                ui.label("Soft Edge");
                ui.checkbox(&mut slice.soft_edge, "")
                    .on_hover_text("Blend with other soft-edged slices where the inputs overlap");
                ui.end_row();

                ui.label("Brightness");
                ui.add(egui::Slider::new(&mut slice.color.brightness, -1.0..=1.0));
                ui.end_row();
                ui.label("Contrast");
                ui.add(egui::Slider::new(&mut slice.color.contrast, -1.0..=1.0));
                ui.end_row();
                for (channel, label) in ["Red", "Green", "Blue"].iter().enumerate() {
                    ui.label(*label);
                    ui.add(egui::Slider::new(&mut slice.color.rgb[channel], -1.0..=1.0));
                    ui.end_row();
                }
            });
        ui.horizontal(|ui| {
            if let Some(warp) = &mut slice.warp
                && ui
                    .button("Reset Warp")
                    .on_hover_text("Corners and grid back to the output rectangle")
                    .clicked()
            {
                *warp = verso_core::output::Warp::identity(
                    slice.output,
                    warp.grid.0.max(2),
                    warp.grid.1.max(2),
                );
            }
            if ui.button("Delete Slice").clicked() {
                remove = true;
            }
        });
        if remove {
            screen.slices.remove(index);
            ao.slice = None;
        }
    }

    /// A new slice showing the whole composition on the whole screen.
    pub(crate) fn add_full_slice(&mut self) {
        let ao = &mut self.advanced;
        let (cw, ch) = ao.setup.canvas;
        let (sw, sh) = ao.screen_size();
        let id = ao.setup.next_id();
        let count = ao.selected_screen().map_or(0, |s| s.slices.len());
        ao.add_slice(Slice::new(
            id,
            &format!("Slice {}", count + 1),
            rect(cw as f32, ch as f32),
            rect(sw as f32, sh as f32),
        ));
    }

    fn advanced_name(&self) -> String {
        if self.advanced.name.is_empty() {
            self.comp.name.clone()
        } else {
            self.advanced.name.clone()
        }
    }

    pub(crate) fn import_advanced_output(&mut self, path: &Path) {
        let result = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| {
                resolume_ao::import(&text)
                    .map(|import| (import, text))
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok((import, text)) => {
                let setup = &import.setup;
                self.status = format!(
                    "Imported {}: {} screen(s), {} slice(s)",
                    path.display(),
                    setup.screens.len(),
                    setup.slice_count()
                );
                for warning in &import.warnings {
                    log::warn!("{}: {warning}", path.display());
                }
                let dmx = setup
                    .screens
                    .iter()
                    .filter(|s| s.device.kind == DeviceKind::Dmx)
                    .count();
                log::info!("{}, {dmx} DMX screen(s)", self.status);
                self.advanced.name = import.name;
                self.advanced.setup = import.setup;
                self.advanced.warnings = import.warnings;
                self.advanced.source = Some(text);
                self.advanced.screen = None;
                self.advanced.slice = None;
                self.advanced.drawing = None;
                self.advanced.window_open = true;
            }
            Err(e) => self.status = format!("Import failed: {e}"),
        }
    }

    pub(crate) fn export_advanced_output(&mut self, path: &Path) {
        let name = self.advanced_name();
        let result =
            resolume_ao::export(&name, &self.advanced.setup, self.advanced.source.as_deref())
                .map_err(|e| e.to_string())
                .and_then(|text| std::fs::write(path, text).map_err(|e| e.to_string()));
        self.status = match result {
            Ok(()) => format!("Exported {}", path.display()),
            Err(e) => format!("Export failed: {e}"),
        };
    }

    /// Adds the shapes of an SVG file as polygon slices of the selected
    /// screen, the drawing stretched over the composition and the screen.
    pub(crate) fn import_svg(&mut self, path: &Path) {
        let drawing = std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|text| verso_io::svg::read(&text).map_err(|e| e.to_string()));
        let drawing = match drawing {
            Ok(drawing) => drawing,
            Err(e) => {
                self.status = format!("SVG import failed: {e}");
                return;
            }
        };
        let ao = &mut self.advanced;
        let (cw, ch) = ao.setup.canvas;
        let (sw, sh) = ao.screen_size();
        let (dw, dh) = (drawing.size.0.max(1.0), drawing.size.1.max(1.0));
        let scale = |points: &[[f32; 2]], w: u32, h: u32| -> Vec<[f32; 2]> {
            points
                .iter()
                .map(|p| [p[0] / dw * w as f32, p[1] / dh * h as f32])
                .collect()
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "SVG".into());
        let count = drawing.polygons.len();
        for (i, polygon) in drawing.polygons.iter().enumerate().rev() {
            let id = ao.setup.next_id();
            ao.add_slice(Slice::polygon(
                id,
                &format!("{stem} {}", i + 1),
                scale(polygon, cw, ch),
                scale(polygon, sw, sh),
            ));
        }
        self.status = format!("Imported {count} shape(s) from {}", path.display());
    }
}

/// Arena's preset folder, when it exists.
pub(crate) fn resolume_preset_dir() -> Option<PathBuf> {
    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })?;
    let dir = PathBuf::from(home).join("Documents/Resolume Arena/Presets/Advanced Output");
    dir.is_dir().then_some(dir)
}
