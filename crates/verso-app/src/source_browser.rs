//! Items of the Sources browser: a list of rows, or a grid of thumbnails.
//! Hovering an item previews it in the Preview monitor; generators and
//! shaders animate in their thumbnails (or only under the pointer, with
//! still thumbnails), MilkDrop families show their preview image.

use std::collections::HashMap;

use eframe::egui::{self, Color32, Sense, Stroke, StrokeKind, TextureId, Vec2};
use verso_core::{Command, Source};
use verso_render::Thumbnails;

use crate::app::{PropsTab, VersoApp};
use crate::grid::SourceTemplate;
use crate::hover_preview::HoverTarget;
use crate::theme;

/// Size of the rendered browser thumbnails.
pub const THUMB_SIZE: (u32, u32) = (192, 108);
/// Smallest tile width in the thumbnail grid.
const TILE_MIN_WIDTH: f32 = 110.0;
const TILE_LABEL_HEIGHT: f32 = 16.0;

/// Thumbnails of the generator and shader sources shown in the browser.
pub struct BrowserThumbs {
    pub thumbs: Thumbnails<String>,
    pub textures: HashMap<String, TextureId>,
    /// Visible tiles to render this frame.
    pub wanted: Vec<(String, Source)>,
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

impl VersoApp {
    /// `items` as rows, or as thumbnails in thumbnail mode.
    pub(crate) fn browser_items(
        &mut self,
        ui: &mut egui::Ui,
        items: Vec<(SourceTemplate, String)>,
    ) {
        if !self.files.thumbnails {
            for (template, tip) in items {
                self.browser_item_with_tip(ui, template, &tip);
            }
            return;
        }
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
        // Rows built by hand: `horizontal_wrapped` wraps at the layout
        // width, which can exceed what the panel shows.
        let mut items = items.into_iter().peekable();
        while items.peek().is_some() {
            let row: Vec<_> = items.by_ref().take(columns as usize).collect();
            ui.horizontal(|ui| {
                for (template, tip) in row {
                    self.browser_tile(ui, template, &tip, width);
                }
            });
        }
    }

    /// Texture showing `source` in a tile, if there is one yet.
    fn tile_texture(
        &mut self,
        ctx: &egui::Context,
        template: &SourceTemplate,
    ) -> Option<TextureId> {
        match &template.source {
            Source::Milkdrop { family, .. } => self.milk_preview(ctx, family),
            source if source.is_procedural() => {
                let key = thumb_key(source, &template.name);
                self.browser_thumbs.textures.get(&key).copied()
            }
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
            self.browser_thumbs.wanted.push((key, source.clone()));
        }
    }

    fn browser_tile(&mut self, ui: &mut egui::Ui, template: SourceTemplate, tip: &str, width: f32) {
        let texture = self.tile_texture(ui.ctx(), &template);
        let id = egui::Id::unique((
            "verso-browser-tile",
            thumb_key(&template.source, &template.name),
        ));
        let image_height = (width * 9.0 / 16.0).round();
        let name = template.name.clone();
        // Presets in a MilkDrop family, shown when there are several.
        let presets = match &template.source {
            Source::Milkdrop { family, .. } => self
                .renderer
                .milkdrop()
                .library()
                .info(family)
                .map(|info| info.variants.len())
                .filter(|&n| n > 1),
            _ => None,
        };
        let response = ui
            .dnd_drag_source(id, template.clone(), |ui| {
                let (rect, response) = ui.allocate_exact_size(
                    Vec2::new(width, image_height + TILE_LABEL_HEIGHT),
                    Sense::click(),
                );
                let painter = ui.painter_at(rect);
                let image = egui::Rect::from_min_size(rect.min, Vec2::new(width, image_height));
                painter.rect_filled(image, 2.0, theme::bg_deep());
                if let Some(texture) = texture {
                    let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
                    painter.image(texture, image, uv, Color32::WHITE);
                }
                // The drag source sits over the tile, so the tile is never
                // "hovered" itself: test the pointer position instead.
                if let Some(count) = presets {
                    let galley = painter.layout_no_wrap(
                        count.to_string(),
                        egui::FontId::monospace(9.0),
                        theme::bg_deep(),
                    );
                    let badge = egui::Rect::from_min_size(
                        image.right_bottom() - galley.size() - Vec2::new(7.0, 4.0),
                        galley.size() + Vec2::new(4.0, 1.0),
                    );
                    painter.rect_filled(badge, 2.0, theme::milkdrop());
                    painter.galley(badge.min + Vec2::new(2.0, 0.5), galley, theme::bg_deep());
                }
                let milkdrop = matches!(template.source, Source::Milkdrop { .. });
                if response.contains_pointer() {
                    let color = if milkdrop {
                        theme::milkdrop()
                    } else {
                        theme::accent()
                    };
                    painter.rect_stroke(image, 2.0, Stroke::new(2.0, color), StrokeKind::Inside);
                } else if milkdrop {
                    painter.rect_stroke(
                        image,
                        2.0,
                        Stroke::new(1.0, theme::milkdrop().gamma_multiply(0.6)),
                        StrokeKind::Inside,
                    );
                }
                let label =
                    egui::Rect::from_min_max(egui::pos2(rect.left(), image.bottom()), rect.max);
                let galley = painter.layout(
                    name.clone(),
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
            })
            .inner;
        self.want_tile(ui, &template, &response);
        self.browser_item_events(response, template, tip);
    }

    /// Hover preview, tooltip and double-click of a browser item.
    pub(crate) fn browser_item_events(
        &mut self,
        response: egui::Response,
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
        if response.double_clicked()
            && let Some((layer, column)) = self.selected_clip
        {
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
        let mut created = Vec::new();
        for (key, source) in wanted {
            let new = match &source {
                Source::Shader(effect) => {
                    let (view, new) = self.browser_thumbs.thumbs.target(device, key.clone());
                    let view = view.clone();
                    self.renderer.render_shader_preview(
                        device, queue, encoder, effect, &view, THUMB_SIZE, time,
                    );
                    new
                }
                source => self.browser_thumbs.thumbs.render(
                    device,
                    queue,
                    encoder,
                    key.clone(),
                    source,
                    time,
                ),
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
