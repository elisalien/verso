//! Groups in the clip grid: header rows, nesting stripes, collapse, layer
//! multi-selection and grouping commands.

use eframe::egui::{self, Align2, FontId, Rect, Sense, Stroke, StrokeKind, Vec2};
use verso_core::{Command, Composition};

use crate::app::{PropsTab, VersoApp};
use crate::effects_ui::group_color;
use crate::grid::{CELL, HEADER_WIDTH};
use crate::{theme, widgets};

pub const GROUP_ROW_HEIGHT: f32 = 30.0;
/// Width of one nesting stripe at the left of headers.
pub const STRIPE: f32 = 5.0;

/// One row of the grid, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    Group { group: usize, depth: usize },
    Layer { layer: usize },
}

/// Rows of the grid: top layer first, each group's header above its members,
/// members of collapsed groups left out.
pub fn rows(comp: &Composition) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut open: Vec<usize> = Vec::new();
    for layer in (0..comp.layers.len()).rev() {
        let path = comp.layer_path(layer);
        let common = open.iter().zip(&path).take_while(|(a, b)| a == b).count();
        open.truncate(common);
        for &group in &path[common..] {
            let hidden = open.iter().any(|&g| comp.groups[g].collapsed);
            if !hidden {
                rows.push(Row::Group {
                    group,
                    depth: open.len(),
                });
            }
            open.push(group);
        }
        if !open.iter().any(|&g| comp.groups[g].collapsed) {
            rows.push(Row::Layer { layer });
        }
    }
    rows
}

/// Draws one colored stripe per group in `path` at the left of `rect`, and
/// returns the remaining width's left edge.
pub fn paint_stripes(ui: &egui::Ui, rect: Rect, path: &[usize]) -> f32 {
    let painter = ui.painter();
    for (depth, &group) in path.iter().enumerate() {
        let stripe = Rect::from_min_size(
            rect.min + Vec2::new(depth as f32 * STRIPE, 0.0),
            Vec2::new(STRIPE - 1.0, rect.height()),
        );
        painter.rect_filled(stripe, 0.0, group_color(group).gamma_multiply(0.85));
    }
    rect.left() + path.len() as f32 * STRIPE
}

impl VersoApp {
    /// Layers a grouping command acts on: the selection, else the selected layer.
    pub(crate) fn layers_to_group(&self) -> Vec<usize> {
        if self.selected_layers.is_empty() {
            vec![self.selected_layer]
        } else {
            self.selected_layers.iter().copied().collect()
        }
    }

    /// Whether grouping the current selection would succeed.
    pub(crate) fn can_group(&self) -> bool {
        let mut trial = self.comp.clone();
        trial
            .apply(Command::AddGroup {
                layers: self.layers_to_group(),
            })
            .is_ok()
    }

    /// Ctrl+G: group the selected layers.
    pub(crate) fn group_selection(&mut self) {
        if !self.can_group() {
            self.status =
                "Select adjacent layers to group them (Ctrl/Shift-click layer headers).".into();
            return;
        }
        let index = self.comp.groups.len();
        self.send(Command::AddGroup {
            layers: self.layers_to_group(),
        });
        self.selected_group = Some(index);
        self.props_tab = PropsTab::Group;
    }

    /// Group the current selection belongs to, innermost first.
    pub(crate) fn current_group(&self) -> Option<usize> {
        self.selected_group
            .or_else(|| self.comp.layers.get(self.selected_layer)?.group)
    }

    /// Ctrl+Shift+G: dissolve the current group.
    pub(crate) fn ungroup_selection(&mut self) {
        match self.current_group() {
            Some(group) => {
                self.status = format!("Ungrouped {}", self.comp.groups[group].name);
                self.send(Command::RemoveGroup(group));
                self.selected_group = None;
            }
            None => self.status = "Nothing to ungroup.".into(),
        }
    }

    /// Click on a layer header: select it; Ctrl toggles it in the selection,
    /// Shift extends the selection from the last clicked layer.
    pub(crate) fn click_layer(&mut self, layer: usize, modifiers: egui::Modifiers) {
        if modifiers.shift {
            let (a, b) = (
                self.selected_layer.min(layer),
                self.selected_layer.max(layer),
            );
            self.selected_layers.extend(a..=b);
        } else if modifiers.command {
            if !self.selected_layers.remove(&layer) {
                self.selected_layers.insert(layer);
            }
            self.selected_layers.insert(self.selected_layer);
        } else {
            self.selected_layers.clear();
        }
        self.selected_layer = layer;
        self.selected_group = None;
        self.props_tab = PropsTab::Layer;
    }

