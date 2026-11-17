//! Lower panels: properties, preview and output monitors, source browser.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, TextureId, Vec2};
use verso_core::{Command, FaderSide, Generator, GeneratorParams, Source};

use crate::app::{PropsTab, VersoApp};
use crate::blend_ui::{BlendOwner, blend_mode_combo};
use crate::grid::SourceTemplate;
use crate::{theme, widgets};

/// Draws `texture` as large as fits in `size`, letterboxed on black. Returns
/// the monitor rectangle.
fn monitor_image(ui: &mut egui::Ui, texture: Option<TextureId>, size: Vec2, aspect: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    if let Some(texture) = texture {
        painter.image(
            texture,
            fitted(rect, aspect),
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    painter.rect_stroke(
        rect,
        0.0,
        Stroke::new(1.0, theme::line()),
        StrokeKind::Inside,
    );
    rect
}

/// The part of `rect` an image of `aspect` fills, letterboxed.
fn fitted(rect: Rect, aspect: f32) -> Rect {
    let fit = if rect.width() / rect.height() > aspect {
        Vec2::new(rect.height() * aspect, rect.height())
    } else {
        Vec2::new(rect.width(), rect.width() / aspect)
    };
    Rect::from_center_size(rect.center(), fit)
}

/// `m:ss` for a duration in seconds.
pub(crate) fn clock(seconds: f64) -> String {
    let whole = seconds.max(0.0) as u64;
    format!("{}:{:02}", whole / 60, whole % 60)
}

/// Thin playback progress line along the bottom edge of `rect`.
pub(crate) fn progress_strip(painter: &egui::Painter, rect: Rect, progress: f32) {
    let strip = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - 3.0),
        rect.right_bottom(),
    );
    painter.rect_filled(strip, 0.0, Color32::from_black_alpha(120));
    let done = Rect::from_min_max(
        strip.min,
        egui::pos2(
            strip.left() + strip.width() * progress.clamp(0.0, 1.0),
            strip.bottom(),
        ),
    );
    painter.rect_filled(done, 0.0, theme::accent());
}

fn property_grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    // Labels fit the minimum column width; sliders take the rest.
    widgets::fit_sliders(ui, 90.0, 10.0);
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .min_col_width(90.0)
        .show(ui, add);
}

impl VersoApp {
    pub(crate) fn composition_properties(&mut self, ui: &mut egui::Ui) {
        property_grid(ui, "composition", |ui| {
            ui.label("Name");
            let mut name = self.comp.name.clone();
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::Rename(name));
            }
            ui.end_row();

            ui.label("Resolution");
            // Applied when the drag or the typing ends: each change rebuilds
            // every render texture.
            let (mut width, mut height) = self
                .edit_size
                .unwrap_or((self.comp.width, self.comp.height));
            let done = ui
                .horizontal(|ui| {
                    let w = ui.add(egui::DragValue::new(&mut width).range(16..=8192));
                    ui.label("×");
                    let h = ui.add(egui::DragValue::new(&mut height).range(16..=8192));
                    if w.changed() || h.changed() {
                        self.edit_size = Some((width, height));
                    }
                    let editing = w.dragged() || h.dragged() || w.has_focus() || h.has_focus();
                    self.edit_size.is_some() && !editing
                })
                .inner;
            if done && let Some((width, height)) = self.edit_size.take() {
                self.send(Command::SetResolution { width, height });
            }
            ui.end_row();

            ui.label("Master");
            let mut opacity = self.comp.master.opacity;
            if widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(200.0, 18.0),
                theme::accent(),
                1.0,
            )
            .changed()
            {
                self.send(Command::SetMasterOpacity(opacity));
            }
            ui.end_row();

            ui.label("Crossfader");
            let mut crossfader = self.comp.master.crossfader;
            if ui
                .add(egui::Slider::new(&mut crossfader, 0.0..=1.0).show_value(false))
                .changed()
            {
                self.send(Command::SetCrossfader(crossfader));
            }
            ui.end_row();

