//! Dark, dense look for a live performance tool. Verso's own palette.

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Vec2};

pub const BG_DEEP: Color32 = Color32::from_rgb(14, 15, 17);
pub const BG_PANEL: Color32 = Color32::from_rgb(28, 30, 33);
pub const BG_RAISED: Color32 = Color32::from_rgb(40, 43, 47);
pub const BG_HEADER: Color32 = Color32::from_rgb(34, 36, 40);
pub const LINE: Color32 = Color32::from_rgb(54, 57, 62);
pub const TEXT: Color32 = Color32::from_rgb(208, 210, 214);
pub const TEXT_DIM: Color32 = Color32::from_rgb(151, 155, 161);
/// Playing clips, active toggles, fader fills.
pub const ACCENT: Color32 = Color32::from_rgb(46, 158, 190);
/// Bypass.
pub const WARN: Color32 = Color32::from_rgb(226, 86, 86);
/// Solo.
pub const SOLO: Color32 = Color32::from_rgb(232, 190, 64);
/// Selection outline.
pub const SELECT: Color32 = Color32::from_rgb(196, 202, 212);

pub fn apply(ctx: &egui::Context) {
    let mut style = (*ctx.global_style()).clone();
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(10.0)),
        (TextStyle::Body, FontId::proportional(12.0)),
        (TextStyle::Button, FontId::proportional(12.0)),
        (TextStyle::Heading, FontId::proportional(13.0)),
        (TextStyle::Monospace, FontId::monospace(11.5)),
    ]
    .into();
    style.spacing.item_spacing = Vec2::new(4.0, 3.0);
    style.spacing.button_padding = Vec2::new(5.0, 2.0);
    style.spacing.interact_size.y = 18.0;

    let v = &mut style.visuals;
    *v = egui::Visuals::dark();
    v.override_text_color = Some(TEXT);
    v.panel_fill = BG_PANEL;
    v.window_fill = BG_PANEL;
    v.extreme_bg_color = BG_DEEP;
    v.faint_bg_color = BG_HEADER;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.window_corner_radius = CornerRadius::same(3);
    v.menu_corner_radius = CornerRadius::same(3);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(2);
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.bg_fill = BG_RAISED;
    v.widgets.inactive.weak_bg_fill = BG_RAISED;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(52, 56, 61);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(60, 64, 70);
    ctx.set_global_style(style);
}
