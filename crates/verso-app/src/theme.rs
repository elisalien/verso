//! Interface colors. Dark and dense by default, editable from
//! Window › Interface Colors and saved in the user folder.

use std::sync::RwLock;

use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke, TextStyle, Vec2};
use serde::{Deserialize, Serialize};

/// Every color the interface uses, by role. Stored as sRGB bytes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    pub bg_deep: [u8; 3],
    pub bg_panel: [u8; 3],
    pub bg_raised: [u8; 3],
    pub bg_header: [u8; 3],
    pub line: [u8; 3],
    pub text: [u8; 3],
    pub text_dim: [u8; 3],
    /// Playing clips, active toggles, fader fills, focus.
    pub accent: [u8; 3],
    /// Bypass.
    pub warn: [u8; 3],
    /// Solo.
    pub solo: [u8; 3],
    /// Selection outline.
    pub select: [u8; 3],
    /// MilkDrop clips and browser entries, which blend into each other and
    /// read other layers, unlike the other sources.
    #[serde(default = "default_milkdrop")]
    pub milkdrop: [u8; 3],
}

fn default_milkdrop() -> [u8; 3] {
    Palette::TEAL.milkdrop
}

impl Palette {
    pub const TEAL: Palette = Palette {
        bg_deep: [14, 15, 17],
        bg_panel: [28, 30, 33],
        bg_raised: [40, 43, 47],
        bg_header: [34, 36, 40],
        line: [54, 57, 62],
        text: [208, 210, 214],
        text_dim: [151, 155, 161],
        accent: [46, 158, 190],
        warn: [226, 86, 86],
        solo: [232, 190, 64],
        select: [196, 202, 212],
        milkdrop: [214, 92, 196],
    };
    pub const AMBER: Palette = Palette {
        accent: [232, 150, 48],
        select: [240, 214, 170],
        ..Palette::TEAL
    };
    pub const VIOLET: Palette = Palette {
        bg_deep: [15, 13, 20],
        bg_panel: [29, 26, 36],
        bg_raised: [42, 38, 52],
        bg_header: [35, 31, 44],
        line: [58, 53, 70],
        accent: [158, 110, 240],
        select: [214, 200, 245],
        milkdrop: [236, 120, 150],
        ..Palette::TEAL
    };
    /// Brighter text and borders, for bright venues or tired eyes.
    pub const HIGH_CONTRAST: Palette = Palette {
        bg_deep: [0, 0, 0],
        bg_panel: [16, 16, 16],
        bg_raised: [38, 38, 38],
        bg_header: [26, 26, 26],
        line: [110, 110, 110],
        text: [250, 250, 250],
        text_dim: [190, 190, 190],
        accent: [0, 200, 255],
        warn: [255, 70, 70],
        solo: [255, 220, 0],
        select: [255, 255, 255],
        milkdrop: [255, 90, 230],
    };

    pub const PRESETS: [(&'static str, Palette); 4] = [
        ("Teal", Palette::TEAL),
        ("Amber", Palette::AMBER),
        ("Violet", Palette::VIOLET),
        ("High Contrast", Palette::HIGH_CONTRAST),
    ];

    /// Editable roles with their labels, for the settings window.
    pub fn roles_mut(&mut self) -> [(&'static str, &mut [u8; 3]); 12] {
        [
            ("Accent", &mut self.accent),
            ("Selection", &mut self.select),
            ("MilkDrop", &mut self.milkdrop),
            ("Bypass", &mut self.warn),
            ("Solo", &mut self.solo),
            ("Text", &mut self.text),
            ("Dim text", &mut self.text_dim),
            ("Panels", &mut self.bg_panel),
            ("Headers", &mut self.bg_header),
            ("Buttons", &mut self.bg_raised),
            ("Background", &mut self.bg_deep),
            ("Lines", &mut self.line),
        ]
    }
}

static PALETTE: RwLock<Palette> = RwLock::new(Palette::TEAL);

fn get(role: impl Fn(&Palette) -> [u8; 3]) -> Color32 {
    let [r, g, b] = role(&PALETTE.read().map(|p| *p).unwrap_or(Palette::TEAL));
    Color32::from_rgb(r, g, b)
}

pub fn bg_deep() -> Color32 {
    get(|p| p.bg_deep)
}
pub fn bg_panel() -> Color32 {
    get(|p| p.bg_panel)
}
pub fn bg_raised() -> Color32 {
    get(|p| p.bg_raised)
}
pub fn bg_header() -> Color32 {
    get(|p| p.bg_header)
}
pub fn line() -> Color32 {
    get(|p| p.line)
}
pub fn text() -> Color32 {
    get(|p| p.text)
}
pub fn text_dim() -> Color32 {
    get(|p| p.text_dim)
}
pub fn accent() -> Color32 {
    get(|p| p.accent)
}
pub fn warn() -> Color32 {
    get(|p| p.warn)
}
pub fn solo() -> Color32 {
    get(|p| p.solo)
}
pub fn select() -> Color32 {
    get(|p| p.select)
}
pub fn milkdrop() -> Color32 {
    get(|p| p.milkdrop)
}

pub fn palette() -> Palette {
    PALETTE.read().map(|p| *p).unwrap_or(Palette::TEAL)
}

fn palette_file() -> Option<std::path::PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("colors.ron"))
}