    /// Items of the layer header's right-click menu about groups.
    pub(crate) fn layer_group_menu(&mut self, ui: &mut egui::Ui, layer: usize) {
        if !self.selected_layers.contains(&layer) && self.selected_layer != layer {
            self.selected_layer = layer;
            self.selected_layers.clear();
        }
        let count = self.layers_to_group().len();
        let label = if count > 1 {
            format!("Group {count} Layers")
        } else {
            "Group Layer".to_owned()
        };
        if ui
            .add_enabled(
                self.can_group(),
                egui::Button::new(label).shortcut_text("Ctrl+G"),
            )
            .on_disabled_hover_text("Only adjacent layers can be grouped")
            .clicked()
        {
            self.group_selection();
            ui.close();
        }
        if let Some(group) = self.comp.layers[layer].group {
            let parent = self.comp.groups[group].parent;
            let out = match parent {
                Some(p) => format!("Move Out to {}", self.comp.groups[p].name),
                None => format!("Remove from {}", self.comp.groups[group].name),
            };
            if ui.button(out).clicked() {
                self.send(Command::SetLayerGroup {
                    layer,
                    group: parent,
                });
                ui.close();
            }
            if ui
                .add(
                    egui::Button::new(format!("Ungroup {}", self.comp.groups[group].name))
                        .shortcut_text("Ctrl+Shift+G"),
                )
                .clicked()
            {
                self.send(Command::RemoveGroup(group));
                ui.close();
            }
        }
        ui.separator();
        let mut name = self.comp.layers[layer].name.clone();
        ui.label(egui::RichText::new("Name").small().color(theme::text_dim()));
        if ui.text_edit_singleline(&mut name).changed() {
            self.send(Command::RenameLayer { layer, name });
        }
        if ui
            .add_enabled(self.comp.layers.len() > 1, egui::Button::new("Delete Layer"))
            .on_hover_text("Deletes the layer and its clips (Ctrl+Z undoes)")
            .clicked()
        {
            self.send(Command::RemoveLayer(layer));
            ui.close();
        }
    }

    /// Header row of a group: stripes, collapse arrow, name, eject, bypass,
    /// solo, opacity; then one launch button per column for this group only.
    pub(crate) fn group_row(&mut self, ui: &mut egui::Ui, group: usize, depth: usize) {
        let g = self.comp.groups[group].clone();
        let size = Vec2::new(HEADER_WIDTH, GROUP_ROW_HEIGHT);
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        let color = group_color(group);
        let selected = self.props_tab == PropsTab::Group && self.current_group() == Some(group);
        ui.painter()
            .rect_filled(rect, 2.0, color.gamma_multiply(0.18));
        ui.painter().rect_stroke(
            rect,
            2.0,
            Stroke::new(
                if selected { 1.5 } else { 1.0 },
                if selected {
                    theme::select()
                } else {
                    color.gamma_multiply(0.6)
                },
            ),
            StrokeKind::Inside,
        );
        let mut path = self.comp.ancestors(Some(group));
        path.reverse();
        let left = paint_stripes(ui, rect, &path[..depth]);

        let inner = Rect::from_min_max(
            egui::pos2(left + 3.0, rect.top() + 3.0),
            rect.max - Vec2::new(6.0, 3.0),
        );
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(inner));
        let (mut bypass, mut solo, mut opacity) = (g.bypass, g.solo, g.opacity);
        child.horizontal_centered(|ui| {
            let arrow = if g.collapsed { "▸" } else { "▾" };
            if ui
                .add(egui::Button::new(arrow).frame(false))
                .on_hover_text(if g.collapsed {
                    "Show layers"
                } else {
                    "Hide layers"
                })
                .clicked()
            {
                self.send(Command::SetGroupCollapsed {
                    group,
                    collapsed: !g.collapsed,
                });
            }
            ui.label(egui::RichText::new(&g.name).strong().color(color));
            if widgets::square_button(ui, "X", "Eject every layer of the group").clicked() {
                let members: Vec<usize> = self.comp.group_layers(group).collect();
                for layer in members {
                    self.send(Command::EjectLayer(layer));
                }
            }
            if widgets::toggle(ui, &mut bypass, "B", theme::warn(), "Bypass group").changed() {
                self.send(Command::SetGroupBypass { group, bypass });
            }
            if widgets::toggle(ui, &mut solo, "S", theme::solo(), "Solo group").changed() {
                self.send(Command::SetGroupSolo { group, solo });
            }
            let width = ui.available_width().max(30.0);
            if widgets::fader(ui, &mut opacity, Vec2::new(width, 16.0), color, 1.0).changed() {
                self.send(Command::SetGroupOpacity { group, opacity });
            }
        });

