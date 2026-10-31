//! Lower panels: properties, preview and output monitors, source browser.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, TextureId, Vec2};
use verso_core::{BlendMode, Command, FaderSide, Generator, Source};

use crate::app::{PropsTab, VersoApp};
use crate::grid::SourceTemplate;
use crate::{theme, widgets};

/// Draws `texture` as large as fits in `size`, letterboxed on black. Returns
/// the monitor rectangle.
fn monitor_image(ui: &mut egui::Ui, texture: Option<TextureId>, size: Vec2, aspect: f32) -> Rect {
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, Color32::BLACK);
    if let Some(texture) = texture {
        let fit = if rect.width() / rect.height() > aspect {
            Vec2::new(rect.height() * aspect, rect.height())
        } else {
            Vec2::new(rect.width(), rect.width() / aspect)
        };
        painter.image(
            texture,
            Rect::from_center_size(rect.center(), fit),
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
        egui::pos2(strip.left() + strip.width() * progress.clamp(0.0, 1.0), strip.bottom()),
    );
    painter.rect_filled(done, 0.0, theme::accent());
}

fn property_grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
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
        let (mut name, mut blend, mut opacity, mut bypass) =
            (group.name.clone(), group.blend, group.opacity, group.bypass);
        property_grid(ui, "group", |ui| {
            ui.label("Name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameGroup { group: g, name });
            }
            ui.end_row();

            ui.label("Blend Mode");
            egui::ComboBox::from_id_salt("group blend")
                .selected_text(blend.name())
                .show_ui(ui, |ui| {
                    for b in BlendMode::ALL {
                        ui.selectable_value(&mut blend, b, b.name());
                    }
                });
            if blend != self.comp.groups[g].blend {
                self.send(Command::SetGroupBlend { group: g, blend });
            }
            ui.end_row();

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
        let (mut name, mut blend, mut opacity, mut bypass, mut solo, mut side) = (
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
            egui::ComboBox::from_id_salt("layer blend")
                .selected_text(blend.name())
                .show_ui(ui, |ui| {
                    for b in BlendMode::ALL {
                        ui.selectable_value(&mut blend, b, b.name());
                    }
                });
            if blend != self.comp.layers[index].blend {
                self.send(Command::SetLayerBlend {
                    layer: index,
                    blend,
                });
            }
            ui.end_row();

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
                    ui.color_edit_button_rgba_unmultiplied(color);
                    ui.end_row();
                }
                Source::Generator { kind, color, speed } => {
                    ui.label("Source");
                    egui::ComboBox::from_id_salt("clip generator")
                        .selected_text(kind.name())
                        .show_ui(ui, |ui| {
                            for g in Generator::ALL {
                                ui.selectable_value(kind, g, g.name());
                            }
                        });
                    ui.end_row();
                    ui.label("Color");
                    ui.color_edit_button_rgba_unmultiplied(color);
                    ui.end_row();
                    ui.label("Speed");
                    ui.add(egui::Slider::new(speed, 0.0..=4.0));
                    ui.end_row();
                }
                Source::Milkdrop {
                    family,
                    variant,
                    blend,
                    input,
                    input_mix,
                } => {
                    let family = family.clone();
                    if let Some(picked) = self.milkdrop_props(ui, &family, *variant) {
                        *variant = picked;
                    }
                    self.milkdrop_playback(ui, layer, blend, input, input_mix);
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
                Source::Video {
                    path,
                    speed,
                    looping,
                } => {
                    ui.label("Source");
                    ui.label("Video");
                    ui.end_row();
                    file_row(ui, path);
                    ui.label("Speed");
                    ui.add(egui::Slider::new(speed, 0.0..=4.0).suffix("×"));
                    ui.end_row();
                    ui.label("Loop");
                    ui.checkbox(looping, "");
                    ui.end_row();
                    ui.label("Position");
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
        // Media clips show their own frame at its own aspect.
        let previewed = self.preview_slot();
        let (clip_texture, clip_aspect) = match previewed {
            Some((layer, column)) => {
                let procedural = self.comp.layers[layer].clips[column]
                    .as_ref()
                    .is_none_or(|clip| clip.source.is_procedural());
                let milk = match self.comp.layers[layer].clips[column].as_ref().map(|c| &c.source) {
                    Some(Source::Milkdrop { family, .. }) => Some(family.clone()),
                    _ => None,
                };
                if let Some(family) = milk {
                    (self.milk_preview(ui.ctx(), &family), 16.0 / 9.0)
                } else if procedural {
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
        monitor_image(ui, Some(self.output_texture), size, aspect);
    }

    pub(crate) fn browser(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "Browser");
        widgets::tabs(
            ui,
            &mut self.files.tab,
            &[
                (BrowserTab::Sources, "Sources"),
                (BrowserTab::Effects, "Effects"),
                (BrowserTab::Files, "Files"),
                (BrowserTab::Online, "Online"),
            ],
        );
        ui.separator();
        match self.files.tab {
            BrowserTab::Sources => {
                hint(
                    ui,
                    "Drag onto a clip slot, or double-click to load into the selected slot.",
                );
                for template in SourceTemplate::all() {
                    self.browser_item(ui, template);
                }
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
                    if !shaders.is_empty() {
                        ui.add_space(6.0);
                        hint(ui, "Shaders");
                        for template in shaders {
                            self.browser_item(ui, template);
                        }
                    }
                    self.milkdrop_browser(ui);
                });
            }
            BrowserTab::Effects => self.effect_browser(ui),
            BrowserTab::Files => self.file_list(ui),
            BrowserTab::Online => self.online_browser(ui),
        }
    }

    fn file_list(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.button("Open Folder…").clicked() {
                self.dialogs.open(crate::dialogs::DialogKind::MediaFolder, || {
                    rfd::FileDialog::new().pick_folder()
                });
            }
            if self.files.folder.is_some() && ui.button("⟳").on_hover_text("Refresh").clicked() {
                self.files.refresh();
            }
        });
        let Some(folder) = &self.files.folder else {
            hint(
                ui,
                "Pick a folder of images and videos, or drop files from your file manager onto a clip slot.",
            );
            return;
        };
        hint(ui, &folder.display().to_string());
        let entries = self.files.entries.clone();
        if entries.is_empty() {
            hint(ui, "No images or videos in this folder.");
        }
        egui::ScrollArea::vertical().show(ui, |ui| {
            for template in entries {
                self.browser_item(ui, template);
            }
        });
    }

    /// One draggable row of the browser.
    fn browser_item(&mut self, ui: &mut egui::Ui, template: SourceTemplate) {
        self.browser_item_with_tip(ui, template, "");
    }

    /// Browser row with `tip` shown on hover when not empty.
    pub(crate) fn browser_item_with_tip(&mut self, ui: &mut egui::Ui, template: SourceTemplate, tip: &str) {
        let key = template
            .source
            .path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| template.name.clone());
        let id = egui::Id::unique(("verso-browser", key));
        let (name, kind) = (template.name.clone(), source_kind(&template.source));
        let response = ui
            .dnd_drag_source(id, template.clone(), |ui| {
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
                    &name,
                    egui::FontId::proportional(12.0),
                    theme::text(),
                );
                ui.painter().text(
                    rect.right_center() - Vec2::new(8.0, 0.0),
                    egui::Align2::RIGHT_CENTER,
                    kind,
                    egui::FontId::proportional(10.0),
                    theme::text_dim(),
                );
                response
            })
            .inner;
        let response = if tip.is_empty() { response } else { response.on_hover_text(tip) };
        if response.double_clicked()
            && let Some((layer, column)) = self.selected_clip
        {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(template.clip()),
            });
            self.props_tab = PropsTab::Clip;
        }
    }

    /// Loads files dropped from the OS file manager into the slot under the
    /// pointer (else the selected slot), filling the following columns.
    pub(crate) fn handle_dropped_files(&mut self, ctx: &egui::Context) {
        let (files, pointer) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        if files.is_empty() {
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

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::text_dim()));
}

fn source_kind(source: &Source) -> &'static str {
    match source {
        Source::Solid { .. } => "color",
        Source::Shader(_) => "shader",
        Source::Generator { .. } => "generator",
        Source::Image { .. } => "image",
        Source::Video { .. } => "video",
        Source::Milkdrop { .. } => "milkdrop",
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
    Effects,
    Files,
    Online,
}

/// State of the browser's Files tab.
#[derive(Default)]
pub struct FileBrowser {
    pub tab: BrowserTab,
    pub folder: Option<std::path::PathBuf>,
    pub entries: Vec<SourceTemplate>,
}

impl FileBrowser {
    pub fn open(&mut self, folder: std::path::PathBuf) {
        self.folder = Some(folder);
        self.refresh();
    }

    /// Lists the supported media files of the folder, sorted by name.
    pub fn refresh(&mut self) {
        let Some(folder) = &self.folder else { return };
        let mut paths: Vec<_> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .collect();
        paths.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
        self.entries = paths
            .iter()
            .filter_map(|p| SourceTemplate::from_path(p))
            .collect();
    }
}
