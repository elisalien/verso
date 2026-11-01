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
    painter.rect_filled(rect, 2.0, theme::bg_deep());
    let mut fill = rect;
    fill.set_width(rect.width() * value.clamp(0.0, 1.0));
    painter.rect_filled(
        fill,
        2.0,
        color.gamma_multiply(if response.hovered() { 0.85 } else { 0.65 }),
    );
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme::line()),
        StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        format!("{:.0}%", *value * 100.0),
        FontId::proportional(10.0),
        theme::text(),
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
    painter.rect_filled(rect, 2.0, theme::bg_deep());
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
        Stroke::new(2.0, theme::text()),
    );
    painter.rect_stroke(
        rect,
        2.0,
        Stroke::new(1.0, theme::line()),
        StrokeKind::Inside,
    );
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
    paint_square(ui, rect, label, false, theme::accent(), response.hovered());
    response.on_hover_text(hint)
}

fn paint_square(ui: &Ui, rect: Rect, label: &str, on: bool, on_color: Color32, hovered: bool) {
    let painter = ui.painter_at(rect);
    let fill = if on {
        on_color
    } else if hovered {
        theme::bg_raised().gamma_multiply(1.4)
    } else {
        theme::bg_raised()
    };
    painter.rect_filled(rect, 2.0, fill);
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.0),
        if on { theme::bg_deep() } else { theme::text() },
    );
}

/// Panel title bar.
pub fn header(ui: &mut Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, theme::bg_header());
    ui.painter().text(
        rect.left_center() + Vec2::new(6.0, 0.0),
        Align2::LEFT_CENTER,
        title,
        FontId::proportional(11.0),
        theme::text_dim(),
    );
}

/// Row of tabs; returns nothing, updates `current`.
pub fn tabs<T: PartialEq + Copy>(ui: &mut Ui, current: &mut T, items: &[(T, &str)]) {
    ui.horizontal(|ui| {
        for (value, label) in items {
            let selected = *current == *value;
            let text = egui::RichText::new(*label).color(if selected {
                theme::accent()
            } else {
                theme::text_dim()
            });
            if ui.selectable_label(selected, text).clicked() {
                *current = *value;
            }
        }
    });
}

/// The payload dropped on `response`, if it is a `T`.
///
/// `Response::dnd_release_payload` takes the payload before checking its
/// type, so asking for the wrong type first would lose the drop.
pub fn dropped<T: std::any::Any + Send + Sync>(response: &Response) -> Option<std::sync::Arc<T>> {
    response.dnd_hover_payload::<T>()?;
    response.dnd_release_payload::<T>()
}

/// Color button that only writes `color` back when the user changed it:
/// egui's unmultiplied editor round-trips through premultiplied alpha and
/// would otherwise alter the value by float error on every frame.
pub fn color_edit(ui: &mut Ui, color: &mut [f32; 4]) -> Response {
    let mut edited = *color;
    let response = ui.color_edit_button_rgba_unmultiplied(&mut edited);
    if response.changed() {
        *color = edited;
    }
    response
}

/// Room a slider keeps for its value field and suffix.
const SLIDER_VALUE_WIDTH: f32 = 64.0;

/// Sizes the sliders of a two-column grid (labels `label_width` wide, then
/// the widget) to fill the panel, so the whole range stays visible when the
/// panel is resized.
pub fn fit_sliders(ui: &mut Ui, label_width: f32, column_spacing: f32) {
    let spacing = ui.spacing().item_spacing.x;
    let width = ui.available_width() - label_width - column_spacing - SLIDER_VALUE_WIDTH - spacing;
    ui.spacing_mut().slider_width = width.max(24.0);
}

/// Width of the widest of `labels` in the body font.
pub fn label_width<'a>(ui: &Ui, labels: impl IntoIterator<Item = &'a str>) -> f32 {
    labels
        .into_iter()
        .map(|text| {
            eframe::egui::WidgetText::from(text)
                .into_galley(ui, None, f32::INFINITY, eframe::egui::TextStyle::Body)
                .size()
                .x
        })
        .fold(0.0, f32::max)
}
