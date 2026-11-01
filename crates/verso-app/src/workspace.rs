//! The window layout: which panels exist, how they are docked, and which
//! group, layer or clip each Properties panel shows.

use std::collections::BTreeMap;
use std::path::PathBuf;

use eframe::egui;
use serde::{Deserialize, Serialize};
use verso_core::EffectTarget;

use crate::app::{PropsTab, VersoApp};
use crate::dock::{Dock, Node, PanelId};
use crate::{theme, widgets};

/// What a Properties panel edits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropKind {
    Composition,
    Group,
    Layer,
    Clip,
}

impl PropKind {
    fn name(self) -> &'static str {
        match self {
            PropKind::Composition => "Composition",
            PropKind::Group => "Group",
            PropKind::Layer => "Layer",
            PropKind::Clip => "Clip",
        }
    }

    pub fn of_tab(tab: PropsTab) -> PropKind {
        match tab {
            PropsTab::Composition => PropKind::Composition,
            PropsTab::Group => PropKind::Group,
            PropsTab::Layer => PropKind::Layer,
            PropsTab::Clip => PropKind::Clip,
        }
    }
}

/// A fixed target for a Properties panel, instead of following the selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Lock {
    Group(usize),
    Layer(usize),
    Clip(usize, usize),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PanelKind {
    Grid,
    Preview,
    Output,
    Browser,
    Properties { kind: PropKind, lock: Option<Lock> },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub dock: Dock,
    pub panels: BTreeMap<PanelId, PanelKind>,
    /// Undocked panels, each in its own window.
    #[serde(default)]
    pub floating: Vec<PanelId>,
    next: u32,
}

fn layout_file() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("layout.ron"))
}

/// Layout picked with Window › Save Layout as Default.
fn default_layout_file() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("default-layout.ron"))
}

fn read_layout(file: Option<PathBuf>) -> Option<Workspace> {
    file.and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| ron::from_str::<Workspace>(&text).ok())
        .filter(|w| w.is_consistent())
}

fn write_layout(file: Option<PathBuf>, workspace: &Workspace) -> std::io::Result<()> {
    let file = file.ok_or_else(|| std::io::Error::other("no user folder"))?;
    let text = ron::to_string(workspace).map_err(std::io::Error::other)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(file, text)
}

impl Default for Workspace {
    /// Grid on top; below, Properties tabs, Preview, Output and Browser.
    fn default() -> Self {
        let mut panels = BTreeMap::new();
        let mut add = |kind: PanelKind| {
            let id = PanelId(panels.len() as u32);
            panels.insert(id, kind);
            id
        };
        let grid = add(PanelKind::Grid);
        let props: Vec<PanelId> = [
            PropKind::Composition,
            PropKind::Group,
            PropKind::Layer,
            PropKind::Clip,
        ]
        .into_iter()
        .map(|kind| add(PanelKind::Properties { kind, lock: None }))
        .collect();
        let preview = add(PanelKind::Preview);
        let output = add(PanelKind::Output);
        let browser = add(PanelKind::Browser);
        let monitors = Node::split(false, 0.5, Node::tabs(&[preview]), Node::tabs(&[output]));
        let right = Node::split(false, 0.68, monitors, Node::tabs(&[browser]));
        let bottom = Node::split(false, 0.36, Node::tabs(&props), right);
        let next = panels.len() as u32;
        Self {
            dock: Dock::new(Node::split(true, 0.6, Node::tabs(&[grid]), bottom)),
            panels,
            floating: Vec::new(),
            next,
        }
    }
}

impl Workspace {
    /// The layout of the last session, else the user's default.
    pub fn load() -> Self {
        read_layout(layout_file()).unwrap_or_else(Self::user_default)
    }

    /// The layout saved as default, else the built-in one.
    pub fn user_default() -> Self {
        read_layout(default_layout_file()).unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = write_layout(layout_file(), self);
    }

    /// Makes this layout the one Reset Layout and new installs start from.
    pub fn save_as_default(&self) -> std::io::Result<()> {
        write_layout(default_layout_file(), self)
    }

    /// The layout as stored in a composition file.
    pub fn to_text(&self) -> Option<String> {
        ron::to_string(self).ok()
    }

    /// A layout stored in a composition file, if it is usable.
    pub fn from_text(text: &str) -> Option<Self> {
        ron::from_str::<Workspace>(text)
            .ok()
            .filter(|w| w.is_consistent())
    }

