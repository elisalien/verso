//! Items of the browser: a list of rows, or a grid of thumbnails. Hovering
//! an item previews it in the Preview monitor; generators and shaders
//! animate in their thumbnails (or only under the pointer, with still
//! thumbnails), MilkDrop families show their preview image, image and video
//! files their picture or poster, and effects are shown applied to a
//! generator. Right-click an item to star it as a favorite.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, TextureId, Vec2};
use verso_core::{Command, Effect, Source};
use verso_render::Thumbnails;

use crate::app::{PropsTab, VersoApp};
use crate::grid::SourceTemplate;
use crate::hover_preview::HoverTarget;
use crate::library::source_key;
use crate::theme;

/// Size of the rendered browser thumbnails.
pub const THUMB_SIZE: (u32, u32) = (192, 108);
/// Smallest tile width in the thumbnail grid.
const TILE_MIN_WIDTH: f32 = 110.0;
const TILE_LABEL_HEIGHT: f32 = 16.0;
/// Thumbnail key of the picture effects are applied to in their tiles.
const EFFECT_INPUT: &str = "effect-input";

/// What a thumbnail shows.
#[derive(Clone)]
pub enum ThumbJob {
    Source(Source),
    /// A filter effect applied to the effect input picture.
    Effect(Effect),
}

/// Thumbnails of the generator, shader and effect items shown in the browser.
pub struct BrowserThumbs {
    pub thumbs: Thumbnails<String>,
    pub textures: HashMap<String, TextureId>,
    /// Visible tiles to render this frame.
    pub wanted: Vec<(String, ThumbJob)>,
}

impl BrowserThumbs {
    pub fn new(device: &eframe::wgpu::Device) -> Self {
        BrowserThumbs {
            thumbs: Thumbnails::new(device, THUMB_SIZE),
            textures: HashMap::new(),
            wanted: Vec::new(),
        }
    }
}

/// Key of a source's browser thumbnail.
fn thumb_key(source: &Source, name: &str) -> String {
    match source {
        Source::Shader(effect) => format!("shader:{}", effect.id),
        Source::Milkdrop { family, .. } => format!("milkdrop:{family}"),
        _ => format!("builtin:{name}"),
    }
}

/// Key of an effect's browser thumbnail.
pub fn effect_thumb_key(id: &str) -> String {
    format!("effect:{id}")
}

/// Columns and tile width of a thumbnail grid in the visible width.
pub(crate) fn tile_grid(ui: &egui::Ui) -> (usize, f32) {
    // The visible width: the panel may lay out wider than it shows.
    let available = ui
        .available_width()
        .min(ui.clip_rect().right() - ui.cursor().left())
        - 2.0;
    let spacing = ui.spacing().item_spacing.x;
    let columns = ((available + spacing) / (TILE_MIN_WIDTH + spacing))
        .floor()
        .max(1.0);
    let width = ((available + spacing) / columns - spacing).floor();
    (columns as usize, width)
}

/// What a tile looks like.
pub(crate) struct TileLook<'a> {
    pub name: &'a str,
    /// Texture and its aspect ratio.
    pub texture: Option<(TextureId, f32)>,
    pub accent: Color32,
    /// Outlined even without the pointer (MilkDrop families).
    pub outlined: bool,
    /// Starred or not; `None` for items that cannot be favorites.
    pub favorite: Option<bool>,
    /// Number shown in a badge (presets in a MilkDrop family).
    pub badge: Option<usize>,
}

/// Where the favorite star of a tile at `rect` sits: its top-left corner.
pub(crate) fn tile_star(rect: Rect) -> Rect {
    Rect::from_min_size(rect.min + Vec2::new(1.0, 1.0), Vec2::splat(18.0))
}