/// Loads the saved palette, if any, and applies it.
pub fn load(ctx: &egui::Context) {
    let saved = palette_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .and_then(|text| ron::from_str::<Palette>(&text).ok());
    set(ctx, saved.unwrap_or(Palette::TEAL));
}

/// Makes `palette` current, restyles egui and saves it.
pub fn set(ctx: &egui::Context, palette: Palette) {
    if let Ok(mut current) = PALETTE.write() {
        *current = palette;
    }
    apply(ctx);
    if let (Some(file), Ok(text)) = (palette_file(), ron::to_string(&palette)) {
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(file, text);
    }
}

fn lighten(color: Color32, amount: u8) -> Color32 {
    Color32::from_rgb(
        color.r().saturating_add(amount),
        color.g().saturating_add(amount),
        color.b().saturating_add(amount),
    )
}

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
    v.override_text_color = Some(text());
    v.panel_fill = bg_panel();
    v.window_fill = bg_panel();
    v.extreme_bg_color = bg_deep();
    v.faint_bg_color = bg_header();
    v.selection.bg_fill = accent().gamma_multiply(0.45);
    v.selection.stroke = Stroke::new(1.0, accent());
    v.hyperlink_color = accent();
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
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, line());
    v.widgets.inactive.bg_fill = bg_raised();
    v.widgets.inactive.weak_bg_fill = bg_raised();
    v.widgets.hovered.weak_bg_fill = lighten(bg_raised(), 12);
    v.widgets.active.weak_bg_fill = lighten(bg_raised(), 20);
    ctx.set_global_style(style);
}

/// Window › Interface Colors.
pub fn settings_window(ctx: &egui::Context, open: &mut bool) {
    let mut palette = palette();
    let before = palette;
    egui::Window::new("Interface Colors")
        .open(open)
        .resizable(false)
        .default_pos([320.0, 140.0])
        .show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (name, preset) in Palette::PRESETS {
                    if ui.selectable_label(palette == preset, name).clicked() {
                        palette = preset;
                    }
                }
            });
            ui.separator();
            egui::Grid::new("colors")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for (label, color) in palette.roles_mut() {
                        ui.label(label);
                        ui.color_edit_button_srgb(color);
                        ui.end_row();
                    }
                });
            ui.separator();
            if ui.button("Reset to Default").clicked() {
                palette = Palette::TEAL;
            }
        });
    if palette != before {
        set(ctx, palette);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_round_trips_and_presets_differ() {
        let text = ron::to_string(&Palette::VIOLET).unwrap();
        assert_eq!(ron::from_str::<Palette>(&text).unwrap(), Palette::VIOLET);
        assert_ne!(Palette::TEAL, Palette::AMBER);
        let mut p = Palette::TEAL;
        *p.roles_mut()[0].1 = [1, 2, 3];
        assert_eq!(p.accent, [1, 2, 3]);
    }
}
