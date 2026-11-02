//! A small docking layout: a tree of resizable splits whose leaves are tab
//! groups. Drag a tab onto another group's tab bar or centre to move it there,
//! or onto an edge to split that group.

use eframe::egui::{self, Align2, CursorIcon, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use serde::{Deserialize, Serialize};

use crate::theme;

const TAB_HEIGHT: f32 = 22.0;
const SEPARATOR: f32 = 4.0;
/// Placeholder used while moving a tab.
const MOVING: PanelId = PanelId(u32::MAX);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PanelId(pub u32);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Node {
    Tabs {
        tabs: Vec<PanelId>,
        active: usize,
    },
    /// `vertical`: children stacked top and bottom, else left and right.
    /// `ratio` is the share of the first child.
    Split {
        vertical: bool,
        ratio: f32,
        children: Box<[Node; 2]>,
    },
}

impl Node {
    pub fn tabs(tabs: &[PanelId]) -> Node {
        Node::Tabs {
            tabs: tabs.to_vec(),
            active: 0,
        }
    }

    pub fn split(vertical: bool, ratio: f32, first: Node, second: Node) -> Node {
        Node::Split {
            vertical,
            ratio,
            children: Box::new([first, second]),
        }
    }

    fn at_mut(&mut self, path: &[usize]) -> Option<&mut Node> {
        match (path.split_first(), self) {
            (None, node) => Some(node),
            (Some((&i, rest)), Node::Split { children, .. }) => children.get_mut(i)?.at_mut(rest),
            _ => None,
        }
    }

    fn for_each_tab(&self, f: &mut impl FnMut(PanelId)) {
        match self {
            Node::Tabs { tabs, .. } => tabs.iter().copied().for_each(f),
            Node::Split { children, .. } => children.iter().for_each(|c| c.for_each_tab(f)),
        }
    }

    /// Removes `id` wherever it is. Returns whether it was found.
    fn remove(&mut self, id: PanelId) -> bool {
        match self {
            Node::Tabs { tabs, active } => {
                let Some(i) = tabs.iter().position(|&t| t == id) else {
                    return false;
                };
                tabs.remove(i);
                if *active >= i && *active > 0 {
                    *active -= 1;
                }
                true
            }
            Node::Split { children, .. } => children.iter_mut().any(|c| c.remove(id)),
        }
    }

    fn replace(&mut self, from: PanelId, to: PanelId) {
        match self {
            Node::Tabs { tabs, .. } => tabs
                .iter_mut()
                .filter(|t| **t == from)
                .for_each(|t| *t = to),
            Node::Split { children, .. } => children.iter_mut().for_each(|c| c.replace(from, to)),
        }
    }

    fn is_empty(&self) -> bool {
        matches!(self, Node::Tabs { tabs, .. } if tabs.is_empty())
    }

    /// Replaces splits that have an empty side by their other side.
    fn collapse(&mut self) {
        if let Node::Split { children, .. } = self {
            children.iter_mut().for_each(Node::collapse);
            let keep = match (children[0].is_empty(), children[1].is_empty()) {
                (true, _) => Some(1),
                (_, true) => Some(0),
                _ => None,
            };
            if let Some(keep) = keep {
                let child = std::mem::replace(&mut children[keep], Node::tabs(&[]));
                *self = child;
            }
        }
    }

    /// Makes `id` the active tab of its group. Returns whether it was found.
    fn activate(&mut self, id: PanelId) -> bool {
        match self {
            Node::Tabs { tabs, active } => match tabs.iter().position(|&t| t == id) {
                Some(i) => {
                    *active = i;
                    true
                }
                None => false,
            },
            Node::Split { children, .. } => children.iter_mut().any(|c| c.activate(id)),
        }
    }

    /// Whether `id` is the visible tab of its group.
    #[cfg(test)]
    fn is_visible(&self, id: PanelId) -> bool {
        match self {
            Node::Tabs { tabs, active } => tabs.get(*active) == Some(&id),
            Node::Split { children, .. } => children.iter().any(|c| c.is_visible(id)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// Where a dragged tab lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drop {
    /// Into the tab group at `path`.
    Into(Vec<usize>),
    /// Splits the group at `path`, the tab going to `side`.
    Split(Vec<usize>, Side),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Dock {
    pub root: Node,
    /// Panel last clicked, outlined. Not saved.
    #[serde(skip)]
    pub focused: Option<PanelId>,
}

impl Dock {
    pub fn new(root: Node) -> Self {
        Self {
            root,
            focused: None,
        }
    }

    pub fn panels(&self) -> Vec<PanelId> {
        let mut all = Vec::new();
        self.root.for_each_tab(&mut |id| all.push(id));
        all
    }

    #[cfg(test)]
    pub fn contains(&self, id: PanelId) -> bool {
        self.panels().contains(&id)
    }

    pub fn activate(&mut self, id: PanelId) -> bool {
        self.root.activate(id)
    }

    #[cfg(test)]
    pub fn is_visible(&self, id: PanelId) -> bool {
        self.root.is_visible(id)
    }

    pub fn remove(&mut self, id: PanelId) {
        self.root.remove(id);
        self.root.collapse();
    }

    /// Adds `id` as a tab next to `beside` (or to the first group).
    pub fn add_beside(&mut self, id: PanelId, beside: Option<PanelId>) {
        fn add(node: &mut Node, id: PanelId, beside: Option<PanelId>) -> bool {
            match node {
                Node::Tabs { tabs, active } => {
                    if beside.is_none_or(|b| tabs.contains(&b)) {
                        tabs.push(id);
                        *active = tabs.len() - 1;
                        true
                    } else {
                        false
                    }
                }
                Node::Split { children, .. } => children.iter_mut().any(|c| add(c, id, beside)),
            }
        }
        if !add(&mut self.root, id, beside) {
            add(&mut self.root, id, None);
        }
    }

    /// Moves `id` to `drop`.
    pub fn move_tab(&mut self, id: PanelId, drop: &Drop) {
        let path = match drop {
            Drop::Into(path) | Drop::Split(path, _) => path,
        };
        let Some(target) = self.root.at_mut(path) else {
            return;
        };
        // Moving a group's only tab onto itself changes nothing.
        if matches!(target, Node::Tabs { tabs, .. } if tabs == &[id]) {
            return;
        }
        match drop {
            Drop::Into(_) => {
                if let Node::Tabs { tabs, active } = target {
                    if tabs.contains(&id) {
                        return;
                    }
                    tabs.push(MOVING);
                    *active = tabs.len() - 1;
                }
            }
            Drop::Split(_, side) => {
                let old = std::mem::replace(target, Node::tabs(&[]));
                let new = Node::tabs(&[MOVING]);
                let (vertical, first, second) = match side {
                    Side::Left => (false, new, old),
                    Side::Right => (false, old, new),
                    Side::Top => (true, new, old),
                    Side::Bottom => (true, old, new),
                };
                *target = Node::split(vertical, 0.5, first, second);
            }
        }
        self.root.remove(id);
        self.root.replace(MOVING, id);
        self.root.collapse();
        self.root.activate(id);
    }

    /// Draws the layout in `rect`. `title` names a tab, `closable` tells
    /// whether it shows a close button, and `content` draws a visible panel.
    /// Returns the panels closed or undocked this frame.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        title: &dyn Fn(PanelId) -> String,
        closable: &dyn Fn(PanelId) -> bool,
        content: &mut dyn FnMut(&mut egui::Ui, PanelId),
    ) -> DockEvents {
        let mut state = ShowState {
            closed: Vec::new(),
            undocked: Vec::new(),
            drop: None,
            dragging: egui::DragAndDrop::payload::<PanelId>(ui.ctx()).map(|p| *p),
            focused: self.focused,
        };
        show_node(
            ui,
            &mut self.root,
            rect,
            &mut Vec::new(),
            title,
            closable,
            content,
            &mut state,
        );

        let released = ui.input(|i| i.pointer.any_released());
        if released && let (Some(id), Some(drop)) = (state.dragging, state.drop) {
            egui::DragAndDrop::clear_payload(ui.ctx());
            self.move_tab(id, &drop);
        }
        for &id in state.closed.iter().chain(&state.undocked) {
            self.remove(id);
        }
        self.focused = state.focused;
        DockEvents {
            closed: state.closed,
            undocked: state.undocked,
        }
    }
}

/// What happened to tabs during [`Dock::show`]. Both lists are already
/// removed from the layout.
#[derive(Debug, Default)]
pub struct DockEvents {
    pub closed: Vec<PanelId>,
    pub undocked: Vec<PanelId>,
}

struct ShowState {
    closed: Vec<PanelId>,
    undocked: Vec<PanelId>,
    drop: Option<Drop>,
    dragging: Option<PanelId>,
    focused: Option<PanelId>,
}

#[allow(clippy::too_many_arguments)]
fn show_node(
    ui: &mut egui::Ui,
    node: &mut Node,
    rect: Rect,
    path: &mut Vec<usize>,
    title: &dyn Fn(PanelId) -> String,
    closable: &dyn Fn(PanelId) -> bool,
    content: &mut dyn FnMut(&mut egui::Ui, PanelId),
    state: &mut ShowState,
) {
    match node {
        Node::Split {
            vertical,
            ratio,
            children,
        } => {
            let (first, separator, second) = split_rects(rect, *vertical, *ratio);
            let id = ui.scope_id().with(("dock split", path.clone()));
            let response = ui.interact(separator, id, Sense::drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(if *vertical {
                    CursorIcon::ResizeVertical
                } else {
                    CursorIcon::ResizeHorizontal
                });
            }
            if let Some(pos) = response.interact_pointer_pos()
                && response.dragged()
            {
                *ratio = if *vertical {
                    (pos.y - rect.top()) / rect.height()
                } else {
                    (pos.x - rect.left()) / rect.width()
                }
                .clamp(0.1, 0.9);
            }
            ui.painter().rect_filled(
                separator,
                0.0,
                if response.hovered() || response.dragged() {
                    theme::accent().gamma_multiply(0.6)
                } else {
                    theme::bg_deep()
                },
            );
            for (i, child_rect) in [first, second].into_iter().enumerate() {
                path.push(i);
                show_node(
                    ui,
                    &mut children[i],
                    child_rect,
                    path,
                    title,
                    closable,
                    content,
                    state,
                );
                path.pop();
            }
        }
        Node::Tabs { tabs, active } => {
            show_tabs(
                ui, tabs, active, rect, path, title, closable, content, state,
            );
        }
    }
}

fn split_rects(rect: Rect, vertical: bool, ratio: f32) -> (Rect, Rect, Rect) {
    if vertical {
        let y = rect.top() + (rect.height() - SEPARATOR) * ratio;
        (
            Rect::from_min_max(rect.min, Pos2::new(rect.right(), y)),
            Rect::from_min_max(
                Pos2::new(rect.left(), y),
                Pos2::new(rect.right(), y + SEPARATOR),
            ),
            Rect::from_min_max(Pos2::new(rect.left(), y + SEPARATOR), rect.max),
        )
    } else {
        let x = rect.left() + (rect.width() - SEPARATOR) * ratio;
        (
            Rect::from_min_max(rect.min, Pos2::new(x, rect.bottom())),
            Rect::from_min_max(
                Pos2::new(x, rect.top()),
                Pos2::new(x + SEPARATOR, rect.bottom()),
            ),
            Rect::from_min_max(Pos2::new(x + SEPARATOR, rect.top()), rect.max),
        )
    }
}

#[allow(clippy::too_many_arguments)]
fn show_tabs(
    ui: &mut egui::Ui,
    tabs: &mut [PanelId],
    active: &mut usize,
    rect: Rect,
    path: &[usize],
    title: &dyn Fn(PanelId) -> String,
    closable: &dyn Fn(PanelId) -> bool,
    content: &mut dyn FnMut(&mut egui::Ui, PanelId),
    state: &mut ShowState,
) {
    *active = (*active).min(tabs.len().saturating_sub(1));
    let bar = Rect::from_min_size(rect.min, Vec2::new(rect.width(), TAB_HEIGHT));
    let body = Rect::from_min_max(Pos2::new(rect.left(), bar.bottom()), rect.max);
    let painter = ui.painter_at(rect);
    painter.rect_filled(bar, 0.0, theme::bg_deep());
    painter.rect_filled(body, 0.0, theme::bg_panel());

    // Tabs.
    let mut x = bar.left();
    for (i, &id) in tabs.iter().enumerate() {
        let label = title(id);
        let galley = painter.layout_no_wrap(label, FontId::proportional(11.5), theme::text());
        let close = closable(id);
        let width = galley.size().x + 18.0 + if close { 14.0 } else { 0.0 };
        let tab = Rect::from_min_size(Pos2::new(x, bar.top()), Vec2::new(width, TAB_HEIGHT));
        x += width + 1.0;
        let response = ui.interact(
            tab,
            ui.scope_id().with(("dock tab", id)),
            Sense::click_and_drag(),
        );
        response.dnd_set_drag_payload(id);
        let selected = i == *active;
        let fill = if selected {
            theme::bg_panel()
        } else if response.hovered() {
            theme::bg_raised()
        } else {
            theme::bg_header()
        };
        painter.rect_filled(tab, 2.0, fill);
        if selected {
            let line = Rect::from_min_size(tab.left_top(), Vec2::new(tab.width(), 2.0));
            painter.rect_filled(line, 0.0, theme::accent());
        }
        painter.galley(
            tab.left_center() + Vec2::new(9.0, -galley.size().y / 2.0),
            galley,
            if selected {
                theme::text()
            } else {
                theme::text_dim()
            },
        );
        if response.clicked() {
            *active = i;
        }
        response.context_menu(|ui| {
            if ui
                .button("Undock")
                .on_hover_text("Open in its own window")
                .clicked()
            {
                state.undocked.push(id);
                ui.close();
            }
            if close && ui.button("Close").clicked() {
                state.closed.push(id);
                ui.close();
            }
        });
        if close {
            let close_rect = Rect::from_center_size(
                tab.right_center() - Vec2::new(10.0, 0.0),
                Vec2::splat(12.0),
            );
            let close_response = ui.interact(
                close_rect,
                ui.scope_id().with(("dock close", id)),
                Sense::click(),
            );
            painter.text(
                close_rect.center(),
                Align2::CENTER_CENTER,
                "×",
                FontId::proportional(12.0),
                if close_response.hovered() {
                    theme::warn()
                } else {
                    theme::text_dim()
                },
            );
            if close_response.clicked() {
                state.closed.push(id);
            }
        }
    }

    // Visible panel.
    if let Some(&id) = tabs.get(*active) {
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(body.shrink(4.0))
                .id_salt(("dock panel", id)),
        );
        child.set_clip_rect(body);
        content(&mut child, id);

        // A click anywhere in the group focuses its visible panel, unless a
        // window over the group took it.
        let pressed = ui.input(|i| {
            i.pointer
                .interact_pos()
                .filter(|p| i.pointer.any_pressed() && rect.contains(*p))
        });
        let pressed = pressed.is_some_and(|p| {
            ui.ctx()
                .layer_id_at(p)
                .is_none_or(|layer| layer.order == ui.layer_id().order)
        });
        if pressed {
            state.focused = Some(id);
        }
        if state.focused == Some(id) {
            // Painted in the panel's own layer, so windows stay on top of it.
            let outline = ui.painter().with_clip_rect(rect);
            outline.rect_stroke(
                rect.shrink(1.0),
                3.0,
                Stroke::new(1.5, theme::accent()),
                StrokeKind::Inside,
            );
        }
    }

    // Drop zones while a tab is dragged over this group.
    if state.dragging.is_some()
        && let Some(pointer) = ui.input(|i| i.pointer.hover_pos())
        && rect.contains(pointer)
    {
        let (drop, zone) = if bar.contains(pointer) {
            (Drop::Into(path.to_vec()), body)
        } else {
            let rel = (pointer - body.min) / body.size();
            let edge = 0.25;
            let side = if rel.x < edge {
                Some(Side::Left)
            } else if rel.x > 1.0 - edge {
                Some(Side::Right)
            } else if rel.y < edge {
                Some(Side::Top)
            } else if rel.y > 1.0 - edge {
                Some(Side::Bottom)
            } else {
                None
            };
            match side {
                None => (Drop::Into(path.to_vec()), body),
                Some(side) => {
                    let zone = match side {
                        Side::Left => Rect::from_min_size(
                            body.min,
                            Vec2::new(body.width() / 2.0, body.height()),
                        ),
                        Side::Right => {
                            Rect::from_min_max(Pos2::new(body.center().x, body.top()), body.max)
                        }
                        Side::Top => Rect::from_min_size(
                            body.min,
                            Vec2::new(body.width(), body.height() / 2.0),
                        ),
                        Side::Bottom => {
                            Rect::from_min_max(Pos2::new(body.left(), body.center().y), body.max)
                        }
                    };
                    (Drop::Split(path.to_vec(), side), zone)
                }
            }
        };
        let overlay = ui.ctx().layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            ui.scope_id().with("dock drop"),
        ));
        overlay.rect_filled(zone, 2.0, theme::accent().linear_multiply(0.25));
        overlay.rect_stroke(
            zone,
            2.0,
            Stroke::new(2.0, theme::accent()),
            StrokeKind::Inside,
        );
        state.drop = Some(drop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(dock: &Dock) -> Vec<u32> {
        dock.panels().iter().map(|p| p.0).collect()
    }

    fn sample() -> Dock {
        // [1, 2] | [3]
        Dock::new(Node::split(
            false,
            0.5,
            Node::tabs(&[PanelId(1), PanelId(2)]),
            Node::tabs(&[PanelId(3)]),
        ))
    }

    #[test]
    fn move_into_another_group() {
        let mut dock = sample();
        dock.move_tab(PanelId(1), &Drop::Into(vec![1]));
        assert_eq!(
            dock.root,
            Node::split(
                false,
                0.5,
                Node::Tabs {
                    tabs: vec![PanelId(2)],
                    active: 0
                },
                Node::Tabs {
                    tabs: vec![PanelId(3), PanelId(1)],
                    active: 1
                },
            )
        );
    }

    #[test]
    fn moving_the_last_tab_collapses_its_group() {
        let mut dock = sample();
        dock.move_tab(PanelId(3), &Drop::Into(vec![0]));
        assert_eq!(
            dock.root,
            Node::Tabs {
                tabs: vec![PanelId(1), PanelId(2), PanelId(3)],
                active: 2
            }
        );
    }

    #[test]
    fn split_to_an_edge() {
        let mut dock = sample();
        dock.move_tab(PanelId(2), &Drop::Split(vec![1], Side::Bottom));
        assert_eq!(
            dock.root,
            Node::split(
                false,
                0.5,
                Node::Tabs {
                    tabs: vec![PanelId(1)],
                    active: 0
                },
                Node::split(
                    true,
                    0.5,
                    Node::tabs(&[PanelId(3)]),
                    Node::Tabs {
                        tabs: vec![PanelId(2)],
                        active: 0
                    }
                ),
            )
        );
        assert_eq!(ids(&dock), [1, 3, 2]);
    }

    #[test]
    fn splitting_a_group_with_its_own_tab() {
        let mut dock = sample();
        dock.move_tab(PanelId(1), &Drop::Split(vec![0], Side::Left));
        assert_eq!(ids(&dock), [1, 2, 3]);
        // Its only tab onto itself: no change.
        let before = dock.clone();
        dock.move_tab(PanelId(1), &Drop::Split(vec![0, 0], Side::Right));
        assert_eq!(dock, before);
    }

    #[test]
    fn remove_add_and_activate() {
        let mut dock = sample();
        dock.remove(PanelId(3));
        assert_eq!(
            dock.root,
            Node::Tabs {
                tabs: vec![PanelId(1), PanelId(2)],
                active: 0
            }
        );
        dock.add_beside(PanelId(9), Some(PanelId(2)));
        assert!(dock.is_visible(PanelId(9)));
        assert!(dock.activate(PanelId(1)));
        assert!(dock.is_visible(PanelId(1)));
        assert!(!dock.activate(PanelId(42)));
        // Unknown neighbour: goes to the first group.
        dock.add_beside(PanelId(10), Some(PanelId(77)));
        assert!(dock.contains(PanelId(10)));
    }

    #[test]
    fn layout_round_trips_through_ron() {
        let dock = sample();
        let text = ron::to_string(&dock).unwrap();
        assert_eq!(ron::from_str::<Dock>(&text).unwrap(), dock);
    }
}