    /// Every docked panel is known and the grid is present.
    fn is_consistent(&self) -> bool {
        let mut shown = self.dock.panels();
        shown.extend(&self.floating);
        shown.iter().all(|id| self.panels.contains_key(id))
            && shown.iter().any(|id| self.panels[id] == PanelKind::Grid)
    }

    /// Puts an undocked panel back into the layout.
    pub fn redock(&mut self, id: PanelId) {
        self.floating.retain(|&f| f != id);
        self.dock.add_beside(id, None);
    }

    pub fn add(&mut self, kind: PanelKind, beside: Option<PanelId>) -> PanelId {
        let id = PanelId(self.next);
        self.next += 1;
        self.panels.insert(id, kind);
        self.dock.add_beside(id, beside);
        id
    }

    fn find(&self, wanted: impl Fn(&PanelKind) -> bool) -> Option<PanelId> {
        self.dock
            .panels()
            .into_iter()
            .find(|id| wanted(&self.panels[id]))
    }

    /// Brings forward a Properties panel of `kind` that follows the selection.
    pub fn focus(&mut self, kind: PropKind) {
        let following = |p: &PanelKind| matches!(p, PanelKind::Properties { kind: k, lock: None } if *k == kind);
        if let Some(id) = self.find(following) {
            self.dock.activate(id);
        }
    }

    /// Opens a new Properties panel next to the other Properties panels.
    pub fn open_properties(&mut self, kind: PropKind, lock: Option<Lock>) -> PanelId {
        let beside = self.find(|p| matches!(p, PanelKind::Properties { .. }));
        self.add(PanelKind::Properties { kind, lock }, beside)
    }

    /// Reopens a single-instance panel, or shows it if it is open.
    pub fn show_panel(&mut self, kind: PanelKind) {
        if self.floating.iter().any(|id| self.panels[id] == kind) {
            return;
        }
        match self.find(|p| *p == kind) {
            Some(id) => {
                self.dock.activate(id);
            }
            None => {
                let beside = self.find(|p| matches!(p, PanelKind::Preview | PanelKind::Output));
                self.add(kind, beside);
            }
        }
    }

    fn closed(&mut self, ids: &[PanelId]) {
        for id in ids {
            self.panels.remove(id);
        }
    }
}

/// Label of a clip slot for target lists.
fn clip_label(app: &VersoApp, layer: usize, column: usize) -> String {
    let name = app.comp.layers[layer].clips[column]
        .as_ref()
        .map(|c| c.name.as_str())
        .unwrap_or("Empty");
    format!(
        "{} · {} · {name}",
        app.comp.layers[layer].name, app.comp.columns[column].name
    )
}

impl VersoApp {
    /// Target a Properties panel shows now: its lock, else the selection.
    fn resolve(&self, kind: PropKind, lock: Option<Lock>) -> Option<Lock> {
        match (kind, lock) {
            (PropKind::Composition, _) => None,
            (_, Some(lock)) => Some(lock),
            (PropKind::Group, None) => self.current_group().map(Lock::Group),
            (PropKind::Layer, None) => Some(Lock::Layer(self.selected_layer)),
            (PropKind::Clip, None) => self.selected_clip.map(|(l, c)| Lock::Clip(l, c)),
        }
    }

    fn target_label(&self, target: Option<Lock>) -> String {
        match target {
            // Nested groups show their whole path.
            Some(Lock::Group(g)) => (g < self.comp.groups.len()).then(|| {
                let mut path = self.comp.ancestors(Some(g));
                path.reverse();
                path.iter()
                    .map(|&p| self.comp.groups[p].name.as_str())
                    .collect::<Vec<_>>()
                    .join(" › ")
            }),
            Some(Lock::Layer(l)) => self.comp.layers.get(l).map(|l| l.name.clone()),
            Some(Lock::Clip(l, c)) => (l < self.comp.layers.len() && c < self.comp.columns.len())
                .then(|| clip_label(self, l, c)),
            None => None,
        }
        .unwrap_or_else(|| "None".into())
    }

    fn panel_title(&self, kind: &PanelKind) -> String {
        match kind {
            PanelKind::Grid => "Clips".into(),
            PanelKind::Preview => "Preview".into(),
            PanelKind::Output => "Output".into(),
            PanelKind::Browser => "Browser".into(),
            PanelKind::Properties { kind, lock: None } => kind.name().into(),
            PanelKind::Properties { kind, lock } => {
                format!("{} 🔒 {}", kind.name(), self.target_label(*lock))
            }
        }
    }

