//! Clip grid: composition row, column headers, one row per layer.

use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use verso_core::{Clip, Command, FaderSide, Generator, Source};

use crate::app::{PropsTab, VersoApp};
use crate::{theme, widgets};

pub const CELL: Vec2 = Vec2::new(112.0, 63.0);
const HEADER_WIDTH: f32 = 236.0;
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

    pub fn clip(&self) -> Clip {
        Clip {
            name: self.name.clone(),
            source: self.source.clone(),
        }
    }
}

impl VersoApp {
    pub(crate) fn grid(&mut self, ui: &mut egui::Ui) {
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
        let mut child =
            ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink2(Vec2::new(3.0, 2.0))));
        child.horizontal_centered(|ui| {
            if widgets::square_button(ui, "X", "Clear all layers").clicked() {
                self.send(Command::ClearAll);
            }
            ui.label(egui::RichText::new("Composition").strong());
            let mut opacity = self.comp.master.opacity;
            if widgets::fader(
                ui,
                &mut opacity,
                Vec2::new(ui.available_width(), 16.0),
                theme::ACCENT,
                1.0,
            )
            .changed()
            {
                self.send(Command::SetMasterOpacity(opacity));
            }
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
        let response = response.on_hover_text("Click to launch the column");
        if response.clicked() {
            self.send(Command::TriggerColumn(column));
        }
        response.context_menu(|ui| {
            let mut name = self.comp.columns[column].name.clone();
            ui.label("Column name");
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameColumn { column, name });
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
        // Playing indicator on the left edge.
        if self.comp.layer_gain(index) > 0.0 {
            let strip = Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height()));
            painter.rect_filled(strip, 0.0, theme::ACCENT);
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
        if widgets::vfader(
            &mut fader_ui,
            &mut opacity,
            fader_rect.size(),
            theme::ACCENT,
            1.0,
        )
        .changed()
        {
            self.send(Command::SetLayerOpacity {
                layer: index,
                opacity,
            });
        }

        let controls = Rect::from_min_max(
            inner.min,
            egui::pos2(fader_rect.left() - 6.0, inner.bottom()),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(controls));
        child.vertical(|ui| {
            ui.horizontal(|ui| {
                if widgets::square_button(ui, "X", "Eject the playing clip").clicked() {
                    self.send(Command::EjectLayer(index));
                }
                if widgets::toggle(ui, &mut bypass, "B", theme::WARN, "Bypass").changed() {
                    self.send(Command::SetLayerBypass {
                        layer: index,
                        bypass,
                    });
                }
                if widgets::toggle(ui, &mut solo, "S", theme::SOLO, "Solo").changed() {
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
        if response.clicked() {
            self.selected_layer = index;
            self.props_tab = PropsTab::Layer;
        }
    }

    fn cell(&mut self, ui: &mut egui::Ui, layer: usize, column: usize) {
        let (rect, response) = ui.allocate_exact_size(CELL, Sense::click());
        let clip = self.comp.layers[layer].clips[column].as_ref();
        let active = self.comp.layers[layer].active == Some(column);
        let selected = self.selected_clip == Some((layer, column));
        let drop_hover = response.dnd_hover_payload::<SourceTemplate>().is_some();
        let painter = ui.painter_at(rect);

        match (clip, self.cell_textures.get(&(layer, column))) {
            (Some(clip), Some(&texture)) => {
                let image = rect.shrink(1.0);
                let tint = if active || response.hovered() {
                    Color32::WHITE
                } else {
                    Color32::from_gray(150)
                };
                painter.image(
                    texture,
                    image,
                    Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                    tint,
                );
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
            }
            (Some(_), None) => {
                painter.rect_filled(rect, 2.0, theme::BG_RAISED);
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

        if let Some(template) = response.dnd_release_payload::<SourceTemplate>() {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(template.clip()),
            });
            self.selected_clip = Some((layer, column));
            self.props_tab = PropsTab::Clip;
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
            if value != self.comp.master.crossfader {
                self.send(Command::SetCrossfader(value));
            }
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