/// Paints the favorite star of a browser item in `rect`, to click: filled
/// when it is a favorite, else an outline, dim until the pointer is over the
/// item. Always there, so it is found without hovering. `backdrop` darkens
/// what is under it (a thumbnail), to keep it readable.
pub(crate) fn paint_star(
    painter: &egui::Painter,
    rect: Rect,
    favorite: bool,
    hovered: bool,
    backdrop: bool,
) {
    let (glyph, color) = match (favorite, hovered) {
        (true, _) => ("★", theme::solo()),
        (false, true) => ("☆", theme::text()),
        (false, false) => ("☆", theme::text_dim()),
    };
    if backdrop {
        painter.circle_filled(rect.center(), 8.5, Color32::from_black_alpha(150));
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        egui::FontId::proportional(13.0),
        color,
    );
}

/// Whether the click on a browser item landed on its star.
pub(crate) fn star_clicked(response: &egui::Response, star: Rect) -> bool {
    response.clicked()
        && response
            .interact_pointer_pos()
            .is_some_and(|pos| star.contains(pos))
}

/// Allocates and paints a thumbnail tile of `width`. Returns its response.
pub(crate) fn paint_tile(ui: &mut egui::Ui, width: f32, look: &TileLook) -> egui::Response {
    let image_height = (width * 9.0 / 16.0).round();
    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(width, image_height + TILE_LABEL_HEIGHT),
        Sense::click(),
    );
    let painter = ui.painter_at(rect);
    let image = Rect::from_min_size(rect.min, Vec2::new(width, image_height));
    painter.rect_filled(image, 2.0, theme::bg_deep());
    if let Some((texture, aspect)) = look.texture {
        // Letterboxed, so files keep their shape.
        let fit = if image.width() / image.height() > aspect {
            Vec2::new(image.height() * aspect, image.height())
        } else {
            Vec2::new(image.width(), image.width() / aspect)
        };
        let uv = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
        painter.image(
            texture,
            Rect::from_center_size(image.center(), fit),
            uv,
            Color32::WHITE,
        );
    }
    if let Some(count) = look.badge {
        let galley = painter.layout_no_wrap(
            count.to_string(),
            egui::FontId::monospace(9.0),
            theme::bg_deep(),
        );
        let badge = Rect::from_min_size(
            image.right_bottom() - galley.size() - Vec2::new(7.0, 4.0),
            galley.size() + Vec2::new(4.0, 1.0),
        );
        painter.rect_filled(badge, 2.0, look.accent);
        painter.galley(badge.min + Vec2::new(2.0, 0.5), galley, theme::bg_deep());
    }
    if let Some(favorite) = look.favorite {
        paint_star(
            &painter,
            tile_star(rect),
            favorite,
            response.contains_pointer(),
            true,
        );
    }
    // The drag source sits over the tile, so the tile is never "hovered"
    // itself: test the pointer position instead.
    if response.contains_pointer() {
        painter.rect_stroke(
            image,
            2.0,
            Stroke::new(2.0, look.accent),
            StrokeKind::Inside,
        );
    } else if look.outlined {
        painter.rect_stroke(
            image,
            2.0,
            Stroke::new(1.0, look.accent.gamma_multiply(0.6)),
            StrokeKind::Inside,
        );
    }
    let label = Rect::from_min_max(egui::pos2(rect.left(), image.bottom()), rect.max);
    let galley = painter.layout(
        look.name.to_owned(),
        egui::FontId::proportional(11.0),
        theme::text(),
        width - 4.0,
    );
    painter.with_clip_rect(label).galley(
        label.left_top() + Vec2::new(2.0, 1.0),
        galley,
        theme::text(),
    );
    response
}

