//! Live previews of single sources, for clip cells and the clip monitor.

use std::collections::HashMap;
use std::hash::Hash;

use verso_core::Source;

use crate::{
    GeneratorParams, OUTPUT_FORMAT, Target, fullscreen_pipeline, pass, uniform_buffer,
    uniform_entry,
};

struct Thumb {
    target: Target,
    params: wgpu::Buffer,
    group: wgpu::BindGroup,
}

/// A set of same-size source previews keyed by `K`, rendered in [`OUTPUT_FORMAT`].
pub struct Thumbnails<K> {
    size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    thumbs: HashMap<K, Thumb>,
}

impl<K: Hash + Eq + Clone> Thumbnails<K> {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("thumbnail"),
            entries: &[uniform_entry(0)],
        });
        let pipeline = fullscreen_pipeline(
            device,
            "thumbnail",
            include_str!("shaders/generator.wgsl"),
            &layout,
            OUTPUT_FORMAT,
        );
        Self {
            size,
            pipeline,
            layout,
            thumbs: HashMap::new(),
        }
    }

    /// Renders `source` into the preview for `key`, creating it if needed.
    /// Returns `true` when the preview was just created, so its view is new.
    /// File-based sources are not rendered here and return `false`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        key: K,
        source: &Source,
        time: f32,
    ) -> bool {
        let aspect = self.size.0 as f32 / self.size.1 as f32;
        let Some(params) = GeneratorParams::new(source, time, aspect) else {
            return false;
        };
        let created = self.thumb(device, key.clone()).1;
        let thumb = &self.thumbs[&key];
        queue.write_buffer(&thumb.params, 0, bytemuck::bytes_of(&params));
        let mut rp = pass(encoder, &thumb.target.view, true);
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &thumb.group, &[]);
        rp.draw(0..3, 0..1);
        created
    }

    /// The preview texture for `key`, for drawing into it with another
    /// renderer (generator shaders). Returns the view and whether it is new.
    pub fn target(&mut self, device: &wgpu::Device, key: K) -> (&wgpu::TextureView, bool) {
        let (thumb, created) = self.thumb(device, key);
        (&thumb.target.view, created)
    }

    fn thumb(&mut self, device: &wgpu::Device, key: K) -> (&Thumb, bool) {
        let mut created = false;
        let thumb = self.thumbs.entry(key).or_insert_with(|| {
            created = true;
            let params = uniform_buffer(device, "thumbnail params", size_of::<GeneratorParams>());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("thumbnail"),
                layout: &self.layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: params.as_entire_binding(),
                }],
            });
            Thumb {
                target: Target::new(device, "thumbnail", self.size, OUTPUT_FORMAT),
                params,
                group,
            }
        });
        (thumb, created)
    }

    pub fn view(&self, key: &K) -> Option<&wgpu::TextureView> {
        self.thumbs.get(key).map(|t| &t.target.view)
    }

    pub fn texture(&self, key: &K) -> Option<&wgpu::Texture> {
        self.thumbs.get(key).map(|t| &t.target.texture)
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }
}
