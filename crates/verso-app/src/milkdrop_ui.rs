//! MilkDrop presets in the app: the library folders, the browser section,
//! preview images and the preset picker of a clip.

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;
use verso_core::Source;

use crate::app::VersoApp;
use crate::grid::SourceTemplate;
use crate::theme;

/// Folders scanned for MilkDrop presets at startup: the bundled
/// `presets/milkdrop` next to the executable and in the working directory,
/// then the user's own folder.
pub fn milkdrop_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("presets").join("milkdrop")];
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("presets").join("milkdrop")))
    {
        dirs.push(exe_dir);
    }
    dirs.extend(user_milkdrop_dir());
    dirs
}

/// Where users put their own Butterchurn `.json` presets.
pub fn user_milkdrop_dir() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("milkdrop"))
}

#[derive(Default)]
pub struct MilkState {
    /// Preview image per family id; `None` when it has none or failed.
    previews: HashMap<String, Option<egui::TextureHandle>>,
    search: String,
}

fn load_preview(
    ctx: &egui::Context,
    id: &str,
    path: &std::path::Path,
) -> Option<egui::TextureHandle> {
    let image = image::open(path).ok()?.to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let color = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
    Some(ctx.load_texture(
        format!("milkdrop {id}"),
        color,
        egui::TextureOptions::LINEAR,
    ))
}

impl VersoApp {
    /// Preview image of a family, loaded on first use.
    pub(crate) fn milk_preview(
        &mut self,
        ctx: &egui::Context,
        family: &str,
    ) -> Option<egui::TextureId> {
        if !self.milk.previews.contains_key(family) {
            let texture = self
                .renderer
                .milkdrop()
                .library()
                .info(family)
                .and_then(|info| info.preview.clone())
                .and_then(|path| load_preview(ctx, family, &path));
            self.milk.previews.insert(family.to_owned(), texture);
        }
        self.milk.previews[family].as_ref().map(|t| t.id())
    }

    /// Browser section listing the preset families by category.
    pub(crate) fn milkdrop_browser(&mut self, ui: &mut egui::Ui) {
        let families: Vec<_> = self
            .renderer
            .milkdrop()
            .library()
            .sorted()
            .into_iter()
            .cloned()
            .collect();
        if families.is_empty() {
            return;
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("MilkDrop")
                    .small()
                    .color(theme::text_dim()),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.milk.search)
                    .hint_text("Search presets")
                    .desired_width(ui.available_width()),
            );
        });
        let search = self.milk.search.to_lowercase();
        let matches = |info: &verso_milkdrop::FamilyInfo| {
            search.is_empty()
                || info.name.to_lowercase().contains(&search)
                || info
                    .authors
                    .iter()
                    .any(|a| a.to_lowercase().contains(&search))
                || info
                    .variants
                    .iter()
                    .any(|v| v.to_lowercase().contains(&search))
        };
        let mut categories: Vec<&str> = families.iter().map(|f| f.category.as_str()).collect();
        categories.dedup();
        for category in categories {
            let in_category: Vec<_> = families
                .iter()
                .filter(|f| f.category == category && matches(f))
                .collect();
            if in_category.is_empty() {
                continue;
            }
            egui::CollapsingHeader::new(format!("{category} ({})", in_category.len()))
                .id_salt(("milkdrop category", category))
                .default_open(!search.is_empty())
                .show(ui, |ui| {
                    for info in in_category {
                        let template = SourceTemplate {
                            name: info.name.clone(),
                            source: Source::milkdrop(&info.id, 0),
                        };
                        let mut tip = info.authors.join(", ");
                        if info.variants.len() > 1 {
                            tip.push_str(&format!("\n{} presets", info.variants.len()));
                        }
                        self.browser_item_with_tip(ui, template, &tip);
                    }
                });
        }
    }

    /// Blend time and input layer of a MilkDrop clip on `layer`.
    pub(crate) fn milkdrop_playback(
        &mut self,
        ui: &mut egui::Ui,
        layer: usize,
        blend: &mut f32,
        input: &mut Option<usize>,
        input_mix: &mut f32,
    ) {
        ui.label("Blend");
        ui.add(egui::Slider::new(blend, 0.0..=10.0).suffix(" s"))
            .on_hover_text("MilkDrop blend when the preset changes; 0 cuts");
        ui.end_row();

        let layer_name = |i: usize| {
            self.comp
                .layers
                .get(i)
                .map_or_else(|| format!("Layer {}", i + 1), |l| l.name.clone())
        };
        ui.label("Input");
        egui::ComboBox::from_id_salt("milkdrop input")
            .selected_text(input.map_or_else(|| "None".to_owned(), layer_name))
            .show_ui(ui, |ui| {
                ui.selectable_value(input, None, "None");
                for i in (0..self.comp.layers.len()).filter(|&i| i != layer) {
                    ui.selectable_value(input, Some(i), layer_name(i));
                }
            })
            .response
            .on_hover_text(
                "Another layer's image, read by the preset as its image textures. \
                 Set that layer's opacity to 0 to use it only as an input.",
            );
        ui.end_row();
        if input.is_some() {
            ui.label("Input Mix");
            ui.add(egui::Slider::new(input_mix, 0.0..=1.0))
                .on_hover_text("How much of the input is mixed into the feedback each frame");
            ui.end_row();
        }
    }

    /// Clip properties of a MilkDrop source. Returns the new variant when the
    /// user picked another preset of the family.
    pub(crate) fn milkdrop_props(
        &mut self,
        ui: &mut egui::Ui,
        family: &str,
        variant: usize,
    ) -> Option<usize> {
        let info = self.renderer.milkdrop().library().info(family).cloned();
        let Some(info) = info else {
            ui.label("Source");
            ui.label(egui::RichText::new("MilkDrop preset not found").color(theme::warn()))
                .on_hover_text(family);
            ui.end_row();
            return None;
        };
        let mut picked = None;
        ui.label("Source");
        ui.label(format!("MilkDrop · {}", info.name))
            .on_hover_text(&info.id);
        ui.end_row();
        if !info.authors.is_empty() {
            ui.label("Credit");
            ui.label(
                egui::RichText::new(info.authors.join(", "))
                    .small()
                    .color(theme::text_dim()),
            );
            ui.end_row();
        }
        let count = info.variants.len();
        let current = variant.min(count - 1);
        ui.label("Preset");
        ui.horizontal(|ui| {
            if count > 1 {
                if ui
                    .small_button("◀")
                    .on_hover_text("Previous preset")
                    .clicked()
                {
                    picked = Some((current + count - 1) % count);
                }
                if ui.small_button("▶").on_hover_text("Next preset").clicked() {
                    picked = Some((current + 1) % count);
                }
            }
            egui::ComboBox::from_id_salt("milkdrop variant")
                .selected_text(format!("{}/{count}", current + 1))
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (i, name) in info.variants.iter().enumerate() {
                        if ui.selectable_label(i == current, name).clicked() {
                            picked = Some(i);
                        }
                    }
                });
        });
        ui.end_row();
        ui.label("");
        ui.label(
            egui::RichText::new(&info.variants[current])
                .small()
                .color(theme::text_dim()),
        );
        ui.end_row();
        if let Some(error) = self.renderer.milkdrop().error(family, current) {
            let first = error.lines().find(|l| l.contains("error")).unwrap_or(error);
            ui.label("");
            ui.label(egui::RichText::new("This preset cannot run").color(theme::warn()))
                .on_hover_text(first);
            ui.end_row();
        }
        picked.filter(|&p| p != current)
    }
}
