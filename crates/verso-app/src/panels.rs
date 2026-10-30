//! Lower panels: properties, preview and output monitors, source browser.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, TextureId, Vec2};
use verso_core::{BlendMode, Command, FaderSide, Generator, Source};

use crate::app::{PropsTab, VersoApp};
use crate::grid::SourceTemplate;
use crate::{theme, widgets};

/// Draws `texture` as large as fits in `size`, letterboxed on black.
fn monitor_image(ui: &mut egui::Ui, texture: Option<TextureId>, size: Vec2, aspect: f32) {
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
    painter.rect_stroke(rect, 0.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
}

fn property_grid(ui: &mut egui::Ui, id: &str, add: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(id)
        .num_columns(2)
        .spacing([10.0, 6.0])
        .min_col_width(90.0)
        .show(ui, add);
}

impl VersoApp {
    pub(crate) fn properties(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "Properties");
        widgets::tabs(
            ui,
            &mut self.props_tab,
            &[
                (PropsTab::Composition, "Composition"),
                (PropsTab::Layer, "Layer"),
                (PropsTab::Clip, "Clip"),
            ],
        );
        ui.separator();
        egui::ScrollArea::vertical().show(ui, |ui| match self.props_tab {
            PropsTab::Composition => self.composition_properties(ui),
            PropsTab::Layer => self.layer_properties(ui),
            PropsTab::Clip => self.clip_properties(ui),
        });
    }

    fn composition_properties(&mut self, ui: &mut egui::Ui) {
        property_grid(ui, "composition", |ui| {
            ui.label("Name");
            ui.text_edit_singleline(&mut self.comp.name);
            ui.end_row();

            ui.label("Resolution");
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.comp.width).range(16..=8192));
                ui.label("×");
                ui.add(egui::DragValue::new(&mut self.comp.height).range(16..=8192));
            });
            ui.end_row();

            ui.label("Master");
            let mut opacity = self.comp.master.opacity;
            if widgets::fader(ui, &mut opacity, Vec2::new(200.0, 18.0), theme::ACCENT, 1.0)
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

    fn layer_properties(&mut self, ui: &mut egui::Ui) {
        let index = self.selected_layer;
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
            if widgets::fader(ui, &mut opacity, Vec2::new(200.0, 18.0), theme::ACCENT, 1.0)
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
                if widgets::toggle(ui, &mut bypass, "B", theme::WARN, "Bypass").changed() {
                    self.send(Command::SetLayerBypass {
                        layer: index,
                        bypass,
                    });
                }
                if widgets::toggle(ui, &mut solo, "S", theme::SOLO, "Solo").changed() {
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
        });
    }

    fn clip_properties(&mut self, ui: &mut egui::Ui) {
        let Some((layer, column)) = self.selected_clip else {
            ui.label(egui::RichText::new("Select a clip.").color(theme::TEXT_DIM));
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
                    .color(theme::TEXT_DIM),
            );
            return;
        };
        let before = clip.clone();
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

    pub(crate) fn monitors(&mut self, ui: &mut egui::Ui) {
        let aspect = self.comp.width as f32 / self.comp.height.max(1) as f32;
        let spacing = ui.spacing().item_spacing.x;
        let width = ((ui.available_width() - spacing) / 2.0).max(40.0);
        let height = (ui.available_height() - 22.0).max(40.0);
        let clip_texture = self.selected_clip.and(self.monitor_texture);
        let output_texture = self.output_texture;
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                widgets::header(ui, "Preview");
                monitor_image(ui, clip_texture, Vec2::new(width, height), aspect);
            });
            ui.vertical(|ui| {
                ui.set_width(width);
                widgets::header(ui, "Output");
                monitor_image(ui, Some(output_texture), Vec2::new(width, height), aspect);
            });
        });
    }

    pub(crate) fn browser(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "Sources");
        ui.label(
            egui::RichText::new(
                "Drag onto a clip slot, or double-click to load into the selected slot.",
            )
            .small()
            .color(theme::TEXT_DIM),
        );
        ui.add_space(4.0);
        for template in SourceTemplate::all() {
            let id = egui::Id::unique(("verso-source", &template.name));
            let name = template.name.clone();
            let response = ui
                .dnd_drag_source(id, template.clone(), |ui| {
                    let (rect, response) = ui
                        .allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::click());
                    let fill = if response.hovered() {
                        theme::BG_RAISED
                    } else {
                        theme::BG_PANEL
                    };
                    ui.painter().rect_filled(rect, 2.0, fill);
                    ui.painter().text(
                        rect.left_center() + Vec2::new(8.0, 0.0),
                        egui::Align2::LEFT_CENTER,
                        &name,
                        egui::FontId::proportional(12.0),
                        theme::TEXT,
                    );
                    response
                })
                .inner;
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
    }
}
