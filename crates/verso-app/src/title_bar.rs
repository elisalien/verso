//! Optional title bar drawn by Verso instead of the system one.
//!
//! When it is on, the main window has no system decorations: the menu bar
//! moves the window (drag), maximizes it (double-click) and carries the
//! minimize, maximize and close buttons. The window edges resize it. The
//! choice is kept in `window.ron` in the user folder and applies at once
//! (Window › Verso Title Bar).

use eframe::egui::{self, Color32, CursorIcon, ResizeDirection, Sense, Stroke, ViewportCommand};
use serde::{Deserialize, Serialize};

use crate::theme;

/// Width of the band along the window edges that resizes the window.
const EDGE: f32 = 5.0;
/// Size of one caption button.
const BUTTON: egui::Vec2 = egui::vec2(40.0, 22.0);

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowSettings {
    /// Verso draws the title bar; the system one is hidden.
    pub custom_title_bar: bool,
}

impl Default for WindowSettings {
    /// On by default on Windows. Elsewhere the desktop decides (GNOME on
    /// Wayland has no server-side title bar to remove).
    fn default() -> Self {
        Self {
            custom_title_bar: cfg!(windows),
        }
    }
}

fn settings_file() -> Option<std::path::PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("window.ron"))
}

impl WindowSettings {
    pub fn load() -> Self {
        settings_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(file) = settings_file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = ron::to_string(self) {
            let _ = std::fs::write(file, text);
        }
    }

    /// Turns the custom title bar on or off now, and remembers it.
    pub fn set_custom(&mut self, ctx: &egui::Context, custom: bool) {
        self.custom_title_bar = custom;
        ctx.send_viewport_cmd(ViewportCommand::Decorations(!custom));
        self.save();
    }
}

/// Moves the window when the empty part of the bar is dragged, and toggles
/// maximized on double-click. Call before drawing the bar's widgets, so
/// they keep their own clicks.
pub fn drag_area(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let response = ui.interact(
        rect,
        ui.scope_id().with("title bar drag"),
        Sense::click_and_drag(),
    );
    if response.double_clicked() {
        toggle_maximized(ui.ctx());
    } else if response.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

fn toggle_maximized(ctx: &egui::Context) {
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
}

#[derive(Clone, Copy)]
enum Caption {
    Minimize,
    Maximize,
    Close,
}

/// Minimize, maximize and close, right to left (call in a right-to-left
/// layout). Close goes through the usual unsaved-changes question.
pub fn buttons(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    if caption_button(ui, Caption::Close, "Close").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let tip = if maximized { "Restore" } else { "Maximize" };
    if caption_button(ui, Caption::Maximize, tip).clicked() {
        toggle_maximized(&ctx);
    }
    if caption_button(ui, Caption::Minimize, "Minimize").clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

fn caption_button(ui: &mut egui::Ui, caption: Caption, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(BUTTON, Sense::click());
    let painter = ui.painter();
    if response.hovered() {
        let fill = match caption {
            Caption::Close => Color32::from_rgb(196, 43, 28),
            _ => theme::bg_raised(),
        };
        painter.rect_filled(rect, 0.0, fill);
    }
    let color = if response.hovered() && matches!(caption, Caption::Close) {
        Color32::WHITE
    } else {
        theme::text()
    };
    let stroke = Stroke::new(1.0, color);
    let c = rect.center();
    let r = 5.0;
    match caption {
        Caption::Minimize => {
            painter.line_segment([c + egui::vec2(-r, 0.0), c + egui::vec2(r, 0.0)], stroke);
        }
        Caption::Maximize => {
            let square = egui::Rect::from_center_size(c, egui::vec2(2.0 * r, 2.0 * r));
            painter.rect_stroke(square, 0.0, stroke, egui::StrokeKind::Inside);
        }
        Caption::Close => {
            painter.line_segment([c + egui::vec2(-r, -r), c + egui::vec2(r, r)], stroke);
            painter.line_segment([c + egui::vec2(-r, r), c + egui::vec2(r, -r)], stroke);
        }
    }
    response.on_hover_text(tip)
}

/// Resizes the window from its edges and corners, which a window without
/// system decorations does not do by itself. Call once per frame.
pub fn resize_edges(ctx: &egui::Context) {
    let maximized = ctx.input(|i| {
        i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false)
    });
    if maximized {
        return;
    }
    let screen = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let west = pos.x <= screen.left() + EDGE;
    let east = pos.x >= screen.right() - EDGE;
    let north = pos.y <= screen.top() + EDGE;
    let south = pos.y >= screen.bottom() - EDGE;
    let (direction, cursor) = match (north, south, west, east) {
        (true, _, true, _) => (ResizeDirection::NorthWest, CursorIcon::ResizeNwSe),
        (true, _, _, true) => (ResizeDirection::NorthEast, CursorIcon::ResizeNeSw),
        (_, true, true, _) => (ResizeDirection::SouthWest, CursorIcon::ResizeNeSw),
        (_, true, _, true) => (ResizeDirection::SouthEast, CursorIcon::ResizeNwSe),
        (true, ..) => (ResizeDirection::North, CursorIcon::ResizeVertical),
        (_, true, ..) => (ResizeDirection::South, CursorIcon::ResizeVertical),
        (_, _, true, _) => (ResizeDirection::West, CursorIcon::ResizeHorizontal),
        (_, _, _, true) => (ResizeDirection::East, CursorIcon::ResizeHorizontal),
        _ => return,
    };
    ctx.set_cursor_icon(cursor);
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}