            ui.label("Tempo");
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
            ui.end_row();
        });
    }

    pub(crate) fn group_properties(&mut self, ui: &mut egui::Ui, g: usize) {
        if g >= self.comp.groups.len() {
            ui.label(egui::RichText::new("This group no longer exists.").color(theme::text_dim()));
            return;
        }
        let group = &self.comp.groups[g];
        let blend_params = group.blend_params.clone();
        let (mut name, blend, mut opacity, mut bypass) =
            (group.name.clone(), group.blend, group.opacity, group.bypass);
        property_grid(ui, "group", |ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameGroup { group: g, name });
            }
            ui.end_row();

            ui.label("Blend Mode");
            if let Some(blend) = blend_mode_combo(ui, "group blend", blend) {
                self.send(Command::SetGroupBlend { group: g, blend });
            }
            ui.end_row();
            self.blend_param_rows(ui, BlendOwner::Group(g), blend, &blend_params);

            ui.label("Opacity");
            if widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(200.0, 18.0),
                theme::accent(),
                1.0,
            )
            .changed()
            {
                self.send(Command::SetGroupOpacity { group: g, opacity });
            }
            ui.end_row();

            ui.label("State");
            ui.horizontal(|ui| {
                if widgets::toggle(ui, &mut bypass, "B", theme::warn(), "Bypass").changed() {
                    self.send(Command::SetGroupBypass { group: g, bypass });
                }
                let mut solo = self.comp.groups[g].solo;
                if widgets::toggle(ui, &mut solo, "S", theme::solo(), "Solo").changed() {
                    self.send(Command::SetGroupSolo { group: g, solo });
                }
            });
            ui.end_row();

            ui.label("Inside");
            let parent = self.comp.groups[g]
                .parent
                .and_then(|p| self.comp.groups.get(p))
                .map(|p| p.name.clone())
                .unwrap_or_else(|| "Composition".into());
            ui.label(parent);
            ui.end_row();

            ui.label("Layers");
            let members: Vec<_> = self
                .comp
                .group_layers(g)
                .map(|l| self.comp.layers[l].name.clone())
                .collect();
            ui.label(members.join(", "));
            ui.end_row();
        });
        if ui
            .button("Delete Group")
            .on_hover_text("Its layers stay, ungrouped")
            .clicked()
        {
            self.send(Command::RemoveGroup(g));
            self.props_tab = PropsTab::Layer;
        }
    }

    pub(crate) fn layer_properties(&mut self, ui: &mut egui::Ui, index: usize) {
        let Some(layer) = self.comp.layers.get(index) else {
            ui.label("No layer.");
            return;
        };
        let blend_params = layer.blend_params.clone();
        let (mut name, blend, mut opacity, mut bypass, mut solo, mut side) = (
            layer.name.clone(),
            layer.blend,
            layer.opacity,
            layer.bypass,
            layer.solo,
            layer.fader,
        );
        property_grid(ui, "layer", |ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameLayer { layer: index, name });
            }
            ui.end_row();

            ui.label("Blend Mode");
            if let Some(blend) = blend_mode_combo(ui, "layer blend", blend) {
                self.send(Command::SetLayerBlend {
                    layer: index,
                    blend,
                });
            }
            ui.end_row();
            self.blend_param_rows(ui, BlendOwner::Layer(index), blend, &blend_params);

            ui.label("Opacity");
            if widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(200.0, 18.0),
                theme::accent(),
                1.0,
            )
            .changed()
            {
                self.send(Command::SetLayerOpacity {
                    layer: index,
                    opacity,
                });
            }
            ui.end_row();

            ui.label("State");
            ui.horizontal(|ui| {
                if widgets::toggle(ui, &mut bypass, "B", theme::warn(), "Bypass").changed() {
                    self.send(Command::SetLayerBypass {
                        layer: index,
                        bypass,
                    });
                }
                if widgets::toggle(ui, &mut solo, "S", theme::solo(), "Solo").changed() {
                    self.send(Command::SetLayerSolo { layer: index, solo });
                }
            });
            ui.end_row();

            ui.label("Crossfader");
            ui.horizontal(|ui| {
                for (value, label) in [
                    (FaderSide::None, "Off"),
                    (FaderSide::A, "A"),
                    (FaderSide::B, "B"),
                ] {
                    ui.selectable_value(&mut side, value, label);
                }
            });
            if side != self.comp.layers[index].fader {
                self.send(Command::SetLayerFader { layer: index, side });
            }
            ui.end_row();

            ui.label("Transition");
            let mut seconds = self.comp.layers[index].transition;
            if ui
                .add(
                    egui::Slider::new(&mut seconds, 0.0..=10.0)
                        .suffix(" s")
                        .logarithmic(true)
                        .smallest_positive(0.05),
                )
                .on_hover_text("Crossfade time when the playing clip changes. 0 cuts.")
                .changed()
            {
                self.send(Command::SetLayerTransition {
                    layer: index,
                    seconds,
                });
            }
            ui.end_row();

            ui.label("Group");
            let current = self.comp.layers[index].group;
            let group_name = |g: Option<usize>| match g {
                Some(g) => self.comp.groups[g].name.clone(),
                None => "None".to_owned(),
            };
            let mut chosen = current;
            let mut new_group = false;
            egui::ComboBox::from_id_salt("layer group")
                .selected_text(group_name(current))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "None");
                    for g in 0..self.comp.groups.len() {
                        ui.selectable_value(&mut chosen, Some(g), group_name(Some(g)));
                    }
                    new_group = ui.selectable_label(false, "+ New Group").clicked();
                });
            if new_group {
                self.send(Command::AddGroup {
                    layers: vec![index],
                });
            } else if chosen != current {
                self.send(Command::SetLayerGroup {
                    layer: index,
                    group: chosen,
                });
            }
            ui.end_row();
        });
    }

    pub(crate) fn clip_properties(&mut self, ui: &mut egui::Ui, slot: Option<(usize, usize)>) {
        let Some((layer, column)) = slot else {
            ui.label(egui::RichText::new("Select a clip.").color(theme::text_dim()));
            return;
        };
        let Some(Some(mut clip)) = self
            .comp
            .layers
            .get(layer)
            .and_then(|l| l.clips.get(column))
            .cloned()
        else {
            ui.label(
                egui::RichText::new("Empty slot. Drag a source here from the browser.")
                    .color(theme::text_dim()),
            );
            return;
        };
        let before = clip.clone();
        let transport = self.media.transport(layer, column);
        property_grid(ui, "clip", |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut clip.name);
            ui.end_row();

            ui.label("Slot");
            ui.label(format!(
                "{} · {}",
                self.comp.layers[layer].name, self.comp.columns[column].name
            ));
            ui.end_row();

            match &mut clip.source {
                Source::Solid { color } => {
                    ui.label("Source");
                    ui.label("Solid Color");
                    ui.end_row();
                    ui.label("Color");
                    widgets::color_edit(ui, color);
                    ui.end_row();
                }
                Source::Generator {
                    kind,
                    color,
                    speed,
                    params,
                } => {
                    ui.label("Source");
                    let previous = *kind;
                    egui::ComboBox::from_id_salt("clip generator")
                        .selected_text(kind.name())
                        .show_ui(ui, |ui| {
                            for g in Generator::ALL {
                                ui.selectable_value(kind, g, g.name());
                            }
                        });
                    if *kind != previous {
                        // The specific values meant something else.
                        params.shape.clear();
                    }
                    ui.end_row();
                    ui.label("Color");
                    widgets::color_edit(ui, color);
                    ui.end_row();
                    ui.label("Speed");
                    ui.add(egui::Slider::new(speed, 0.0..=4.0));
                    ui.end_row();
                    generator_params(ui, *kind, params);
                }
                Source::Milkdrop {
                    family,
                    variant,
                    blend,
                    pattern,
                    input,
                    input_mix,
                } => {
                    let family = family.clone();
                    if let Some(picked) = self.milkdrop_props(ui, &family, *variant) {
                        *variant = picked;
                    }
                    self.milkdrop_playback(ui, layer, blend, pattern, input, input_mix);
                }
                Source::Shader(effect) => {
                    ui.label("Source");
                    ui.label(&effect.name).on_hover_text(&effect.id);
                    ui.end_row();
                    match self.renderer.library().get(&effect.id).cloned() {
                        Some(def) => {
                            if let Some(credit) = &def.credit {
                                ui.label("Credit");
                                ui.label(
                                    egui::RichText::new(credit).small().color(theme::text_dim()),
                                );
                                ui.end_row();
                            }
                            for param in &def.params {
                                ui.label(&param.name);
                                let value = def.value(effect, param);
                                if let (_, Some(value)) =
                                    crate::effects_ui::param_widget(ui, &param.kind, value)
                                {
                                    effect.params.insert(param.name.clone(), value);
                                }
                                ui.end_row();
                            }
                        }
                        None => {
                            ui.label("");
                            ui.label(egui::RichText::new("Shader not found").color(theme::warn()));
                            ui.end_row();
                        }
                    }
                }
                Source::Image { path } => {
                    ui.label("Source");
                    ui.label("Image");
                    ui.end_row();
                    file_row(ui, path);
                }
                Source::Text { text, style } => {
                    ui.label("Source");
                    ui.label("Text");
                    ui.end_row();
                    ui.label("Text");
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .desired_rows(2)
                            .desired_width(f32::INFINITY),
                    );
                    ui.end_row();
                    text_style_rows(
                        ui,
                        style,
                        self.media.font_families(),
                        self.media.fonts_loading(),
                    );
                }
                Source::Time {
                    mode,
                    format,
                    style,
                } => {
                    use verso_core::text::{TimeFormat, TimeMode};
                    ui.label("Source");
                    ui.label("Time");
                    ui.end_row();
                    ui.label("Shows");
                    ui.horizontal(|ui| {
                        let countdown = match *mode {
                            TimeMode::CountDown { seconds } => seconds,
                            _ => 60.0,
                        };
                        for choice in [
                            TimeMode::Clock,
                            TimeMode::CountUp,
                            TimeMode::CountDown { seconds: countdown },
                        ] {
                            let same =
                                std::mem::discriminant(mode) == std::mem::discriminant(&choice);
                            if ui.selectable_label(same, choice.name()).clicked() && !same {
                                *mode = choice;
                            }
                        }
                    })
                    .response
                    .on_hover_text("Timers start when the clip is launched");
                    ui.end_row();
                    if let TimeMode::CountDown { seconds } = mode {
                        ui.label("From");
                        ui.add(
                            egui::DragValue::new(seconds)
                                .range(1.0..=86_400.0)
                                .speed(1.0)
                                .custom_formatter(|v, _| clock(v))
                                .custom_parser(parse_clock),
                        );
                        ui.end_row();
                    }
                    ui.label("Format");
                    egui::ComboBox::from_id_salt("time format")
                        .selected_text(format.name())
                        .show_ui(ui, |ui| {
                            for f in TimeFormat::ALL {
                                ui.selectable_value(format, f, f.name());
                            }
                        });
                    ui.end_row();
                    text_style_rows(
                        ui,
                        style,
                        self.media.font_families(),
                        self.media.fonts_loading(),
                    );
                }
                Source::Ndi { name } => {
                    ui.label("Source");
                    ui.label("NDI® input");
                    ui.end_row();
                    ui.label("Sender");
                    ui.label(name.as_str());
                    ui.end_row();
                    ui.label("");
                    let state = self.media.live_state(layer, column);
                    ui.label(egui::RichText::new(state).small().color(theme::text_dim()));
                    ui.end_row();
                }
                Source::Spout { name } => {
                    ui.label("Source");
                    ui.label("Spout® input");
                    ui.end_row();
                    ui.label("Sender");
                    ui.label(name.as_str());
                    ui.end_row();
                    ui.label("");
                    let state = self.media.live_state(layer, column);
                    ui.label(egui::RichText::new(state).small().color(theme::text_dim()));
                    ui.end_row();
                }
                Source::Video {
                    path,
                    speed,
                    looping,
                } => {
                    // Transport first, as in Resolume's clip panel.
                    let playback = self.media.playback(layer, column);
                    let reversible = playback.is_some_and(|(_, _, r)| r);
                    ui.label(egui::RichText::new("Transport").strong());
                    ui.label("");
                    ui.end_row();
                    if let Some((position, Some(duration), _)) = playback {
                        ui.label("Timeline");
                        let mut at = position;
                        let response = ui.add(
                            egui::Slider::new(&mut at, 0.0..=duration)
                                .custom_formatter(|v, _| format!("{v:.2} s")),
                        );
                        if response.changed()
                            && let Some(error) = self.media.seek(layer, column, at)
                        {
                            self.status = error;
                        }
                        ui.end_row();
                    }
                    ui.label("Speed");
                    let range = if reversible { -4.0..=4.0 } else { 0.0..=4.0 };
                    ui.horizontal(|ui| {
                        ui.add(egui::Slider::new(speed, range).suffix("×"))
                            .on_hover_text(if reversible {
                                "HAP plays natively: negative speeds play backwards"
                            } else {
                                "Backwards playback needs a HAP file"
                            });
                        if reversible && ui.small_button("⇄").on_hover_text("Reverse").clicked() {
                            *speed = -*speed;
                        }
                        if ui
                            .small_button("1×")
                            .on_hover_text("Normal speed")
                            .clicked()
                        {
                            *speed = if *speed < 0.0 { -1.0 } else { 1.0 };
                        }
                    });
                    ui.end_row();
                    ui.label("Play Mode");
                    ui.horizontal(|ui| {
                        ui.selectable_value(looping, true, "Loop");
                        ui.selectable_value(looping, false, "Play Once");
                    });
                    ui.end_row();
                    ui.label("Time");
                    match transport {
                        Some(t) => {
                            let text = match t.duration {
                                Some(d) => format!("{} / {}", clock(t.position), clock(d)),
                                None => clock(t.position),
                            };
                            let bar = egui::ProgressBar::new(t.progress().unwrap_or(0.0))
                                .desired_height(14.0)
                                .text(egui::RichText::new(text).monospace().size(10.0));
                            ui.add(bar).on_hover_text(format!(
                                "Frame {}{}",
                                t.frame,
                                if t.ended { ", ended" } else { "" }
                            ));
                        }
                        None => {
                            ui.label(egui::RichText::new("Not playing").color(theme::text_dim()));
                        }
                    }
                    ui.end_row();
                    ui.label(egui::RichText::new("Source").strong());
                    ui.label("Video");
                    ui.end_row();
                    file_row(ui, path);
                }
            }
        });
        if clip != before {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(clip),
            });
        }
    }

    /// The selected clip, letterboxed, with its launch counter.
    pub(crate) fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let comp_aspect = self.comp.width as f32 / self.comp.height.max(1) as f32;
        // A hovered layer or column header shows what it would output.
        if let Some((texture, caption)) = self.hover_preview() {
            let size = (ui.available_size() - Vec2::new(0.0, 18.0)).max(Vec2::splat(40.0));
            monitor_image(ui, Some(texture), size, comp_aspect);
            ui.label(
                egui::RichText::new(caption)
                    .small()
                    .color(theme::text_dim()),
            );
            return;
        }
        // Media clips show their own frame at its own aspect.
        let previewed = self.preview_slot();
        let (clip_texture, clip_aspect) = match previewed {
            Some((layer, column)) => {
                let procedural = self.comp.layers[layer].clips[column]
                    .as_ref()
                    .is_none_or(|clip| clip.source.is_procedural());
                let milk = match self.comp.layers[layer].clips[column]
                    .as_ref()
                    .map(|c| &c.source)
                {
                    Some(Source::Milkdrop { family, .. }) => Some(family.clone()),
                    _ => None,
                };
                if let Some(family) = milk {
                    (self.milk_preview(ui.ctx(), &family), 16.0 / 9.0)
                } else if procedural || self.monitor_effects == Some((layer, column)) {
                    // Procedural clips and clips with effects render into
                    // the monitor texture.
                    (self.monitor_texture, comp_aspect)
                } else {
                    match self.media.clip_texture(&self.comp, layer, column) {
                        Some((id, (w, h))) => (Some(id), w as f32 / h.max(1) as f32),
                        None => (None, comp_aspect),
                    }
                }
            }
            None => (None, comp_aspect),
        };
        let size = ui.available_size().max(Vec2::splat(40.0));
        let monitor = monitor_image(ui, clip_texture, size, clip_aspect);
        self.track_mouse(ui, monitor, clip_aspect, "preview mouse");
        if let Some((layer, column)) = previewed {
            let launches = self.comp.layers[layer].launch_count(column);
            let painter = ui.painter_at(monitor);
            if let Some(progress) = self
                .media
                .transport(layer, column)
                .and_then(|t| t.progress())
            {
                progress_strip(&painter, monitor, progress);
            }
            let galley = painter.layout_no_wrap(
                format!("× {launches}"),
                egui::FontId::monospace(12.0),
                theme::bg_deep(),
            );
            let badge = Rect::from_min_size(
                monitor.right_bottom() - galley.size() - Vec2::new(14.0, 10.0),
                galley.size() + Vec2::new(8.0, 4.0),
            );
            painter.rect_filled(badge, 3.0, theme::accent());
            painter.galley(badge.min + Vec2::new(4.0, 2.0), galley, theme::bg_deep());
        }
    }

    /// The composition output.
    pub(crate) fn output_panel(&mut self, ui: &mut egui::Ui) {
        let aspect = self.comp.width as f32 / self.comp.height.max(1) as f32;
        let size = ui.available_size().max(Vec2::splat(40.0));
        let monitor = monitor_image(ui, Some(self.output_texture), size, aspect);
        self.track_mouse(ui, monitor, aspect, "output mouse");
    }

    /// Makes the image in `monitor` a mouse control area.
    fn track_mouse(&mut self, ui: &mut egui::Ui, monitor: Rect, aspect: f32, id: &str) {
        let response = ui.interact(monitor, ui.make_persistent_id(id), Sense::click_and_drag());
        self.controls
            .mouse
            .track(&response, fitted(monitor, aspect));
    }

    pub(crate) fn browser(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "Browser");
        widgets::tabs(
            ui,
            &mut self.files.tab,
            &[
                (BrowserTab::Sources, "Sources"),
                (BrowserTab::Milkdrop, "MilkDrop"),
                (BrowserTab::Effects, "Effects"),
                (BrowserTab::Files, "Library"),
                (BrowserTab::Online, "Online"),
                (BrowserTab::YouTube, "YouTube"),
                (BrowserTab::Favorites, "★ Favorites"),
            ],
        );
        ui.separator();
        match self.files.tab {
            BrowserTab::Favorites => self.favorites_browser(ui),
            BrowserTab::Sources => {
                hint(
                    ui,
                    "Drag onto a clip slot, or double-click to load into the selected slot. Hover to preview.",
                );
                self.browser_toolbar(ui, true);
                let shaders: Vec<SourceTemplate> = self
                    .renderer
                    .library()
                    .sorted_kind(verso_render::ShaderKind::Generator)
                    .into_iter()
                    .map(|def| SourceTemplate {
                        name: def.name.clone(),
                        source: Source::Shader(verso_core::Effect::new(&def.id, &def.name)),
                    })
                    .collect();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let builtin = SourceTemplate::all()
                        .into_iter()
                        .map(|t| (t, String::new()))
                        .collect();
                    self.browser_items(ui, builtin);
                    if !shaders.is_empty() {
                        ui.add_space(6.0);
                        hint(ui, "Shaders");
                        let shaders = shaders.into_iter().map(|t| (t, String::new())).collect();
                        self.browser_items(ui, shaders);
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        hint(ui, "NDI® sources");
                        let searching = self.media.ndi_searching();
                        if ui
                            .add_enabled(
                                !searching,
                                egui::Button::new(if searching { "Searching…" } else { "Find" })
                                    .small(),
                            )
                            .on_hover_text(
                                "Look for NDI senders on the network (needs NDI Tools or the NDI runtime)",
                            )
                            .clicked()
                        {
                            self.media.search_ndi();
                        }
                    });
                    let sources = self
                        .media
                        .ndi_sources()
                        .iter()
                        .map(|name| {
                            let template = SourceTemplate {
                                name: name.clone(),
                                source: Source::Ndi { name: name.clone() },
                            };
                            (template, String::new())
                        })
                        .collect();
                    self.browser_items(ui, sources);
                    if crate::media::SPOUT_INPUT {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            hint(ui, "Spout® sources");
                            if ui
                                .small_button("Find")
                                .on_hover_text("List the Spout senders running on this computer")
                                .clicked()
                            {
                                self.media.search_spout();
                            }
                        });
                        let senders = self
                            .media
                            .spout_senders()
                            .iter()
                            .map(|name| {
                                let template = SourceTemplate {
                                    name: name.clone(),
                                    source: Source::Spout { name: name.clone() },
                                };
                                (template, String::new())
                            })
                            .collect();
                        self.browser_items(ui, senders);
                    }
                });
            }
            BrowserTab::Milkdrop => {
                hint(
                    ui,
                    "MilkDrop presets blend into each other when launched on the same layer, and can read another layer as input.",
                );
                self.browser_toolbar(ui, false);
                ui.horizontal(|ui| {
                    if ui
                        .button("Settings…")
                        .on_hover_text("Blend, pattern and input mix for every MilkDrop clip")
                        .clicked()
                    {
                        self.panel_to_show = Some(crate::workspace::PanelKind::MilkdropSettings);
                    }
                });
                self.milkdrop_browser(ui);
            }
            BrowserTab::Effects => self.effect_browser(ui),
            BrowserTab::Files => self.file_list(ui),
            BrowserTab::Online => self.online_browser(ui),
            BrowserTab::YouTube => self.youtube_browser(ui),
        }
    }

    /// View mode, favorites filter and search field shared by the browser tabs.
    pub(crate) fn browser_toolbar(&mut self, ui: &mut egui::Ui, animate: bool) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.files.thumbnails, false, "List");
            ui.selectable_value(&mut self.files.thumbnails, true, "Thumbnails");
            if animate && self.files.thumbnails {
                let mut on = !self.files.still_thumbnails;
                ui.checkbox(&mut on, "Animate").on_hover_text(
                    "Play every thumbnail. Off: thumbnails stay still and play under the pointer, which saves GPU time with many shaders.",
                );
                self.files.still_thumbnails = !on;
            }
            // The Favorites tab shows nothing else.
            if self.files.tab != BrowserTab::Favorites {
                let star = egui::RichText::new("★").color(if self.files.favorites_only {
                    theme::solo()
                } else {
                    theme::text_dim()
                });
                ui.toggle_value(&mut self.files.favorites_only, star).on_hover_text(
                    "Show only favorites. Click the star of an item, or right-click it, to add or remove it.",
                );
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.files.filter)
                    .hint_text("Filter…")
                    .desired_width(ui.available_width().clamp(40.0, 160.0)),
            );
        });
    }

    /// Whether a browser item passes the favorites filter and search field.
    pub(crate) fn browser_shows(&self, name: &str, favorite_key: &str) -> bool {
        (!self.files.favorites_only || self.favorites.contains(favorite_key))
            && (self.files.filter.trim().is_empty()
                || name
                    .to_lowercase()
                    .contains(&self.files.filter.trim().to_lowercase()))
    }

    /// The Library tab: folders added once and kept, browsed with their
    /// subfolders.
    fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("+ Add Folder…")
                .on_hover_text("Add a folder of images and videos to the library")
                .clicked()
            {
                self.file_request(crate::dialogs::FileRequest::FilesFolder);
            }
            let library = &mut self.files.library;
            if library.current.is_some() && ui.button("⟳").on_hover_text("Refresh").clicked() {
                library.refresh();
            }
            if library.roots.len() > 1 {
                let mut open = None;
                egui::ComboBox::from_id_salt("library root")
                    .selected_text(
                        library
                            .root()
                            .and_then(|r| r.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Folders".into()),
                    )
                    .show_ui(ui, |ui| {
                        for root in &library.roots {
                            let name = root
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| root.display().to_string());
                            if ui
                                .selectable_label(library.root() == Some(root), name)
                                .on_hover_text(root.display().to_string())
                                .clicked()
                            {
                                open = Some(root.clone());
                            }
                        }
                    });
                if let Some(root) = open {
                    library.open(root);
                }
            }
            if let Some(root) = library.root().cloned()
                && ui
                    .small_button("−")
                    .on_hover_text("Remove this folder from the library (files are not deleted)")
                    .clicked()
            {
                library.remove_root(&root);
            }
        });
        if self.files.library.current.is_none() {
            hint(
                ui,
                "Add folders of images and videos: they stay in the library. You can also drop files from your file manager onto a clip slot.",
            );
            return;
        }
        self.browser_toolbar(ui, false);
        // Breadcrumb: each folder from the root down, clickable.
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let trail = self.files.library.trail();
            let last = trail.len().saturating_sub(1);
            for (i, folder) in trail.iter().enumerate() {
                let name = folder
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| folder.display().to_string());
                if i > 0 {
                    ui.label(egui::RichText::new("›").color(theme::text_dim()));
                }
                if i == last {
                    ui.label(egui::RichText::new(name).strong());
                } else if ui.link(name).clicked() {
                    open = Some(folder.clone());
                }
            }
        });
        let folders = self.files.library.folders.clone();
        let entries: Vec<_> = self
            .files
            .library
            .entries
            .iter()
            .filter(|t| self.browser_shows(&t.name, &crate::library::source_key(t)))
            .cloned()
            .collect();
        egui::ScrollArea::vertical()
            .id_salt("library")
            .show(ui, |ui| {
                let up = self
                    .files
                    .library
                    .current
                    .as_ref()
                    .filter(|c| Some(*c) != self.files.library.root())
                    .and_then(|c| c.parent())
                    .map(|p| p.to_path_buf());
                if let Some(parent) = up
                    && folder_row(ui, "..", "Up one folder").double_clicked()
                {
                    open = Some(parent);
                }
                if !self.files.favorites_only && self.files.filter.trim().is_empty() {
                    for folder in &folders {
                        let name = folder
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        let row = folder_row(ui, &format!("📁 {name}"), "Double-click to open");
                        if row.double_clicked() || row.clicked() {
                            open = Some(folder.clone());
                        }
                    }
                }
                if entries.is_empty() {
                    hint(ui, "No images or videos here.");
                }
                let items = entries
                    .into_iter()
                    .map(|t| {
                        let tip = t
                            .source
                            .path()
                            .map(|p| p.display().to_string())
                            .unwrap_or_default();
                        (t, tip)
                    })
                    .collect();
                self.browser_items(ui, items);
            });
        if let Some(folder) = open {
            self.files.library.open(folder);
        }
    }

    /// Browser row with `tip` shown on hover when not empty.
    pub(crate) fn browser_item_with_tip(
        &mut self,
        ui: &mut egui::Ui,
        template: SourceTemplate,
        tip: &str,
    ) {
        let key = template
            .source
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| template.name.clone());
        let id = egui::Id::unique(("verso-browser", key));
        let (name, kind) = (template.name.clone(), source_kind(&template.source));
        let kind_color = if matches!(template.source, Source::Milkdrop { .. }) {
            theme::milkdrop()
        } else {
            theme::text_dim()
        };
        let favorite = self
            .favorites
            .contains(&crate::library::source_key(&template));
        let mut star = Rect::NOTHING;
        let response = ui
            .dnd_drag_source(id, template.clone(), |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                let fill = if response.contains_pointer() {
                    theme::bg_raised()
                } else {
                    theme::bg_panel()
                };
                ui.painter().rect_filled(rect, 2.0, fill);
                ui.painter().text(
                    rect.left_center() + Vec2::new(8.0, 0.0),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    egui::FontId::proportional(12.0),
                    theme::text(),
                );
                let kind_rect = ui.painter().text(
                    rect.right_center() - Vec2::new(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    kind,
                    egui::FontId::proportional(10.0),
                    kind_color,
                );
                star = Rect::from_center_size(
                    egui::pos2(kind_rect.left() - 13.0, rect.center().y),
                    Vec2::splat(18.0),
                );
                crate::source_browser::paint_star(
                    ui.painter(),
                    star,
                    favorite,
                    response.contains_pointer(),
                    false,
                );
                response
            })
            .inner;
        self.browser_item_events(response, star, template, tip);
    }

    /// Loads files dropped from the OS file manager into the slot under the
    /// pointer (else the selected slot), filling the following columns.
    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (files, pointer) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        if files.is_empty() {
            return;
        }
        // A Resolume Advanced Output preset goes to the Advanced Output window.
        let preset = files
            .iter()
            .map(|f| f.path())
            .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")));
        if let Some(path) = preset {
            let path = path.to_owned();
            self.import_advanced_output(&path);
            return;
        }
        let under_pointer = pointer.and_then(|pos| {
            self.cell_rects
                .iter()
                .find(|(rect, _)| rect.contains(pos))
                .map(|&(_, cell)| cell)
        });
        let Some((layer, first)) = under_pointer
            .or(self.selected_clip)
            .or(Some((self.selected_layer, 0)))
        else {
            return;
        };
        let templates: Vec<_> = files
            .iter()
            .filter_map(|f| SourceTemplate::from_path(f.path()))
            .collect();
        if templates.is_empty() {
            self.status = "No supported image or video in the dropped files.".to_owned();
            return;
        }
        let columns = self.comp.columns.len();
        for (offset, template) in templates.into_iter().enumerate() {
            let column = first + offset;
            if column >= columns {
                self.status = "Some files did not fit: add columns.".to_owned();
                break;
            }
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(template.clip()),
            });
        }
        self.selected_clip = Some((layer, first));
        self.props_tab = PropsTab::Clip;
    }
}