    /// Draws the docked panels in the space left by the menu and status bars.
    pub(crate) fn workspace(&mut self, ui: &mut egui::Ui) {
        // Set again by the grid while the pointer is over a clip.
        self.hovered_clip = None;
        self.hover_target = None;
        self.hover_source = None;
        // Selecting something brings its Properties panel forward.
        if self.props_tab != self.last_props_tab {
            self.last_props_tab = self.props_tab;
            self.workspace.focus(PropKind::of_tab(self.props_tab));
        }
        let titles: BTreeMap<PanelId, String> = self
            .workspace
            .panels
            .iter()
            .map(|(id, kind)| (*id, self.panel_title(kind)))
            .collect();
        let kinds = self.workspace.panels.clone();
        let mut dock = std::mem::replace(&mut self.workspace.dock, Dock::new(Node::tabs(&[])));
        let rect = ui.max_rect();
        let events = dock.show(
            ui,
            rect,
            &|id| titles.get(&id).cloned().unwrap_or_default(),
            &|id| kinds.get(&id).is_some_and(|k| *k != PanelKind::Grid),
            &mut |ui, id| self.panel(ui, id),
        );
        self.workspace.dock = dock;
        self.workspace.closed(&events.closed);
        self.workspace.floating.extend(events.undocked);
        self.floating_panels(ui.ctx(), &titles);
    }

