//! Clip grid: composition row, column headers, one row per layer.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use verso_core::control::{Action, Param};
use verso_core::{Clip, Command, EffectTarget, FaderSide, Generator, Source};

use crate::app::{PropsTab, VersoApp};
use crate::effects_ui::EffectTemplate;
use crate::{theme, widgets};

pub const CELL: Vec2 = Vec2::new(112.0, 63.0);
pub const HEADER_WIDTH: f32 = 236.0;
const COLUMN_HEADER_HEIGHT: f32 = 22.0;

/// A source dragged from the browser.
#[derive(Clone)]
pub struct SourceTemplate {
    pub name: String,
    pub source: Source,
}

impl SourceTemplate {
    pub fn all() -> Vec<SourceTemplate> {
        let mut all: Vec<_> = Generator::ALL
            .iter()
            .map(|&kind| SourceTemplate {
                name: kind.name().to_owned(),
                source: Source::Generator {
                    kind,
                    color: [1.0, 1.0, 1.0, 1.0],
                    speed: 1.0,
                },
            })
            .collect();
        all.push(SourceTemplate {
            name: "Solid Color".to_owned(),
            source: Source::Solid {
                color: [1.0, 1.0, 1.0, 1.0],
            },
        });
        all
    }

    /// Template for an image or video file; `None` for unsupported files.
    pub fn from_path(path: &std::path::Path) -> Option<SourceTemplate> {
        let name = path.file_stem()?.to_string_lossy().into_owned();
        let source = match verso_media::MediaKind::of(path)? {
            verso_media::MediaKind::Image => Source::Image {
                path: path.to_owned(),
            },
            verso_media::MediaKind::Video => Source::Video {
                path: path.to_owned(),
                speed: 1.0,
                looping: true,
            },
        };
        Some(SourceTemplate { name, source })
    }

    pub fn clip(&self) -> Clip {
        Clip::new(self.name.clone(), self.source.clone())
    }
}

