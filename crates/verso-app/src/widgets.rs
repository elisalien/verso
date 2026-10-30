use eframe::egui::{
    self, Align2, Color32, FontId, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::theme;

/// Horizontal fader: filled bar, drag or click to set, double-click resets to `reset`.
pub fn fader(ui: &mut Ui, value: &mut f32, size: Vec2, color: Color32, reset: f32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if let Some(pos) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked())
    {
        *value = ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        response.mark_changed();
    }
    if response.double_clicked() {
        *value = reset;
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::BG_DEEP);
    let mut fill = rect;
    fill.set_width(rect.width() * value.clamp(0.0, 1.0));
    painter.rect_filled(
        fill,
        2.0,
        color.gamma_multiply(if response.hovered() { 0.85 } else { 0.65 }),
    );
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{:.0}%", *value * 100.0),
        FontId::proportional(10.0),
        theme::TEXT,
    );
    response
}

/// Vertical fader, filled from the bottom. Drag or click to set, double-click resets.
pub fn vfader(ui: &mut Ui, value: &mut f32, size: Vec2, color: Color32, reset: f32) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    if let Some(pos) = response.interact_pointer_pos()
        && (response.dragged() || response.clicked())
    {
        *value = ((rect.bottom() - pos.y) / rect.height()).clamp(0.0, 1.0);
        response.mark_changed();
    }
    if response.double_clicked() {
        *value = reset;
        response.mark_changed();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, theme::BG_DEEP);
    let level = value.clamp(0.0, 1.0);
    let fill = Rect::from_min_max(
        egui::pos2(rect.left(), rect.bottom() - rect.height() * level),
        rect.right_bottom(),
    );
    painter.rect_filled(
        fill,
        2.0,
        color.gamma_multiply(if response.hovered() { 0.9 } else { 0.7 }),
    );
    // Cap line at the current level.
    painter.line_segment(
        [fill.left_top(), fill.right_top()],
        Stroke::new(2.0, theme::TEXT),
    );
    painter.rect_stroke(rect, 2.0, Stroke::new(1.0, theme::LINE), StrokeKind::Inside);
    let percent = format!("{:.0}%", level * 100.0);
    response.on_hover_text(percent)
}

/// Small square toggle with a one-letter label, lit with `on_color` when active.
pub fn toggle(ui: &mut Ui, on: &mut bool, label: &str, on_color: Color32, hint: &str) -> Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    paint_square(ui, rect, label, *on, on_color, response.hovered());
    response.on_hover_text(hint)
}

/// Small square push button.
pub fn square_button(ui: &mut Ui, label: &str, hint: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::click());
    paint_square(ui, rect, label, false, theme::ACCENT, response.hovered());
    response.on_hover_text(hint)
}

fn paint_square(ui: &Ui, rect: Rect, label: &str, on: bool, on_color: Color32, hovered: bool) {
    let painter = ui.painter_at(rect);
    let fill = if on {
        on_color
    } else if hovered {
        theme::BG_RAISED.gamma_multiply(1.4)
    } else {
        theme::BG_RAISED
    };
    painter.rect_filled(rect, 2.0, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if on { theme::BG_DEEP } else { theme::TEXT },
    );
}

/// Panel title bar.
pub fn header(ui: &mut Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::BG_HEADER);
    ui.painter().text(
        rect.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
}

/// Row of tabs; returns nothing, updates `current`.
pub fn tabs<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    ui.horizontal(|ui| {
        for (value, label) in items {
            let selected = *current == *value;
            let text = egui::RichText::new(*label).color(if selected {
                theme::ACCENT
            } else {
                theme::TEXT_DIM
            });
            if ui.selectable_label(selected, text).clicked() {
                *current = *value;
            }
        }
    });
}