pub(crate) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::text_dim()));
}

/// Rows of a generator's parameters in the clip properties grid: the shared
/// transform and second color, then the ones of `kind`.
fn generator_params(ui: &mut egui::Ui, kind: Generator, params: &mut GeneratorParams) {
    ui.label("Color 2");
    widgets::color_edit(ui, &mut params.color2)
        .on_hover_text("Color of the dark parts; lower its alpha to make them transparent");
    ui.end_row();
    ui.label("Scale");
    ui.add(
        egui::Slider::new(
            &mut params.scale,
            GeneratorParams::MIN_SCALE..=GeneratorParams::MAX_SCALE,
        )
        .logarithmic(true),
    );
    ui.end_row();
    ui.label("Angle");
    ui.add(egui::Slider::new(&mut params.angle, -180.0..=180.0).suffix("°"));
    ui.end_row();
    ui.label("Position");
    ui.horizontal(|ui| {
        for (axis, v) in ["X", "Y"].into_iter().zip(&mut params.offset) {
            ui.add(
                egui::DragValue::new(v)
                    .speed(0.005)
                    .range(-2.0..=2.0)
                    .prefix(format!("{axis} ")),
            );
        }
    });
    ui.end_row();
    for (index, shape) in kind.shape_params().iter().enumerate() {
        ui.label(shape.name);
        let mut value = params.shape_value(kind, index);
        let mut slider = egui::Slider::new(&mut value, shape.min..=shape.max);
        if shape.integer {
            slider = slider.integer();
        }
        if ui.add(slider).changed() {
            params.set_shape(kind, index, value);
        }
        ui.end_row();
    }
    ui.label("");
    let defaults = GeneratorParams::default();
    // Specific values written at their default still count as default.
    let at_default = params.scale == defaults.scale
        && params.angle == defaults.angle
        && params.offset == defaults.offset
        && params.color2 == defaults.color2
        && (0..kind.shape_params().len())
            .all(|i| params.shape_value(kind, i) == defaults.shape_value(kind, i));
    if ui
        .add_enabled(!at_default, egui::Button::new("Reset"))
        .on_hover_text("Puts the generator parameters back to their defaults")
        .clicked()
    {
        *params = defaults;
    }
    ui.end_row();
}

