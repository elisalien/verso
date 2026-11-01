//! Preview monitor for what the pointer is over: a layer or column header, a
//! MilkDrop clip, or a source in the browser. A second renderer draws a copy
//! of the composition with only that layer, that column or that clip
//! launched, or a one-clip composition of the source, without touching the
//! output. It stays alive between hovers, so MilkDrop shaders compile once.

use eframe::egui::TextureId;
use eframe::egui_wgpu;
use verso_core::{Clip, Command, Composition, FaderSide, Source};
use verso_render::{MediaView, Renderer};

/// Width of the hover preview; the height follows the composition aspect.
const WIDTH: u32 = 640;

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoverTarget {
    Layer(usize),
    Column(usize),
    /// A clip slot, previewed live (MilkDrop clips).
    Clip(usize, usize),
    /// A source in the browser; the source itself is kept by the app.
    Source,
}

#[derive(Default)]
pub struct HoverPreview {
    renderer: Option<Renderer>,
    texture: Option<TextureId>,
    /// Target drawn last frame.
    shown: Option<HoverTarget>,
}

/// `comp` reduced to what `target` would show.
pub fn preview_composition(comp: &Composition, target: HoverTarget) -> Composition {
    let mut comp = comp.clone();
    comp.master.opacity = 1.0;
    for layer in &mut comp.layers {
        // No crossfades: the preview shows the destination at once.
        layer.transition = 0.0;
    }
    match target {
        HoverTarget::Layer(index) => {
            for (i, layer) in comp.layers.iter_mut().enumerate() {
                layer.solo = i == index;
                if i == index {
                    layer.bypass = false;
                    layer.opacity = 1.0;
                    layer.fader = FaderSide::None;
                }
            }
            for group in &mut comp.groups {
                group.bypass = false;
                group.opacity = 1.0;
            }
        }
        HoverTarget::Column(column) => {
            let _ = comp.apply(Command::TriggerColumn(column));
        }
        HoverTarget::Clip(layer, column) => {
            let _ = comp.apply(Command::TriggerClip { layer, column });
            return preview_composition(&comp, HoverTarget::Layer(layer));
        }
        // Built with `source_composition`.
        HoverTarget::Source => {}
    }
    comp
}

/// A one-layer composition playing `source`, for previewing a browser item.
pub fn source_composition(source: &Source, name: &str) -> Composition {
    let mut comp = Composition::new("preview", 1, 1);
    let mut source = source.clone();
    if let Source::Milkdrop { blend, .. } = &mut source {
        // Moving between presets in the browser cuts instead of blending.
        *blend = 0.0;
    }
    let _ = comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(Clip::new(name, source)),
    });
    let _ = comp.apply(Command::TriggerColumn(0));
    comp.layers[0].transition = 0.0;
    comp
}

impl HoverPreview {
    /// Texture of the preview, once `target` has been rendered.
    pub fn texture(&self, target: HoverTarget) -> Option<TextureId> {
        self.texture.filter(|_| self.shown == Some(target))
    }

    /// Renderer that drew `target`, for reading its output back.
    pub fn renderer(&self, target: HoverTarget) -> Option<&Renderer> {
        self.renderer
            .as_ref()
            .filter(|_| self.shown == Some(target))
    }

    /// Renders `target` of `comp`. `setup` fills a new renderer's effect and
    /// MilkDrop libraries.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        render_state: &egui_wgpu::RenderState,
        encoder: &mut eframe::wgpu::CommandEncoder,
        comp: &Composition,
        target: HoverTarget,
        media: &[Option<MediaView<'_>>],
        time: f32,
        setup: impl FnOnce(&mut Renderer),
    ) {
        let device = &render_state.device;
        let height = (WIDTH as f32 * comp.height as f32 / comp.width.max(1) as f32).round() as u32;
        let size = (WIDTH, height.max(1));
        self.shown = Some(target);
        let renderer = self.renderer.get_or_insert_with(|| {
            let mut renderer = Renderer::new(device, size);
            setup(&mut renderer);
            renderer
        });
        let mut preview = preview_composition(comp, target);
        preview.width = size.0;
        preview.height = size.1;
        renderer.render(device, &render_state.queue, encoder, &preview, media, time);

        // The output view changes with a new renderer or size: point the egui
        // texture at the current one.
        let mut egui_renderer = render_state.renderer.write();
        let filter = eframe::wgpu::FilterMode::Linear;
        match self.texture {
            Some(id) => egui_renderer.update_egui_texture_from_wgpu_texture(
                device,
                renderer.output_view(),
                filter,
                id,
            ),
            None => {
                self.texture = Some(egui_renderer.register_native_texture(
                    device,
                    renderer.output_view(),
                    filter,
                ))
            }
        }
    }

    /// Nothing is hovered. The renderer stays, with its compiled shaders.
    pub fn idle(&mut self) {
        self.shown = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp() -> Composition {
        let mut comp = Composition::new("t", 2, 2);
        for layer in 0..2 {
            for column in 0..2 {
                comp.apply(Command::SetClip {
                    layer,
                    column,
                    clip: Some(Clip::new("c", Source::Solid { color: [1.0; 4] })),
                })
                .unwrap();
            }
        }
        comp
    }

    #[test]
    fn layer_preview_solos_the_layer() {
        let mut comp = comp();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.layers[1].opacity = 0.0;
        let preview = preview_composition(&comp, HoverTarget::Layer(1));
        assert_eq!(preview.layer_gain(0), 0.0);
        assert_eq!(preview.layer_gain(1), 1.0);
    }

    #[test]
    fn column_preview_launches_the_column() {
        let comp = comp();
        let preview = preview_composition(&comp, HoverTarget::Column(1));
        assert!(preview.layers.iter().all(|l| l.active == Some(1)));
        assert!(comp.layers.iter().all(|l| l.active.is_none()));
    }

    #[test]
    fn clip_preview_launches_and_solos_the_clip() {
        let comp = comp();
        let preview = preview_composition(&comp, HoverTarget::Clip(1, 1));
        assert_eq!(preview.layers[1].active, Some(1));
        assert_eq!(preview.layer_gain(0), 0.0);
        assert_eq!(preview.layer_gain(1), 1.0);
    }

    #[test]
    fn source_preview_plays_the_source_without_blend() {
        let preview = source_composition(&Source::milkdrop("a/b", 2), "b");
        assert_eq!(preview.layers.len(), 1);
        assert_eq!(preview.layer_gain(0), 1.0);
        let source = &preview.layers[0].active_clip().unwrap().source;
        assert!(matches!(source, Source::Milkdrop { variant: 2, blend, .. } if *blend == 0.0));
    }
}