impl VersoApp {
    /// `items` as rows, or as thumbnails in thumbnail mode, after the
    /// favorites filter and the search field.
    pub(crate) fn browser_items(
        &mut self,
        ui: &mut egui::Ui,
        items: Vec<(SourceTemplate, String)>,
    ) {
        let items: Vec<_> = items
            .into_iter()
            .filter(|(t, _)| self.browser_shows(&t.name, &source_key(t)))
            .collect();
        if !self.files.thumbnails {
            for (template, tip) in items {
                self.browser_item_with_tip(ui, template, &tip);
            }
            return;
        }
        let (columns, width) = tile_grid(ui);
        // Rows built by hand: `horizontal_wrapped` wraps at the layout
        // width, which can exceed what the panel shows.
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let row: Vec<_> = items.by_ref().take(columns).collect();
            ui.horizontal(|ui| {
                for (template, tip) in row {
                    self.browser_tile(ui, template, &tip, width);
                }
            });
        }
    }

    /// The Favorites tab: every starred source, MilkDrop family, effect and
    /// media file together, each used as in its own tab.
    pub(crate) fn favorites_browser(&mut self, ui: &mut egui::Ui) {
        crate::panels::hint(
            ui,
            "Everything you starred. Click the star of an item in any tab, or right-click it, to add or remove it.",
        );
        self.browser_toolbar(ui, true);
        let builtin = SourceTemplate::all();
        let (mut sources, mut milkdrop, mut files, mut effects) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for key in self.favorites.keys() {
            let Some((kind, id)) = key.split_once(':') else {
                continue;
            };
            let named = |source: Source| SourceTemplate {
                name: id.to_owned(),
                source,
            };
            match kind {
                "builtin" => sources.extend(
                    builtin
                        .iter()
                        .find(|t| t.name == id)
                        .map(|t| (t.clone(), String::new())),
                ),
                "shader" => sources.extend(self.renderer.library().get(id).map(|def| {
                    let template = SourceTemplate {
                        name: def.name.clone(),
                        source: Source::Shader(Effect::new(&def.id, &def.name)),
                    };
                    (template, String::new())
                })),
                "ndi" => sources.push((
                    named(Source::Ndi {
                        name: id.to_owned(),
                    }),
                    "NDI® source".to_owned(),
                )),
                "spout" => sources.push((
                    named(Source::Spout {
                        name: id.to_owned(),
                    }),
                    "Spout® source".to_owned(),
                )),
                "milkdrop" => {
                    milkdrop.extend(self.renderer.milkdrop().library().info(id).map(|info| {
                        let template = SourceTemplate {
                            name: info.name.clone(),
                            source: self.milk_prefs.source(&info.id, 0),
                        };
                        (template, info.authors.join(", "))
                    }))
                }
                "file" => files.extend(
                    SourceTemplate::from_path(std::path::Path::new(id)).map(|t| (t, id.to_owned())),
                ),
                "effect" => effects.extend(self.renderer.library().get(id).map(|def| {
                    crate::effects_ui::EffectTemplate {
                        id: def.id.clone(),
                        name: def.name.clone(),
                    }
                })),
                _ => {}
            }
        }
        effects.retain(|t| self.browser_shows(&t.name, &crate::library::effect_key(&t.id)));
        effects.sort_by_key(|t| t.name.to_lowercase());
        for list in [&mut sources, &mut milkdrop, &mut files] {
            list.retain(|(t, _)| self.browser_shows(&t.name, &source_key(t)));
            list.sort_by_key(|(t, _)| t.name.to_lowercase());
        }
        egui::ScrollArea::vertical()
            .id_salt("favorites browser")
            .show(ui, |ui| {
                if sources.is_empty()
                    && milkdrop.is_empty()
                    && files.is_empty()
                    && effects.is_empty()
                {
                    crate::panels::hint(ui, "No favorites here yet.");
                }
                for (title, items) in [
                    ("Sources", sources),
                    ("MilkDrop", milkdrop),
                    ("Media", files),
                ] {
                    if !items.is_empty() {
                        ui.add_space(4.0);
                        crate::panels::hint(ui, title);
                        self.browser_items(ui, items);
                    }
                }
                if !effects.is_empty() {
                    ui.add_space(4.0);
                    crate::panels::hint(ui, "Effects");
                    self.effect_items(ui, effects);
                }
            });
    }

    /// Texture showing `source` in a tile, with its aspect ratio, if there
    /// is one yet.
    fn tile_texture(
        &mut self,
        ctx: &egui::Context,
        template: &SourceTemplate,
    ) -> Option<(TextureId, f32)> {
        match &template.source {
            Source::Milkdrop { family, .. } => {
                self.milk_preview(ctx, family).map(|t| (t, 16.0 / 9.0))
            }
            // Stills drawn at the composition's aspect.
            Source::Text { .. } | Source::Time { .. } => {
                let aspect = self.comp.width as f32 / self.comp.height.max(1) as f32;
                self.media.text_tile(&template.source).map(|t| (t, aspect))
            }
            source if source.is_procedural() => {
                let key = thumb_key(source, &template.name);
                self.browser_thumbs
                    .textures
                    .get(&key)
                    .map(|&t| (t, 16.0 / 9.0))
            }
            Source::Image { .. } | Source::Video { .. } => self
                .media
                .file_thumbnail(&template.source)
                .map(|(t, (w, h))| (t, w as f32 / h.max(1) as f32)),
            _ => None,
        }
    }

    /// Asks for the thumbnail of a visible generator or shader tile to be
    /// rendered next frame. With still thumbnails, only once to get a still
    /// and then while the pointer is over it, so a browser full of shaders
    /// costs nothing at rest.
    fn want_tile(&mut self, ui: &egui::Ui, template: &SourceTemplate, response: &egui::Response) {
        let source = &template.source;
        if matches!(source, Source::Milkdrop { .. })
            || !source.is_procedural()
            || !ui.is_rect_visible(response.rect)
        {
            return;
        }
        let key = thumb_key(source, &template.name);
        if !self.files.still_thumbnails
            || response.contains_pointer()
            || !self.browser_thumbs.textures.contains_key(&key)
        {
            self.browser_thumbs
                .wanted
                .push((key, ThumbJob::Source(source.clone())));
        }
    }

    /// Asks for an effect tile's thumbnail, like [`Self::want_tile`].
    pub(crate) fn want_effect_tile(
        &mut self,
        ui: &egui::Ui,
        effect: Effect,
        response: &egui::Response,
    ) {
        if !ui.is_rect_visible(response.rect) {
            return;
        }
        let key = effect_thumb_key(&effect.id);
        if !self.files.still_thumbnails
            || response.contains_pointer()
            || !self.browser_thumbs.textures.contains_key(&key)
        {
            self.browser_thumbs
                .wanted
                .push((key, ThumbJob::Effect(effect)));
        }
    }

    /// Texture of an effect tile, if rendered yet.
    pub(crate) fn effect_tile_texture(&self, id: &str) -> Option<(TextureId, f32)> {
        self.browser_thumbs
            .textures
            .get(&effect_thumb_key(id))
            .map(|&t| (t, 16.0 / 9.0))
    }

    fn browser_tile(&mut self, ui: &mut egui::Ui, template: SourceTemplate, tip: &str, width: f32) {
        let texture = self.tile_texture(ui.ctx(), &template);
        let id = egui::Id::unique((
            "verso-browser-tile",
            thumb_key(&template.source, &template.name),
            template.source.path().map(|p| p.to_path_buf()),
        ));
        let milkdrop = matches!(template.source, Source::Milkdrop { .. });
        // Presets in a MilkDrop family, shown when there are several.
        let badge = match &template.source {
            Source::Milkdrop { family, .. } => self
                .renderer
                .milkdrop()
                .library()
                .info(family)
                .map(|info| info.variants.len())
                .filter(|&n| n > 1),
            _ => None,
        };
        let favorite = self.favorites.contains(&source_key(&template));
        let name = template.name.clone();
        let look = TileLook {
            name: &name,
            texture,
            accent: if milkdrop {
                theme::milkdrop()
            } else {
                theme::accent()
            },
            outlined: milkdrop,
            favorite: Some(favorite),
            badge,
        };
        let response = ui
            .dnd_drag_source(id, template.clone(), |ui| paint_tile(ui, width, &look))
            .inner;
        self.want_tile(ui, &template, &response);
        let star = tile_star(response.rect);
        self.browser_item_events(response, star, template, tip);
    }

    /// Hover preview, tooltip, star click, double-click and right-click menu
    /// of a browser item whose favorite star is at `star`.
    pub(crate) fn browser_item_events(
        &mut self,
        response: egui::Response,
        star: Rect,
        template: SourceTemplate,
        tip: &str,
    ) {
        let response = if tip.is_empty() {
            response
        } else {
            response.on_hover_text(tip)
        };
        // Files have no live preview; the other sources play in the monitor.
        if response.contains_pointer() && template.source.is_procedural() {
            self.hover_target = Some(HoverTarget::Source);
            self.hover_source = Some((template.name.clone(), template.source.clone()));
        }
        let key = source_key(&template);
        let favorite = self.favorites.contains(&key);
        let has_slot = self.selected_clip.is_some();
        let on_star = star_clicked(&response, star);
        let mut toggle = on_star;
        let mut load = response.double_clicked() && !on_star;
        response.context_menu(|ui| {
            let label = if favorite {
                "★ Remove from Favorites"
            } else {
                "☆ Add to Favorites"
            };
            if ui.button(label).clicked() {
                toggle = true;
                ui.close();
            }
            if ui
                .add_enabled(has_slot, egui::Button::new("Load into Selected Slot"))
                .clicked()
            {
                load = true;
                ui.close();
            }
        });
        if toggle {
            self.favorites.toggle(&key);
        }
        if load && let Some((layer, column)) = self.selected_clip {
            self.send(Command::SetClip {
                layer,
                column,
                clip: Some(template.clip()),
            });
            self.props_tab = PropsTab::Clip;
        }
    }

    /// Renders the thumbnails of the tiles drawn last frame and registers
    /// the new ones with egui.
    pub(crate) fn render_browser_thumbs(
        &mut self,
        encoder: &mut eframe::wgpu::CommandEncoder,
        time: f32,
    ) {
        let render_state = self.render_state.clone();
        let (device, queue) = (&render_state.device, &render_state.queue);
        let wanted = std::mem::take(&mut self.browser_thumbs.wanted);
        // Effects are shown applied to the first built-in generator.
        if wanted
            .iter()
            .any(|(_, job)| matches!(job, ThumbJob::Effect(_)))
            && let Some(input) = SourceTemplate::all().into_iter().next()
        {
            self.browser_thumbs.thumbs.render(
                device,
                queue,
                encoder,
                EFFECT_INPUT.to_owned(),
                &input.source,
                time,
            );
        }
        let mut created = Vec::new();
        for (key, job) in wanted {
            let new = match &job {
                ThumbJob::Source(Source::Shader(effect)) => {
                    let (view, new) = self.browser_thumbs.thumbs.target(device, key.clone());
                    let view = view.clone();
                    self.renderer.render_shader_preview(
                        device, queue, encoder, effect, &view, THUMB_SIZE, time,
                    );
                    new
                }
                ThumbJob::Source(source) => self.browser_thumbs.thumbs.render(
                    device,
                    queue,
                    encoder,
                    key.clone(),
                    source,
                    time,
                ),
                ThumbJob::Effect(effect) => {
                    let Some(input) = self
                        .browser_thumbs
                        .thumbs
                        .view(&EFFECT_INPUT.to_owned())
                        .cloned()
                    else {
                        continue;
                    };
                    let (view, new) = self.browser_thumbs.thumbs.target(device, key.clone());
                    let view = view.clone();
                    self.renderer.render_filter_preview(
                        device, queue, encoder, effect, &input, &view, THUMB_SIZE, time,
                    );
                    new
                }
            };
            if new || !self.browser_thumbs.textures.contains_key(&key) {
                created.push(key);
            }
        }
        if created.is_empty() {
            return;
        }
        let mut egui_renderer = render_state.renderer.write();
        for key in created {
            if let Some(view) = self.browser_thumbs.thumbs.view(&key) {
                let id = egui_renderer.register_native_texture(
                    device,
                    view,
                    eframe::wgpu::FilterMode::Linear,
                );
                self.browser_thumbs.textures.insert(key, id);
            }
        }
    }
}