fn source_kind(source: &Source) -> &'static str {
    match source {
        Source::Solid { .. } => "color",
        Source::Shader(_) => "shader",
        Source::Generator { .. } => "generator",
        Source::Image { .. } => "image",
        Source::Video { .. } => "video",
        Source::Milkdrop { .. } => "milkdrop",
        Source::Ndi { .. } => "NDI",
        Source::Spout { .. } => "Spout",
        Source::Text { .. } => "text",
        Source::Time { .. } => "time",
    }
}

/// Seconds from `m:ss` or plain seconds.
fn parse_clock(text: &str) -> Option<f64> {
    match text.trim().split_once(':') {
        Some((m, s)) => Some(m.trim().parse::<f64>().ok()? * 60.0 + s.trim().parse::<f64>().ok()?),
        None => text.trim().parse().ok(),
    }
}

/// Rows of a text source's look, in a property grid.
pub(crate) fn text_style_rows(
    ui: &mut egui::Ui,
    style: &mut verso_core::text::TextStyle,
    families: &[verso_text::FontFamily],
    loading: bool,
) {
    use verso_core::text::{TextAlign, TextAnimation, WEIGHTS, weight_name};
    let breathing = style.animation == TextAnimation::Breather;
    let family = families.iter().find(|f| f.name == style.font);
    ui.label("Font");
    ui.horizontal(|ui| {
        let shown = if style.font.is_empty() {
            "Default sans-serif".to_owned()
        } else {
            style.font.clone()
        };
        let variable = families.iter().filter(|f| f.is_variable()).count();
        egui::ComboBox::from_id_salt("text font")
            .selected_text(shown)
            .width(170.0)
            .height(320.0)
            .show_ui(ui, |ui| {
                if loading {
                    ui.label(egui::RichText::new("Loading fonts…").color(theme::text_dim()));
                }
                if breathing {
                    ui.label(
                        egui::RichText::new("Typefaces whose weight can vary")
                            .color(theme::text_dim()),
                    );
                    if variable == 0 && !loading {
                        ui.label(
                            egui::RichText::new("No variable font installed")
                                .color(theme::text_dim()),
                        );
                    }
                } else {
                    ui.selectable_value(&mut style.font, String::new(), "Default sans-serif");
                }
                for family in families.iter().filter(|f| !breathing || f.is_variable()) {
                    ui.selectable_value(&mut style.font, family.name.clone(), &family.name);
                }
            })
            .response
            .on_hover_text(if breathing {
                "Variable fonts with a weight axis. System fonts, plus those in the fonts \
                 folder of Verso's user folder"
            } else {
                "System fonts, plus those in the fonts folder of Verso's user folder"
            });
        ui.toggle_value(&mut style.italic, egui::RichText::new("I").italics())
            .on_hover_text("Italic");
    });
    ui.end_row();
    ui.label("Weight");
    let choices = match family {
        Some(family) => family.weight_choices(),
        None => WEIGHTS.iter().map(|(w, _)| *w).collect(),
    };
    egui::ComboBox::from_id_salt("text weight")
        .selected_text(weight_name(style.weight))
        .show_ui(ui, |ui| {
            for weight in &choices {
                ui.selectable_value(&mut style.weight, *weight, weight_name(*weight));
            }
        })
        .response
        .on_hover_text(match family {
            Some(f) if f.is_variable() => "Weights of this variable font",
            Some(_) => "Weights this font family has",
            None => "The nearest weight the font has is used",
        });
    ui.end_row();
    if breathing && family.is_none_or(|f| !f.is_variable()) {
        ui.label("");
        let folder = crate::online_ui::user_dir()
            .map(|d| d.join("fonts").display().to_string())
            .unwrap_or_else(|| "the fonts folder of Verso's user folder".into());
        let any = families.iter().any(|f| f.is_variable());
        let hint = if any {
            "The Breather needs a variable font: pick one in the list.".to_owned()
        } else {
            format!(
                "The Breather needs a variable font (one whose weight can vary), and none \
                 is installed. Windows 11 has Bahnschrift; others can be dropped in {folder}."
            )
        };
        ui.label(egui::RichText::new(hint).color(theme::text_dim()).small());
        ui.end_row();
    }
    ui.label("Size");
    ui.add(egui::Slider::new(&mut style.size, 0.01..=1.0).logarithmic(true))
        .on_hover_text("Font size as a fraction of the frame height");
    ui.end_row();
    ui.label("Color");
    widgets::color_edit(ui, &mut style.color);
    ui.end_row();
    ui.label("Align");
    ui.horizontal(|ui| {
        for align in TextAlign::ALL {
            ui.selectable_value(&mut style.align, align, align.name());
        }
    });
    ui.end_row();
    ui.label("Position");
    ui.horizontal(|ui| {
        ui.add(
            egui::DragValue::new(&mut style.position[0])
                .speed(0.005)
                .prefix("x "),
        );
        ui.add(
            egui::DragValue::new(&mut style.position[1])
                .speed(0.005)
                .prefix("y "),
        );
    })
    .response
    .on_hover_text("Center of the text: 0 is the left or top edge, 1 the right or bottom");
    ui.end_row();
    ui.label("Spacing");
    ui.add(egui::Slider::new(&mut style.letter_spacing, -0.2..=1.0).suffix(" em"));
    ui.end_row();
    ui.label("Line height");
    ui.add(egui::Slider::new(&mut style.line_height, 0.6..=3.0));
    ui.end_row();
    ui.label("Animation");
    egui::ComboBox::from_id_salt("text animation")
        .selected_text(style.animation.name())
        .show_ui(ui, |ui| {
            for animation in TextAnimation::ALL {
                ui.selectable_value(&mut style.animation, animation, animation.name());
            }
        });
    ui.end_row();
    // The Breather needs a variable font: switch to one when the font
    // cannot vary.
    if !breathing
        && style.animation == TextAnimation::Breather
        && family.is_none_or(|f| !f.is_variable())
        && let Some(variable) = families
            .iter()
            .find(|f| f.name == "Bahnschrift" && f.is_variable())
            .or_else(|| families.iter().find(|f| f.is_variable()))
    {
        style.font = variable.name.clone();
    }
    if style.animation != TextAnimation::None {
        ui.label("Speed");
        ui.add(egui::Slider::new(&mut style.speed, 0.0..=4.0).suffix("×"));
        ui.end_row();
    }
    if breathing {
        let (lo, hi) = family.and_then(|f| f.weight_axis).unwrap_or((100.0, 900.0));
        let breath = &mut style.breath;
        ui.label("Weights");
        ui.horizontal(|ui| {
            ui.add(
                egui::DragValue::new(&mut breath.min_weight)
                    .range(lo..=hi)
                    .speed(2.0)
                    .max_decimals(0),
            );
            ui.label("to");
            ui.add(
                egui::DragValue::new(&mut breath.max_weight)
                    .range(lo..=hi)
                    .speed(2.0)
                    .max_decimals(0),
            );
        })
        .response
        .on_hover_text(format!(
            "Thinnest and boldest weights the letters reach ({lo:.0} to {hi:.0} for this font)"
        ));
        ui.end_row();
        ui.label("Wave");
        ui.add(egui::Slider::new(&mut breath.spread, 0.0..=0.5))
            .on_hover_text(
                "How far apart neighbouring letters are in their breath: \
                 0 breathes all letters together",
            );
        ui.end_row();
        if family.is_some_and(|f| f.width_axis) {
            ui.label("Width");
            ui.checkbox(&mut breath.width, "Widen as they grow bolder")
                .on_hover_text("This font also has a width axis");
            ui.end_row();
        }
    }
}