impl VersoApp {
    pub(crate) fn grid(&mut self, ui: &mut egui::Ui) {
        self.cell_rects.clear();
        egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(3.0);
            ui.horizontal(|ui| {
                self.composition_header(ui);
                for column in 0..self.comp.columns.len() {
                    self.column_header(ui, column);
                }
            });
            // Top layer first, as it renders on top; group headers above
            // their layers.
            for row in crate::groups_ui::rows(&self.comp) {
                ui.horizontal(|ui| match row {
                    crate::groups_ui::Row::Group { group, depth } => {
                        self.group_row(ui, group, depth)
                    }
                    crate::groups_ui::Row::Layer { layer } => {
                        self.layer_header(ui, layer);
                        for column in 0..self.comp.columns.len() {
                            self.cell(ui, layer, column);
                        }
                    }
                });
            }
        });
    }

    fn composition_header(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(
            Vec2::new(HEADER_WIDTH, COLUMN_HEADER_HEIGHT),
            Sense::click(),
        );
        let selected = self.props_tab == PropsTab::Composition;
        ui.painter().rect_filled(rect, 2.0, theme::bg_header());
        if selected {
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, theme::select()),
                StrokeKind::Inside,
            );
        }
        if response.dnd_hover_payload::<EffectTemplate>().is_some() {
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(2.0, theme::accent()),
                StrokeKind::Inside,
            );
        }
        if let Some(template) = response.dnd_release_payload::<EffectTemplate>() {
            self.add_effect(EffectTarget::Composition, &template);
            self.props_tab = PropsTab::Composition;
        }
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(3.0, 2.0))));
        child.horizontal_centered(|ui| {
            if widgets::square_button(ui, "X", "Clear all layers").clicked() {
                self.send(Command::ClearAll);
            }
            ui.label(egui::RichText::new("Composition").strong());
            let mut opacity = self.comp.master.opacity;
            let fader = widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(ui.available_width(), 16.0),
                theme::accent(),
                1.0,
            );
            let learning = self.learnable(ui, fader.rect, Action::Set(Param::Master));
            if fader.changed() && !learning {
                self.send(Command::SetMasterOpacity(opacity));
            }
            self.modulation_menu(&fader, Param::Master);
        });
        if response.clicked() {
            self.props_tab = PropsTab::Composition;
        }
    }

    fn column_header(&mut self, ui: &mut egui::Ui, column: usize) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(CELL.x, COLUMN_HEADER_HEIGHT), Sense::click());
        let playing = self.comp.layers.iter().any(|l| l.active == Some(column));
        let fill = if response.hovered() {
            theme::bg_raised().gamma_multiply(1.3)
        } else {
            theme::bg_header()
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, fill);
        if playing {
            let bar = Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, 2.0),
                Vec2::new(rect.width(), 2.0),
            );
            painter.rect_filled(bar, 0.0, theme::accent());
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &self.comp.columns[column].name,
            FontId::proportional(11.0),
            if playing {
                theme::text()
            } else {
                theme::text_dim()
            },
        );
        let response = response.on_hover_text("Click to launch the column");
        let learning = self.learnable(ui, rect, Action::LaunchColumn(column));
        if response.clicked() && !learning {
            self.send(Command::TriggerColumn(column));
        }
        response.context_menu(|ui| {
            let mut name = self.comp.columns[column].name.clone();
            ui.label("Column name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameColumn { column, name });
            }
            ui.separator();
            if ui.button("Clear Column").on_hover_text("Eject this column on every layer playing it").clicked() {
                let playing: Vec<usize> = (0..self.comp.layers.len())
                    .filter(|&l| self.comp.layers[l].active == Some(column))
                    .collect();
                for layer in playing {
                    self.send(Command::EjectLayer(layer));
                }
                ui.close();
            }
            if ui
                .add_enabled(self.comp.columns.len() > 1, egui::Button::new("Delete Column"))
                .on_hover_text("Deletes the column and its clips on every layer (Ctrl+Z undoes)")
                .clicked()
            {
                self.send(Command::RemoveColumn(column));
                ui.close();
            }
        });
    }

    fn layer_header(&mut self, ui: &mut egui::Ui, index: usize) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(HEADER_WIDTH, CELL.y), Sense::click());
        let layer = &self.comp.layers[index];
        let selected = (self.props_tab == PropsTab::Layer && self.selected_layer == index)
            || self.selected_layers.contains(&index);
        let painter = ui.painter();
        painter.rect_filled(
            rect,
            2.0,
            if self.selected_layers.contains(&index) {
                theme::bg_raised()
            } else {
                theme::bg_header()
            },
        );
        if selected {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, theme::select()),
                StrokeKind::Inside,
            );
        }
        // One stripe per group around the layer, then the playing indicator.
        let path = self.comp.layer_path(index);
        let left = crate::groups_ui::paint_stripes(ui, rect, &path);
        let painter = ui.painter();
        if self.comp.layer_gain(index) > 0.0 {
            let strip =
                Rect::from_min_size(egui::pos2(left, rect.top()), Vec2::new(3.0, rect.height()));
            painter.rect_filled(strip, 0.0, theme::accent());
        }
        if response.dnd_hover_payload::<EffectTemplate>().is_some() {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(2.0, theme::accent()),
                StrokeKind::Inside,
            );
        }

        let (mut bypass, mut solo, mut opacity, fader_side) =
            (layer.bypass, layer.solo, layer.opacity, layer.fader);
        let name = layer.name.clone();
        let blend = layer.blend.name();
        let inner = Rect::from_min_max(egui::pos2(left + 6.0, rect.top()), rect.max)
            .shrink2(Vec2::new(2.0, 4.0));
        // Opacity fader on the right edge, full header height.
        let fader_rect = Rect::from_min_max(
            egui::pos2(inner.right() - 16.0, inner.top()),
            inner.right_bottom(),
        );
        let mut fader_ui = ui.new_child(egui::UiBuilder::new().max_rect(fader_rect));
        let fader = widgets::vfader(
            &mut fader_ui,
            &mut opacity,
            fader_rect.size(),
            theme::accent(),
            1.0,
        );
        let learning = self.learnable(ui, fader_rect, Action::Set(Param::LayerOpacity(index)));
        if fader.changed() && !learning {
            self.send(Command::SetLayerOpacity {
                layer: index,
                opacity,
            });
        }
        self.modulation_menu(&fader, Param::LayerOpacity(index));

        let controls = Rect::from_min_max(
            inner.min,
            egui::pos2(fader_rect.left() - 6.0, inner.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(controls));
        child.vertical(|ui| {
            ui.horizontal(|ui| {
                let eject = widgets::square_button(ui, "X", "Eject the playing clip");
                if !self.learnable(ui, eject.rect, Action::EjectLayer(index)) && eject.clicked() {
                    self.send(Command::EjectLayer(index));
                }
                let toggle = widgets::toggle(ui, &mut bypass, "B", theme::warn(), "Bypass");
                if !self.learnable(ui, toggle.rect, Action::Set(Param::LayerBypass(index)))
                    && toggle.changed()
                {
                    self.send(Command::SetLayerBypass {
                        layer: index,
                        bypass,
                    });
                }
                let toggle = widgets::toggle(ui, &mut solo, "S", theme::solo(), "Solo");
                if !self.learnable(ui, toggle.rect, Action::Set(Param::LayerSolo(index)))
                    && toggle.changed()
                {
                    self.send(Command::SetLayerSolo { layer: index, solo });
                }
                ui.label(egui::RichText::new(name).strong());
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                for (side, label) in [(FaderSide::A, "A"), (FaderSide::B, "B")] {
                    let mut on = fader_side == side;
                    if widgets::toggle(ui, &mut on, label, theme::accent(), "Crossfader side")
                        .changed()
                    {
                        let side = if on { side } else { FaderSide::None };
                        self.send(Command::SetLayerFader { layer: index, side });
                    }
                }
                ui.label(egui::RichText::new(blend).small().color(theme::text_dim()));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", opacity * 100.0))
                            .small()
                            .monospace()
                            .color(theme::text_dim()),
                    );
                });
            });
        });
        if let Some(template) = response.dnd_release_payload::<EffectTemplate>() {
            self.add_effect(EffectTarget::Layer(index), &template);
            self.selected_layer = index;
            self.props_tab = PropsTab::Layer;
        } else if response.clicked() {
            let modifiers = ui.input(|i| i.modifiers);
            self.click_layer(index, modifiers);
        }
        response.context_menu(|ui| self.layer_group_menu(ui, index));
    }

    fn cell(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let (rect, response) = ui.allocate_exact_size(CELL, Sense::click());
        let milk_family = match self.comp.layers[layer].clips[column].as_ref().map(|c| &c.source) {
            Some(Source::Milkdrop { family, .. }) => Some(family.clone()),
            _ => None,
        };
        let milk_preview = milk_family.and_then(|family| self.milk_preview(ui.ctx(), &family));
        let clip = self.comp.layers[layer].clips[column].as_ref();
        let active = self.comp.layers[layer].active == Some(column);
        let selected = self.selected_clip == Some((layer, column));
        let drop_hover = response.dnd_hover_payload::<SourceTemplate>().is_some()
            || (response.dnd_hover_payload::<EffectTemplate>().is_some() && clip.is_some());
        let painter = ui.painter_at(rect);
        self.cell_rects.push((rect, (layer, column)));
        if response.hovered() && clip.is_some() {
            self.hovered_clip = Some((layer, column));
        }

        let texture = clip.and_then(|clip| {
            if let Some(id) = milk_preview {
                Some((id, None))
            } else if clip.source.is_procedural() {
                self.cell_textures
                    .get(&(layer, column))
                    .map(|&id| (id, None))
            } else {
                self.media
                    .clip_texture(&self.comp, layer, column)
                    .map(|(id, size)| (id, Some(size)))
            }
        });
        match (clip, texture) {
            (Some(clip), Some((texture, media_size))) => {
                let image = rect.shrink(1.0);
                let tint = if active || response.hovered() {
                    Color32::WHITE
                } else {
                    Color32::from_gray(150)
                };
                painter.image(texture, image, cover_uv(media_size, image.size()), tint);
                let label = Rect::from_min_max(
                    image.left_bottom() - Vec2::new(0.0, 15.0),
                    image.right_bottom(),
                );
                painter.rect_filled(label, 0.0, Color32::from_black_alpha(170));
                painter.text(
                    label.left_center() + Vec2::new(4.0, 0.0),
                    Align2::LEFT_CENTER,
                    &clip.name,
                    FontId::proportional(10.5),
                    theme::text(),
                );
                // Launch counter of this slot, bottom right over the name strip.
                let launches = self.comp.layers[layer].launch_count(column);
                if launches > 0 {
                    let galley = painter.layout_no_wrap(
                        launches.to_string(),
                        FontId::monospace(10.0),
                        theme::bg_deep(),
                    );
                    let badge = Rect::from_min_size(
                        label.right_center() - Vec2::new(galley.size().x + 8.0, 6.5),
                        Vec2::new(galley.size().x + 6.0, 13.0),
                    );
                    painter.rect_filled(badge, 3.0, theme::accent());
                    painter.galley(
                        badge.center() - galley.size() / 2.0,
                        galley,
                        theme::bg_deep(),
                    );
                }
            }
            (Some(clip), None) => {
                painter.rect_filled(rect, 2.0, theme::bg_raised());
                let failed = self.media.failed(&clip.source);
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    if failed { "Missing" } else { "Loading…" },
                    FontId::proportional(10.5),
                    if failed {
                        theme::warn()
                    } else {
                        theme::text_dim()
                    },
                );
                painter.text(
                    rect.left_bottom() + Vec2::new(4.0, -3.0),
                    Align2::LEFT_BOTTOM,
                    &clip.name,
                    FontId::proportional(10.5),
                    theme::text_dim(),
                );
            }
            (None, _) => {
                painter.rect_filled(rect, 2.0, theme::bg_panel());
            }
        }

        let stroke = if drop_hover || active {
            Stroke::new(2.0, theme::accent())
        } else if selected {
            Stroke::new(1.5, theme::select())
        } else {
            Stroke::new(1.0, theme::line())
        };
        painter.rect_stroke(rect, 2.0, stroke, StrokeKind::Inside);
        if active {
            // Play marker in the top-left corner.
            let c = rect.left_top() + Vec2::new(8.0, 8.0);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    c + Vec2::new(-3.0, -4.0),
                    c + Vec2::new(4.0, 0.0),
                    c + Vec2::new(-3.0, 4.0),
                ],
                theme::accent(),
                Stroke::NONE,
            ));
        }

        if let Some(template) = response.dnd_release_payload::<EffectTemplate>() {
            if self.comp.layers[layer].clips[column].is_some() {
                self.add_effect(EffectTarget::Clip { layer, column }, &template);
                self.selected_clip = Some((layer, column));
                self.props_tab = PropsTab::Clip;
            }
        } else if let Some(template) = response.dnd_release_payload::<SourceTemplate>() {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(template.clip()),
            });
            self.selected_clip = Some((layer, column));
            self.props_tab = PropsTab::Clip;
        } else if self.learnable(ui, rect, Action::LaunchClip { layer, column }) {
            // Learn mode picks the cell instead of launching it.
        } else if response.clicked() {
            self.selected_clip = Some((layer, column));
            self.selected_layer = layer;
            if self.comp.layers[layer].clips[column].is_some() {
                self.props_tab = PropsTab::Clip;
            }
            self.send(Command::TriggerClip { layer, column });
        }
        response.context_menu(|ui| self.cell_menu(ui, layer, column));
    }

    fn cell_menu(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        for template in SourceTemplate::all() {
            if ui.button(&template.name).clicked() {
                self.send(Command::SetClip {
                    layer,
                    column,
                    clip: Some(template.clip()),
                });
                self.selected_clip = Some((layer, column));
                self.props_tab = PropsTab::Clip;
            }
        }
        ui.separator();
        let launches = self.comp.layers[layer].launch_count(column);
        if ui
            .add_enabled(
                launches > 0,
                egui::Button::new(format!("Reset Counter ({launches})")),
            )
            .clicked()
        {
            self.send(Command::ResetLaunchCount { layer, column });
        }
        if ui.button("Clear").clicked() {
            self.send(Command::SetClip {
                layer,
                column,
                clip: None,
            });
        }
    }

    pub(crate) fn crossfader(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.add_space(HEADER_WIDTH - 140.0);
            ui.label(egui::RichText::new("A").strong());
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(320.0, 16.0), Sense::click_and_drag());
            let mut value = self.comp.master.crossfader;
            if let Some(pos) = response.interact_pointer_pos()
                && (response.dragged() || response.clicked())
            {
                value = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
            }
            if response.double_clicked() {
                value = 0.5;
            }
            let learning = self.learnable(ui, rect, Action::Set(Param::Crossfader));
            if value != self.comp.master.crossfader && !learning {
                self.send(Command::SetCrossfader(value));
            }
            self.modulation_menu(&response, Param::Crossfader);
            let painter = ui.painter_at(rect);
            let track = Rect::from_center_size(rect.center(), Vec2::new(rect.width(), 4.0));
            painter.rect_filled(track, 2.0, theme::bg_deep());
            painter.line_segment(
                [rect.center_top(), rect.center_bottom()],
                Stroke::new(1.0, theme::line()),
            );
            let x = rect.left() + rect.width() * value;
            let handle =
                Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::new(10.0, 16.0));
            painter.rect_filled(handle, 2.0, theme::accent());
            ui.label(egui::RichText::new("B").strong());
            ui.label(egui::RichText::new("Crossfader").color(theme::text_dim()));
        });
    }
}

/// UV rectangle that crops an image of `size` to fill `target` without
/// distortion. `None` (procedural previews) uses the whole texture.
fn cover_uv(size: Option<(u32, u32)>, target: Vec2) -> Rect {
    let full = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
    let Some((w, h)) = size else { return full };
    let image_aspect = w as f32 / h.max(1) as f32;
    let target_aspect = target.x / target.y.max(1.0);
    if image_aspect > target_aspect {
        let keep = target_aspect / image_aspect;
        Rect::from_min_max(
            egui::pos2(0.5 - keep / 2.0, 0.0),
            egui::pos2(0.5 + keep / 2.0, 1.0),
        )
    } else {
        let keep = image_aspect / target_aspect;
        Rect::from_min_max(
            egui::pos2(0.0, 0.5 - keep / 2.0),
            egui::pos2(1.0, 0.5 + keep / 2.0),
        )
    }
}