        if response.clicked() {
            if ui.input(|i| i.modifiers.command) {
                self.selected_layers.extend(self.comp.group_layers(group));
            }
            self.selected_group = Some(group);
            if let Some(first) = self.comp.group_layers(group).last() {
                self.selected_layer = first;
            }
            self.props_tab = PropsTab::Group;
        }
        if response.double_clicked() {
            self.send(Command::SetGroupCollapsed {
                group,
                collapsed: !g.collapsed,
            });
        }
        response.context_menu(|ui| {
            let mut name = g.name.clone();
            ui.label(egui::RichText::new("Name").small().color(theme::text_dim()));
            if ui.text_edit_singleline(&mut name).changed() {
                self.send(Command::RenameGroup { group, name });
            }
            ui.separator();
            if ui.button("Select Layers").clicked() {
                self.selected_layers = self.comp.group_layers(group).collect();
                ui.close();
            }
            let collapse = if g.collapsed { "Expand" } else { "Collapse" };
            if ui.button(collapse).clicked() {
                self.send(Command::SetGroupCollapsed {
                    group,
                    collapsed: !g.collapsed,
                });
                ui.close();
            }
            if ui
                .add(egui::Button::new("Ungroup").shortcut_text("Ctrl+Shift+G"))
                .clicked()
            {
                self.send(Command::RemoveGroup(group));
                self.selected_group = None;
                ui.close();
            }
        });

        // Column launch buttons for this group only.
        for column in 0..self.comp.columns.len() {
            let (cell, response) =
                ui.allocate_exact_size(Vec2::new(CELL.x, GROUP_ROW_HEIGHT), Sense::click());
            let playing = self
                .comp
                .group_layers(group)
                .any(|l| self.comp.layers[l].active == Some(column));
            let fill = if response.hovered() {
                color.gamma_multiply(0.35)
            } else {
                color.gamma_multiply(0.12)
            };
            ui.painter().rect_filled(cell, 2.0, fill);
            if playing {
                let bar = Rect::from_min_size(
                    cell.left_bottom() - Vec2::new(0.0, 2.0),
                    Vec2::new(cell.width(), 2.0),
                );
                ui.painter().rect_filled(bar, 0.0, color);
            }
            ui.painter().text(
                cell.center(),
                Align2::CENTER_CENTER,
                "▶",
                FontId::proportional(11.0),
                if response.hovered() {
                    theme::text()
                } else {
                    theme::text_dim()
                },
            );
            let learning = self.learnable(
                ui,
                cell,
                verso_core::control::Action::LaunchGroupColumn { group, column },
            );
            if response
                .on_hover_text(format!(
                    "Launch {} in {}",
                    self.comp.columns[column].name, g.name
                ))
                .clicked()
                && !learning
            {
                self.send(Command::LaunchGroupColumn { group, column });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_put_headers_above_members_and_hide_collapsed() {
        let mut comp = Composition::demo_with_layers(5);
        comp.apply(Command::AddGroup {
            layers: vec![1, 2, 3],
        })
        .unwrap(); // 0
        comp.apply(Command::AddGroup { layers: vec![2, 3] })
            .unwrap(); // 1 in 0
        assert_eq!(
            rows(&comp),
            [
                Row::Layer { layer: 4 },
                Row::Group { group: 0, depth: 0 },
                Row::Group { group: 1, depth: 1 },
                Row::Layer { layer: 3 },
                Row::Layer { layer: 2 },
                Row::Layer { layer: 1 },
                Row::Layer { layer: 0 },
            ]
        );
        comp.apply(Command::SetGroupCollapsed {
            group: 1,
            collapsed: true,
        })
        .unwrap();
        assert!(!rows(&comp).contains(&Row::Layer { layer: 3 }));
        assert!(rows(&comp).contains(&Row::Group { group: 1, depth: 1 }));
        comp.apply(Command::SetGroupCollapsed {
            group: 0,
            collapsed: true,
        })
        .unwrap();
        assert_eq!(
            rows(&comp),
            [
                Row::Layer { layer: 4 },
                Row::Group { group: 0, depth: 0 },
                Row::Layer { layer: 0 },
            ]
        );
    }
}
