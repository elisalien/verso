//! Blend mode pickers grouped by category, and the fine parameters of
//! enhanced modes.

use eframe::egui;
use verso_core::control::{Action, Param};
use verso_core::{BlendCategory, BlendMode, BlendParamKind, BlendParams, Command};

use crate::app::VersoApp;
use crate::theme;

/// Whose blend is edited.
#[derive(Debug, Clone, Copy)]
pub(crate) enum BlendOwner {
    Layer(usize),
    Group(usize),
}

impl BlendOwner {
    fn param(self, name: &str) -> Param {
        match self {
            BlendOwner::Layer(layer) => Param::LayerBlend {
                layer,
                name: name.to_owned(),
            },
            BlendOwner::Group(group) => Param::GroupBlend {
                group,
                name: name.to_owned(),
            },
        }
    }

    fn set_all(self, params: BlendParams) -> Command {
        match self {
            BlendOwner::Layer(layer) => Command::SetLayerBlendParams { layer, params },
            BlendOwner::Group(group) => Command::SetGroupBlendParams { group, params },
        }
    }
}

/// Small category header in a blend mode list.
fn category_header(ui: &mut egui::Ui, category: BlendCategory) {
    ui.label(
        egui::RichText::new(category.name())
            .small()
            .color(theme::text_dim()),
    );
}

/// Blend modes as selectable items under category headers, for a combo box
/// or a menu. Returns the mode clicked.
pub(crate) fn blend_mode_items(ui: &mut egui::Ui, current: BlendMode) -> Option<BlendMode> {
    let mut picked = None;
    for (i, category) in BlendCategory::ALL.into_iter().enumerate() {
        if i > 0 {
            ui.separator();
        }
        category_header(ui, category);
        for mode in category.modes() {
            if ui.selectable_label(current == mode, mode.name()).clicked() {
                picked = Some(mode);
            }
        }
    }
    picked
}

/// Combo box choosing a blend mode, grouped by category.
pub(crate) fn blend_mode_combo(
    ui: &mut egui::Ui,
    id: &str,
    current: BlendMode,
) -> Option<BlendMode> {
    let mut picked = None;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.name())
        .height(400.0)
        .show_ui(ui, |ui| picked = blend_mode_items(ui, current));
    picked.filter(|m| *m != current)
}

impl VersoApp {
    /// Rows of a property grid with the fine parameters of `mode` and a
    /// Reset button; nothing for plain modes. Sliders can be MIDI-learned and
    /// modulated (right click), like other parameters.
    pub(crate) fn blend_param_rows(
        &mut self,
        ui: &mut egui::Ui,
        owner: BlendOwner,
        mode: BlendMode,
        params: &BlendParams,
    ) {
        if !mode.is_enhanced() {
            return;
        }
        for spec in mode.params() {
            let current = params.get(mode, spec.name).unwrap_or(spec.default);
            let mut value = current;
            ui.label(spec.label).on_hover_text(spec.help);
            let response = match spec.kind {
                BlendParamKind::Choice(options) => {
                    let selected = options.get(value as usize).copied().unwrap_or_default();
                    egui::ComboBox::from_id_salt(("blend param", spec.name, owner_key(owner)))
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            for (i, option) in options.iter().enumerate() {
                                ui.selectable_value(&mut value, i as f32, *option);
                            }
                        })
                        .response
                }
                BlendParamKind::Angle => ui.add(
                    egui::Slider::new(&mut value, spec.min..=spec.max)
                        .suffix("°")
                        .fixed_decimals(0),
                ),
                BlendParamKind::Float => ui.add(egui::Slider::new(&mut value, spec.min..=spec.max)),
            };
            let response = response.on_hover_text(spec.help);
            let control = owner.param(spec.name);
            let learning = self.learnable(ui, response.rect, Action::Set(control.clone()));
            if value != current && !learning {
                self.send(control.command(value, spec_range(spec)));
            }
            self.modulation_menu(&response, control);
            ui.end_row();
        }
        ui.label("");
        let at_default = params.is_mode_default(mode);
        if ui
            .add_enabled(!at_default, egui::Button::new("Reset"))
            .on_hover_text(format!("Back to the defaults of {}", mode.name()))
            .clicked()
        {
            let mut params = params.clone();
            params.reset(mode);
            self.send(owner.set_all(params));
        }
        ui.end_row();
    }
}

fn owner_key(owner: BlendOwner) -> (u8, usize) {
    match owner {
        BlendOwner::Layer(l) => (0, l),
        BlendOwner::Group(g) => (1, g),
    }
}

fn spec_range(spec: &verso_core::BlendParamSpec) -> verso_core::control::Range {
    verso_core::control::Range {
        min: spec.min,
        max: spec.max,
        toggle: spec.is_toggle(),
    }
}
