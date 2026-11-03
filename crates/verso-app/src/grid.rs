//! Clip grid: composition row, column headers, one row per layer.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use verso_core::control::{Action, Param};
use verso_core::{Clip, Command, EffectTarget, FaderSide, Generator, Source};

use crate::app::{PropsTab, VersoApp};
use crate::effects_ui::{EffectTemplate, group_color};
use crate::{theme, widgets};

pub const CELL: Vec2 = Vec2::new(112.0, 63.0);
const HEADER_WIDTH: f32 = 236.0;
const COLUMN_HEADER_HEIGHT: f32 = 22.0;

/// A clip dragged from another cell: `(layer, column)`.
#[derive(Clone, Copy)]
pub struct CellPayload(pub usize, pub usize);

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
            // Top layer first, as it renders on top.
            for layer in (0..self.comp.layers.len()).rev() {
                ui.horizontal(|ui| {
                    self.layer_header(ui, layer);
                    for column in 0..self.comp.columns.len() {
                        self.cell(ui, layer, column);
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
        ui.painter().rect_filled(rect, 2.0, theme::BG_HEADER);
        if selected {
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, theme::SELECT),
                StrokeKind::Inside,
            );
        }
        if response.dnd_hover_payload::<EffectTemplate>().is_some() {
            ui.painter().rect_stroke(
                rect,
                2.0,
                Stroke::new(2.0, theme::ACCENT),
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
            if widgets::square_button(ui, "X", "Clear all layers (Shift+Esc)").clicked() {
                self.send_live(Command::ClearAll);
            }
            ui.label(egui::RichText::new("Composition").strong());
            let mut opacity = self.comp.master.opacity;
            let fader = widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(ui.available_width(), 16.0),
                theme::ACCENT,
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
            theme::BG_RAISED.gamma_multiply(1.3)
        } else {
            theme::BG_HEADER
        };
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 2.0, fill);
        if playing {
            let bar = Rect::from_min_size(
                rect.left_bottom() - Vec2::new(0.0, 2.0),
                Vec2::new(rect.width(), 2.0),
            );
            painter.rect_filled(bar, 0.0, theme::ACCENT);
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            &self.comp.columns[column].name,
            FontId::proportional(11.0),
            if playing {
                theme::TEXT
            } else {
                theme::TEXT_DIM
            },
        );
        let key = match column {
            0..=8 => format!(" ({})", column + 1),
            9..=17 => format!(" (Shift+{})", column - 8),
            _ => String::new(),
        };
        let response = response.on_hover_text(format!(
            "Click to launch the column{key} · right-click: rename, move, delete"
        ));
        let learning = self.learnable(ui, rect, Action::LaunchColumn(column));
        if response.clicked() && !learning {
            self.send_live(Command::TriggerColumn(column));
        }
        let count = self.comp.columns.len();
        response.context_menu(|ui| {
            let mut name = self.comp.columns[column].name.clone();
            ui.label("Column name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameColumn { column, name });
            }
            ui.separator();
            if ui
                .add_enabled(column > 0, egui::Button::new("Move Left"))
                .clicked()
            {
                self.send(Command::MoveColumn {
                    from: column,
                    to: column - 1,
                });
            }
            if ui
                .add_enabled(column + 1 < count, egui::Button::new("Move Right"))
                .clicked()
            {
                self.send(Command::MoveColumn {
                    from: column,
                    to: column + 1,
                });
            }
            if ui
                .add_enabled(count > 1, egui::Button::new("Delete Column"))
                .on_hover_text("Ctrl+Z restores it")
                .clicked()
            {
                self.send(Command::RemoveColumn(column));
            }
        });
    }

    fn layer_header(&mut self, ui: &mut egui::Ui, index: usize) {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(HEADER_WIDTH, CELL.y), Sense::click());
        let layer = &self.comp.layers[index];
        let selected = self.props_tab == PropsTab::Layer && self.selected_layer == index;
        let painter = ui.painter();
        painter.rect_filled(rect, 2.0, theme::BG_HEADER);
        if selected {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(1.0, theme::SELECT),
                StrokeKind::Inside,
            );
        }
        // Playing indicator on the left edge, group tag next to it.
        if self.comp.layer_gain(index) > 0.0 {
            let strip = Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
            painter.rect_filled(strip, 0.0, theme::ACCENT);
        }
        if let Some(g) = layer.group {
            let tag = Rect::from_min_size(
                rect.min + Vec2::new(3.0, 0.0),
                Vec2::new(3.0, rect.height()),
            );
            painter.rect_filled(tag, 0.0, group_color(g));
        }
        if response.dnd_hover_payload::<EffectTemplate>().is_some() {
            painter.rect_stroke(
                rect,
                2.0,
                Stroke::new(2.0, theme::ACCENT),
                StrokeKind::Inside,
            );
        }

        let (mut bypass, mut solo, mut opacity, fader_side) =
            (layer.bypass, layer.solo, layer.opacity, layer.fader);
        let name = layer.name.clone();
        let blend = layer.blend.name();
        let inner = rect.shrink2(Vec2::new(8.0, 4.0));
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
            theme::ACCENT,
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
                    self.send_live(Command::EjectLayer(index));
                }
                let toggle = widgets::toggle(ui, &mut bypass, "B", theme::WARN, "Bypass");
                if !self.learnable(ui, toggle.rect, Action::Set(Param::LayerBypass(index)))
                    && toggle.changed()
                {
                    self.send(Command::SetLayerBypass {
                        layer: index,
                        bypass,
                    });
                }
                let toggle = widgets::toggle(ui, &mut solo, "S", theme::SOLO, "Solo");
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
                    if widgets::toggle(ui, &mut on, label, theme::ACCENT, "Crossfader side")
                        .changed()
                    {
                        let side = if on { side } else { FaderSide::None };
                        self.send(Command::SetLayerFader { layer: index, side });
                    }
                }
                ui.label(egui::RichText::new(blend).small().color(theme::TEXT_DIM));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(format!("{:.0}%", opacity * 100.0))
                            .small()
                            .monospace()
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        });
        if let Some(template) = response.dnd_release_payload::<EffectTemplate>() {
            self.add_effect(EffectTarget::Layer(index), &template);
            self.selected_layer = index;
            self.props_tab = PropsTab::Layer;
        } else if response.clicked() {
            self.selected_layer = index;
            self.props_tab = PropsTab::Layer;
        }
        let count = self.comp.layers.len();
        response.context_menu(|ui| {
            self.selected_layer = index;
            if ui
                .add(egui::Button::new("Duplicate").shortcut_text("Ctrl+D"))
                .clicked()
            {
                self.send(Command::DuplicateLayer(index));
            }
            if ui
                .add_enabled(index + 1 < count, egui::Button::new("Move Up"))
                .clicked()
            {
                self.send(Command::MoveLayer {
                    from: index,
                    to: index + 1,
                });
            }
            if ui
                .add_enabled(index > 0, egui::Button::new("Move Down"))
                .clicked()
            {
                self.send(Command::MoveLayer {
                    from: index,
                    to: index - 1,
                });
            }
            ui.separator();
            if ui
                .add_enabled(count > 1, egui::Button::new("Delete Layer"))
                .on_hover_text("Ctrl+Z restores it")
                .clicked()
            {
                self.send(Command::RemoveLayer(index));
            }
        });
    }

    fn cell(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let has_clip = self.comp.layers[layer].clips[column].is_some();
        let sense = if has_clip && !self.live_lock {
            Sense::click_and_drag()
        } else {
            Sense::click()
        };
        let (rect, response) = ui.allocate_exact_size(CELL, sense);
        if has_clip && !self.live_lock && response.dragged() {
            response.dnd_set_drag_payload(CellPayload(layer, column));
        }
        let clip = self.comp.layers[layer].clips[column].as_ref();
        let active = self.comp.layers[layer].active == Some(column);
        let selected = self.selected_clip == Some((layer, column));
        let drop_hover = response.dnd_hover_payload::<SourceTemplate>().is_some()
            || response
                .dnd_hover_payload::<CellPayload>()
                .is_some_and(|p| (p.0, p.1) != (layer, column))
            || (response.dnd_hover_payload::<EffectTemplate>().is_some() && clip.is_some());
        let painter = ui.painter_at(rect);
        self.cell_rects.push((rect, (layer, column)));

        let texture = clip.and_then(|clip| {
            if clip.source.is_procedural() {
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
                    theme::TEXT,
                );
                // Launch counter of this slot, bottom right over the name strip.
                let launches = self.comp.layers[layer].launch_count(column);
                if launches > 0 {
                    let galley = painter.layout_no_wrap(
                        launches.to_string(),
                        FontId::monospace(10.0),
                        theme::BG_DEEP,
                    );
                    let badge = Rect::from_min_size(
                        label.right_center() - Vec2::new(galley.size().x + 8.0, 6.5),
                        Vec2::new(galley.size().x + 6.0, 13.0),
                    );
                    painter.rect_filled(badge, 3.0, theme::ACCENT);
                    painter.galley(badge.center() - galley.size() / 2.0, galley, theme::BG_DEEP);
                }
            }
            (Some(clip), None) => {
                painter.rect_filled(rect, 2.0, theme::BG_RAISED);
                let failed = self.media.failed(&clip.source);
                painter.text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    if failed { "Missing" } else { "Loading…" },
                    FontId::proportional(10.5),
                    if failed { theme::WARN } else { theme::TEXT_DIM },
                );
                painter.text(
                    rect.left_bottom() + Vec2::new(4.0, -3.0),
                    Align2::LEFT_BOTTOM,
                    &clip.name,
                    FontId::proportional(10.5),
                    theme::TEXT_DIM,
                );
            }
            (None, _) => {
                painter.rect_filled(rect, 2.0, theme::BG_PANEL);
            }
        }

        let stroke = if drop_hover || active {
            Stroke::new(2.0, theme::ACCENT)
        } else if selected {
            Stroke::new(1.5, theme::SELECT)
        } else {
            Stroke::new(1.0, theme::LINE)
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
                theme::ACCENT,
                Stroke::NONE,
            ));
        }

        let launch_on_click = self.prefs.cell_click == crate::prefs::CellClick::Launch;
        let select_only = ui.input(|i| i.modifiers.command);
        if let Some(from) = response.dnd_release_payload::<CellPayload>() {
            if (from.0, from.1) != (layer, column) {
                // Alt (Option) copies; otherwise the clips move or swap.
                let copy = ui.input(|i| i.modifiers.alt);
                self.send(Command::MoveClip {
                    from: (from.0, from.1),
                    to: (layer, column),
                    copy,
                });
                self.selected_clip = Some((layer, column));
            }
        } else if let Some(template) = response.dnd_release_payload::<EffectTemplate>() {
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
        } else if response.clicked() || response.double_clicked() {
            self.selected_clip = Some((layer, column));
            self.selected_layer = layer;
            if self.comp.layers[layer].clips[column].is_some() {
                self.props_tab = PropsTab::Clip;
            }
            let launch = if launch_on_click {
                response.clicked() && !select_only
            } else {
                response.double_clicked()
            };
            if launch {
                self.send_live(Command::TriggerClip { layer, column });
            }
        }
        let hint = if launch_on_click {
            "Click: launch · Ctrl+click: select · drag: move (Alt: copy) · right-click: menu"
        } else {
            "Click: select · double-click: launch · drag: move (Alt: copy) · right-click: menu"
        };
        let response = response.on_hover_text_at_pointer(hint);
        response.context_menu(|ui| self.cell_menu(ui, layer, column));
    }

    fn cell_menu(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let has_clip = self.comp.layers[layer].clips[column].is_some();
        if ui
            .add_enabled(has_clip, egui::Button::new("Copy").shortcut_text("Ctrl+C"))
            .clicked()
        {
            self.selected_clip = Some((layer, column));
            self.copy_clip();
        }
        if ui
            .add_enabled(
                self.clipboard.is_some(),
                egui::Button::new("Paste").shortcut_text("Ctrl+V"),
            )
            .clicked()
        {
            self.selected_clip = Some((layer, column));
            self.paste_clip();
        }
        ui.separator();
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
        if ui
            .add_enabled(has_clip, egui::Button::new("Clear").shortcut_text("Del"))
            .clicked()
        {
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
            painter.rect_filled(track, 2.0, theme::BG_DEEP);
            painter.line_segment(
                [rect.center_top(), rect.center_bottom()],
                Stroke::new(1.0, theme::LINE),
            );
            let x = rect.left() + rect.width() * value;
            let handle =
                Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::new(10.0, 16.0));
            painter.rect_filled(handle, 2.0, theme::ACCENT);
            ui.label(egui::RichText::new("B").strong());
            ui.label(egui::RichText::new("Crossfader").color(theme::TEXT_DIM));
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