/// Grid row showing a file name, with the full path on hover.
fn file_row(ui: &mut egui::Ui, path: &std::path::Path) {
    ui.label("File");
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ui.label(name).on_hover_text(path.display().to_string());
    ui.end_row();
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BrowserTab {
    #[default]
    Sources,
    Milkdrop,
    Effects,
    Files,
    Online,
    YouTube,
    /// Every starred source, MilkDrop family, effect and file.
    Favorites,
}

/// State of the browser.
#[derive(Default)]
pub struct FileBrowser {
    pub tab: BrowserTab,
    /// Items shown as a grid of thumbnails instead of a list.
    pub thumbnails: bool,
    /// Thumbnails stay still and only play under the pointer.
    pub still_thumbnails: bool,
    /// Show only favorites.
    pub favorites_only: bool,
    /// Search field: shows items whose name contains it.
    pub filter: String,
    pub library: crate::library::Library,
}

impl FileBrowser {
    /// Adds `folder` to the library and shows it.
    pub fn open(&mut self, folder: std::path::PathBuf) {
        self.library.add_root(folder);
        self.tab = BrowserTab::Files;
    }
}

/// A folder row of the library.
fn folder_row(ui: &mut egui::Ui, name: &str, tip: &str) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
    let fill = if response.hovered() {
        theme::bg_raised()
    } else {
        theme::bg_panel()
    };
    ui.painter().rect_filled(rect, 2.0, fill);
    ui.painter().text(
        rect.left_center() + Vec2::new(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(12.0),
        theme::text(),
    );
    response.on_hover_text(tip)
}
