//! Effect chains in the Properties panel and the Effects browser tab.

use eframe::egui::{self, Color32, Sense, Vec2};
use verso_core::control::{Action, Param};
use verso_core::{Command, Effect, EffectEdit, EffectTarget, ParamValue};
use verso_render::{ParamKind, ShaderKind};

use crate::app::{PropsTab, VersoApp};
use crate::{theme, widgets};

/// An effect dragged from the browser.
#[derive(Clone)]
pub struct EffectTemplate {
    pub id: String,
    pub name: String,
}

impl EffectTemplate {
    pub fn effect(&self) -> Effect {
        Effect::new(&self.id, &self.name)
    }
}

/// Color of a group's tag, by group index.
pub fn group_color(group: usize) -> Color32 {
    const COLORS: [Color32; 6] = [
        Color32::from_rgb(232, 150, 64),
        Color32::from_rgb(160, 110, 230),
        Color32::from_rgb(90, 200, 120),
        Color32::from_rgb(230, 90, 150),
        Color32::from_rgb(220, 210, 90),
        Color32::from_rgb(110, 170, 240),
    ];
    COLORS[group % COLORS.len()]
}

impl VersoApp {
    pub(crate) fn add_effect(&mut self, target: EffectTarget, template: &EffectTemplate) {
        self.send(Command::EditEffects {
            target,
            edit: EffectEdit::Add(template.effect()),
        });
    }

    /// Target of the effect chain shown in the current Properties tab.
    pub(crate) fn props_target(&self) -> Option<EffectTarget> {
        match self.props_tab {
            PropsTab::Composition => Some(EffectTarget::Composition),
            PropsTab::Layer => Some(EffectTarget::Layer(self.selected_layer)),
            PropsTab::Group => self
                .comp
                .layers
                .get(self.selected_layer)?
                .group
                .map(EffectTarget::Group),
            PropsTab::Clip => {
                let (layer, column) = self.selected_clip?;
                self.comp.layers[layer].clips[column]
                    .is_some()
                    .then_some(EffectTarget::Clip { layer, column })
            }
        }
    }