    /// Each undocked panel in its own OS window, which can go to another
    /// screen. Closing the window docks the panel back.
    fn floating_panels(&mut self, ctx: &egui::Context, titles: &BTreeMap<PanelId, String>) {
        for id in self.workspace.floating.clone() {
            let title = titles.get(&id).cloned().unwrap_or_default();
            let mut redock = false;
            ctx.show_viewport_immediate(
                egui::ViewportId::from_hash_of(("verso-panel", id.0)),
                egui::ViewportBuilder::default()
                    .with_title(format!("Verso – {title}"))
                    .with_inner_size([560.0, 380.0]),
                |ui, _class| {
                    egui::Panel::top(("floating bar", id.0))
                        .frame(egui::Frame::NONE.fill(theme::bg_header()).inner_margin(4))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(&title).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        redock |= ui
                                            .button("Dock back")
                                            .on_hover_text("Return this panel to the main window")
                                            .clicked();
                                    },
                                );
                            });
                        });
                    egui::CentralPanel::default_margins()
                        .frame(egui::Frame::NONE.fill(theme::bg_panel()).inner_margin(4))
                        .show(ui, |ui| self.panel(ui, id));
                    redock |= ui.input(|i| i.viewport().close_requested());
                },
            );
            if redock {
                self.workspace.redock(id);
            }
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, id: PanelId) {
        let Some(kind) = self.workspace.panels.get(&id).cloned() else {
            return;
        };
        match kind {
            PanelKind::Grid => {
                egui::Panel::bottom(("crossfader", id.0))
                    .frame(egui::Frame::NONE.fill(theme::bg_header()).inner_margin(4))
                    .show(ui, |ui| self.crossfader(ui));
                egui::CentralPanel::default_margins()
                    .frame(egui::Frame::NONE.fill(theme::bg_deep()).inner_margin(4))
                    .show(ui, |ui| self.grid(ui));
            }
            PanelKind::Preview => self.preview_panel(ui),
            PanelKind::Output => self.output_panel(ui),
            PanelKind::Browser => self.browser(ui),
            PanelKind::Properties { kind, lock } => {
                let mut lock = lock;
                self.properties_header(ui, kind, &mut lock);
                if let Some(PanelKind::Properties { lock: stored, .. }) =
                    self.workspace.panels.get_mut(&id)
                {
                    *stored = lock;
                }
                ui.separator();
                let target = self.resolve(kind, lock);
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let effects = match (kind, target) {
                        (PropKind::Composition, _) => {
                            self.composition_properties(ui);
                            Some(EffectTarget::Composition)
                        }
                        (PropKind::Group, Some(Lock::Group(g))) => {
                            self.group_properties(ui, g);
                            (g < self.comp.groups.len()).then_some(EffectTarget::Group(g))
                        }
                        (PropKind::Layer, Some(Lock::Layer(l))) if l < self.comp.layers.len() => {
                            self.layer_properties(ui, l);
                            Some(EffectTarget::Layer(l))
                        }
                        (PropKind::Clip, Some(Lock::Clip(l, c))) => {
                            let exists = self
                                .comp
                                .layers
                                .get(l)
                                .and_then(|layer| layer.clips.get(c))
                                .is_some();
                            self.clip_properties(ui, exists.then_some((l, c)));
                            self.comp
                                .layers
                                .get(l)
                                .and_then(|layer| layer.clips.get(c))
                                .and_then(|clip| clip.as_ref())
                                .map(|_| EffectTarget::Clip {
                                    layer: l,
                                    column: c,
                                })
                        }
                        (PropKind::Group, _) => {
                            ui.label(
                                egui::RichText::new("The selected layer is not in a group.")
                                    .color(theme::text_dim()),
                            );
                            None
                        }
                        _ => {
                            ui.label(
                                egui::RichText::new("Nothing selected.").color(theme::text_dim()),
                            );
                            None
                        }
                    };
                    if let Some(target) = effects {
                        self.effect_chain(ui, target);
                    }
                });
            }
        }
    }

    /// Target picker and follow/lock toggle of a Properties panel.
    fn properties_header(&mut self, ui: &mut egui::Ui, kind: PropKind, lock: &mut Option<Lock>) {
        if kind == PropKind::Composition {
            widgets::header(ui, &self.comp.name.clone());
            return;
        }
        let current = self.resolve(kind, *lock);
        let options: Vec<Lock> = match kind {
            PropKind::Group => (0..self.comp.groups.len()).map(Lock::Group).collect(),
            PropKind::Layer => (0..self.comp.layers.len()).rev().map(Lock::Layer).collect(),
            PropKind::Clip => (0..self.comp.layers.len())
                .rev()
                .flat_map(|l| {
                    let layer = &self.comp.layers[l];
                    (0..layer.clips.len())
                        .filter(move |&c| layer.clips[c].is_some())
                        .map(move |c| Lock::Clip(l, c))
                })
                .collect(),
            PropKind::Composition => Vec::new(),
        };
        let labels: Vec<(Lock, String)> = options
            .iter()
            .map(|&o| (o, self.target_label(Some(o))))
            .collect();
        ui.horizontal(|ui| {
            let mut chosen = None;
            egui::ComboBox::from_id_salt("target")
                .selected_text(self.target_label(current))
                .width(ui.available_width() - 90.0)
                .show_ui(ui, |ui| {
                    for (option, label) in &labels {
                        if ui
                            .selectable_label(current == Some(*option), label)
                            .clicked()
                        {
                            chosen = Some(*option);
                        }
                    }
                });
            if let Some(option) = chosen {
                if lock.is_some() {
                    *lock = Some(option);
                } else {
                    // Following panels change the selection itself.
                    match option {
                        Lock::Group(g) => self.selected_group = Some(g),
                        Lock::Layer(l) => self.selected_layer = l,
                        Lock::Clip(l, c) => {
                            self.selected_layer = l;
                            self.selected_clip = Some((l, c));
                        }
                    }
                }
            }
            let locked = lock.is_some();
            let label = if locked { "🔒 Locked" } else { "🔗 Follow" };
            let hint = if locked {
                "Shows a fixed target. Click to follow the selection again."
            } else {
                "Follows the selection. Click to keep showing the current target."
            };
            let button = egui::Button::new(egui::RichText::new(label).color(if locked {
                theme::bg_deep()
            } else {
                theme::text()
            }))
            .fill(if locked {
                theme::solo()
            } else {
                theme::bg_raised()
            });
            if ui.add(button).on_hover_text(hint).clicked() {
                *lock = if locked { None } else { current };
            }
        });
    }

    pub(crate) fn window_menu(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("New Properties panel")
                .small()
                .color(theme::text_dim()),
        );
        for kind in [
            PropKind::Composition,
            PropKind::Group,
            PropKind::Layer,
            PropKind::Clip,
        ] {
            if ui.button(kind.name()).clicked() {
                // A new panel starts locked on the current target, so it
                // keeps showing it while the selection moves on.
                let lock = self.resolve(kind, None);
                let id = self.workspace.open_properties(kind, lock);
                self.workspace.dock.activate(id);
                ui.close();
            }
        }
        ui.separator();
        for (kind, name) in [
            (PanelKind::Preview, "Preview"),
            (PanelKind::Output, "Output"),
            (PanelKind::Browser, "Browser"),
        ] {
            if ui.button(name).clicked() {
                self.workspace.show_panel(kind);
                ui.close();
            }
        }
        ui.separator();
        if ui.button("Messages…").clicked() {
            self.log_open = true;
            ui.close();
        }
        if ui.button("Interface Colors…").clicked() {
            self.colors_open = true;
            ui.close();
        }
        let mut custom = self.window.custom_title_bar;
        if ui
            .checkbox(&mut custom, "Verso Title Bar")
            .on_hover_text("Hides the system title bar; minimize, maximize and close move to the menu bar, which also drags the window")
            .changed()
        {
            self.window.set_custom(ui.ctx(), custom);
        }
        if ui.button("Save Layout as Default").clicked() {
            self.status = match self.workspace.save_as_default() {
                Ok(()) => "Layout saved as default".into(),
                Err(e) => format!("Could not save the default layout: {e}"),
            };
            ui.close();
        }
        if ui
            .button("Reset Layout")
            .on_hover_text("Back to the default layout")
            .clicked()
        {
            self.workspace = Workspace::user_default();
            ui.close();
        }
        if ui
            .button("Built-in Layout")
            .on_hover_text("Back to Verso's original layout")
            .clicked()
        {
            self.workspace = Workspace::default();
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_consistent_and_round_trips() {
        let workspace = Workspace::default();
        assert!(workspace.is_consistent());
        let text = ron::to_string(&workspace).unwrap();
        assert_eq!(ron::from_str::<Workspace>(&text).unwrap(), workspace);
    }

    #[test]
    fn layout_travels_in_the_composition_file() {
        let mut workspace = Workspace::default();
        let locked = workspace.open_properties(PropKind::Clip, Some(Lock::Clip(1, 2)));
        workspace.floating.push(locked);
        let mut comp = verso_core::Composition::new("t", 2, 3);
        comp.layout = workspace.to_text();
        let opened = verso_core::Composition::from_ron(&comp.to_ron().unwrap()).unwrap();
        let restored = opened.layout.as_deref().and_then(Workspace::from_text);
        assert_eq!(restored, Some(workspace));

        // Files saved before layouts were stored still open, without one.
        let old = verso_core::Composition::new("old", 1, 1).to_ron().unwrap();
        assert!(!old.contains("layout"));
        assert_eq!(
            verso_core::Composition::from_ron(&old).unwrap().layout,
            None
        );
        assert_eq!(Workspace::from_text("not a layout"), None);
    }

    #[test]
    fn properties_panels_open_lock_and_focus() {
        let mut workspace = Workspace::default();
        let locked = workspace.open_properties(PropKind::Layer, Some(Lock::Layer(2)));
        assert!(workspace.dock.contains(locked));
        // Focus picks the following Layer panel, not the locked one.
        workspace.focus(PropKind::Layer);
        let following = workspace
            .panels
            .iter()
            .find(|(_, k)| {
                **k == PanelKind::Properties {
                    kind: PropKind::Layer,
                    lock: None,
                }
            })
            .map(|(id, _)| *id)
            .unwrap();
        assert!(workspace.dock.is_visible(following));
        assert!(!workspace.dock.is_visible(locked));
    }

    #[test]
    fn closed_panels_can_be_reopened() {
        let mut workspace = Workspace::default();
        let browser = workspace.find(|p| *p == PanelKind::Browser).unwrap();
        workspace.dock.remove(browser);
        workspace.closed(&[browser]);
        assert!(workspace.find(|p| *p == PanelKind::Browser).is_none());
        workspace.show_panel(PanelKind::Browser);
        assert!(workspace.find(|p| *p == PanelKind::Browser).is_some());
        assert!(workspace.is_consistent());
    }

    #[test]
    fn undocked_panels_stay_consistent_and_redock() {
        let mut workspace = Workspace::default();
        let grid = workspace.find(|p| *p == PanelKind::Grid).unwrap();
        workspace.dock.remove(grid);
        workspace.floating.push(grid);
        assert!(workspace.is_consistent(), "a floating grid still counts");
        let text = ron::to_string(&workspace).unwrap();
        assert_eq!(ron::from_str::<Workspace>(&text).unwrap(), workspace);
        workspace.redock(grid);
        assert!(workspace.floating.is_empty());
        assert!(workspace.dock.contains(grid));
    }

    #[test]
    fn layout_without_grid_is_rejected() {
        let mut workspace = Workspace::default();
        let grid = workspace.find(|p| *p == PanelKind::Grid).unwrap();
        workspace.dock.remove(grid);
        assert!(!workspace.is_consistent());
    }
}