    /// Effect list with parameters, plus an "Add Effect" menu.
    pub(crate) fn effect_chain(&mut self, ui: &mut egui::Ui, target: EffectTarget) {
        ui.add_space(8.0);
        widgets::header(ui, "Effects");
        let Some(chain) = self.comp.effects(target).cloned() else {
            return;
        };
        let edit = |edit| Command::EditEffects { target, edit };
        let count = chain.len();
        for (index, effect) in chain.into_iter().enumerate() {
            egui::Frame::NONE
                .fill(theme::bg_header())
                .inner_margin(egui::Margin::same(4))
                .corner_radius(2)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let mut on = !effect.bypass;
                        if widgets::toggle(ui, &mut on, "●", theme::accent(), "On / bypass")
                            .changed()
                        {
                            self.send(edit(EffectEdit::SetBypass { index, bypass: !on }));
                        }
                        let def = self.renderer.library().get(&effect.id);
                        let label = egui::RichText::new(&effect.name).strong();
                        ui.label(if def.is_some() {
                            label
                        } else {
                            label.color(theme::warn())
                        })
                        .on_hover_text(&effect.id);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if widgets::square_button(ui, "X", "Remove").clicked() {
                                self.send(edit(EffectEdit::Remove(index)));
                            }
                            if index + 1 < count
                                && widgets::square_button(ui, "↓", "Move down").clicked()
                            {
                                self.send(edit(EffectEdit::Move {
                                    from: index,
                                    to: index + 1,
                                }));
                            }
                            if index > 0 && widgets::square_button(ui, "↑", "Move up").clicked() {
                                self.send(edit(EffectEdit::Move {
                                    from: index,
                                    to: index - 1,
                                }));
                            }
                        });
                    });
                    let Some(def) = self.renderer.library().get(&effect.id).cloned() else {
                        ui.label(
                            egui::RichText::new("Effect not found: it is skipped.")
                                .small()
                                .color(theme::warn()),
                        );
                        return;
                    };
                    let labels =
                        widgets::label_width(ui, def.params.iter().map(|p| p.name.as_str()));
                    widgets::fit_sliders(ui, labels, 10.0);
                    egui::Grid::new(("effect", target_key(target), index))
                        .num_columns(2)
                        .spacing([10.0, 4.0])
                        .show(ui, |ui| {
                            for param in &def.params {
                                ui.label(&param.name);
                                let value = def.value(&effect, param);
                                let (response, changed) = param_widget(ui, &param.kind, value);
                                let controllable = matches!(
                                    param.kind,
                                    ParamKind::Float { .. } | ParamKind::Bool { .. }
                                );
                                let control = Param::Effect {
                                    target,
                                    index,
                                    name: param.name.clone(),
                                };
                                let learning = controllable
                                    && self.learnable(
                                        ui,
                                        response.rect,
                                        Action::Set(control.clone()),
                                    );
                                if let Some(value) = changed.filter(|_| !learning) {
                                    self.send(edit(EffectEdit::SetParam {
                                        index,
                                        name: param.name.clone(),
                                        value,
                                    }));
                                }
                                if controllable {
                                    self.modulation_menu(&response, control);
                                }
                                ui.end_row();
                            }
                        });
                });
            ui.add_space(3.0);
        }

        ui.menu_button("+ Add Effect", |ui| {
            let defs: Vec<_> = self
                .renderer
                .library()
                .sorted_kind(ShaderKind::Filter)
                .into_iter()
                .map(|d| {
                    (
                        d.category.clone(),
                        EffectTemplate {
                            id: d.id.clone(),
                            name: d.name.clone(),
                        },
                    )
                })
                .collect();
            let mut category = None;
            for (cat, template) in defs {
                if category.as_ref() != Some(&cat) {
                    ui.label(egui::RichText::new(&cat).small().color(theme::text_dim()));
                    category = Some(cat);
                }
                if ui.button(&template.name).clicked() {
                    self.add_effect(target, &template);
                    ui.close();
                }
            }
        });
    }

    pub(crate) fn effect_browser(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("Load Folder…")
                .on_hover_text("Load .wgsl (Verso) and .fs (ISF) effects from a folder")
                .clicked()
            {
                self.file_request(crate::dialogs::FileRequest::EffectsFolder);
            }
        });
        ui.label(
            egui::RichText::new(
                "Drag onto a clip, a layer header or the Composition header. Double-click adds to the Properties target.",
            )
            .small()
            .color(theme::text_dim()),
        );
        let defs: Vec<_> = self
            .renderer
            .library()
            .sorted_kind(ShaderKind::Filter)
            .into_iter()
            .map(|d| {
                (
                    d.category.clone(),
                    EffectTemplate {
                        id: d.id.clone(),
                        name: d.name.clone(),
                    },
                )
            })
            .collect();
        egui::ScrollArea::vertical().show(ui, |ui| {
            let mut category = None;
            for (cat, template) in defs {
                if category.as_ref() != Some(&cat) {
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(&cat).small().color(theme::text_dim()));
                    category = Some(cat);
                }
                let id = egui::Id::unique(("verso-effect", &template.id));
                let name = template.name.clone();
                let response = ui
                    .dnd_drag_source(id, template.clone(), |ui| {
                        let (rect, response) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 22.0),
                            Sense::click(),
                        );
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
                        response
                    })
                    .inner;
                if response.double_clicked()
                    && let Some(target) = self.props_target()
                {
                    self.add_effect(target, &template);
                }
            }
        });
    }
}

fn target_key(target: EffectTarget) -> (u8, usize, usize) {
    match target {
        EffectTarget::Composition => (0, 0, 0),
        EffectTarget::Group(g) => (1, g, 0),
        EffectTarget::Layer(l) => (2, l, 0),
        EffectTarget::Clip { layer, column } => (3, layer, column),
    }
}

/// Edits one parameter; returns its widget and the new value when it changed.
pub(crate) fn param_widget(
    ui: &mut egui::Ui,
    kind: &ParamKind,
    value: ParamValue,
) -> (egui::Response, Option<ParamValue>) {
    let result = |r: egui::Response, v: ParamValue| {
        let new = r.changed().then_some(v);
        (r, new)
    };
    match (kind, value) {
        (ParamKind::Float { min, max, .. }, ParamValue::Float(mut v)) => {
            let r = ui.add(egui::Slider::new(&mut v, *min..=*max));
            result(r, ParamValue::Float(v))
        }
        (ParamKind::Bool { .. }, ParamValue::Bool(mut v)) => {
            let r = ui.checkbox(&mut v, "");
            result(r, ParamValue::Bool(v))
        }
        (ParamKind::Color { .. }, ParamValue::Color(mut v)) => {
            let r = ui.color_edit_button_rgba_unmultiplied(&mut v);
            result(r, ParamValue::Color(v))
        }
        (ParamKind::Point { min, max, .. }, ParamValue::Point(mut v)) => {
            let inner = ui.horizontal(|ui| {
                let x = ui.add(
                    egui::DragValue::new(&mut v[0])
                        .range(*min..=*max)
                        .speed(0.01)
                        .prefix("x "),
                );
                let y = ui.add(
                    egui::DragValue::new(&mut v[1])
                        .range(*min..=*max)
                        .speed(0.01)
                        .prefix("y "),
                );
                x.changed() || y.changed()
            });
            let new = inner.inner.then_some(ParamValue::Point(v));
            (inner.response, new)
        }
        _ => (ui.label("?"), None),
    }
}
