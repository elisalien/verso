//! wgpu compositing engine for Verso.
//!
//! Each visible layer renders its source into its own texture, then blends onto
//! an accumulator. Two accumulators alternate (ping-pong) so a blend pass can
//! read the result of the previous one. A final pass applies the master and
//! writes the output texture.

mod thumbnails;

use bytemuck::{Pod, Zeroable};
use verso_core::{Composition, Source};

pub use thumbnails::Thumbnails;

/// Internal format: HDR so successive blends do not band.
const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of [`Renderer::output_view`].
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Shader index of a solid color in `generator.wgsl`.
const SOLID_KIND: u32 = 255;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct GeneratorParams {
    time: f32,
    speed: f32,
    kind: u32,
    aspect: f32,
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct BlendParams {
    mode: u32,
    opacity: f32,
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct OutputParams {
    opacity: f32,
    _pad: [f32; 3],
}

impl GeneratorParams {
    fn new(source: &Source, time: f32, aspect: f32) -> Self {
        let (kind, color, speed) = match *source {
            Source::Solid { color } => (SOLID_KIND, color, 0.0),
            Source::Generator { kind, color, speed } => (kind.shader_index(), color, speed),
        };
        Self {
            time,
            speed,
            kind,
            aspect,
            color,
        }
    }
}

struct Target {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
}

impl Target {
    fn new(
        device: &wgpu::Device,
        label: &str,
        size: (u32, u32),
        format: wgpu::TextureFormat,
    ) -> Self {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}

/// GPU resources owned by one layer of the composition.
struct LayerSlot {
    target: Target,
    generator_params: wgpu::Buffer,
    generator_group: wgpu::BindGroup,
    blend_params: wgpu::Buffer,
    /// Blend bind group reading accumulator 0 or 1 as base.
    blend_groups: [wgpu::BindGroup; 2],
}

pub struct Renderer {
    size: (u32, u32),
    generator_pipeline: wgpu::RenderPipeline,
    generator_layout: wgpu::BindGroupLayout,
    blend_pipeline: wgpu::RenderPipeline,
    blend_layout: wgpu::BindGroupLayout,
    output_pipeline: wgpu::RenderPipeline,
    output_layout: wgpu::BindGroupLayout,
    accumulators: [Target; 2],
    output: Target,
    output_params: wgpu::Buffer,
    /// Output bind group reading accumulator 0 or 1.
    output_groups: [wgpu::BindGroup; 2],
    slots: Vec<LayerSlot>,
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn texture_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: false },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn uniform_buffer(device: &wgpu::Device, label: &str, size: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &module,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    clear: bool,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: if clear {
                    wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                } else {
                    wgpu::LoadOp::Load
                },
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

impl Renderer {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let generator_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("generator"),
            entries: &[uniform_entry(0)],
        });
        let blend_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blend"),
            entries: &[uniform_entry(0), texture_entry(1), texture_entry(2)],
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("output"),
            entries: &[uniform_entry(0), texture_entry(1)],
        });

        let generator_pipeline = fullscreen_pipeline(
            device,
            "generator",
            include_str!("shaders/generator.wgsl"),
            &generator_layout,
            WORK_FORMAT,
        );
        let blend_pipeline = fullscreen_pipeline(
            device,
            "blend",
            include_str!("shaders/blend.wgsl"),
            &blend_layout,
            WORK_FORMAT,
        );
        let output_pipeline = fullscreen_pipeline(
            device,
            "output",
            include_str!("shaders/output.wgsl"),
            &output_layout,
            OUTPUT_FORMAT,
        );

        let accumulators = [
            Target::new(device, "accumulator 0", size, WORK_FORMAT),
            Target::new(device, "accumulator 1", size, WORK_FORMAT),
        ];
        let output = Target::new(device, "output", size, OUTPUT_FORMAT);
        let output_params = uniform_buffer(device, "output params", size_of::<OutputParams>());
        let output_groups =
            Self::output_groups(device, &output_layout, &output_params, &accumulators);

        Self {
            size,
            generator_pipeline,
            generator_layout,
            blend_pipeline,
            blend_layout,
            output_pipeline,
            output_layout,
            accumulators,
            output,
            output_params,
            output_groups,
            slots: Vec::new(),
        }
    }

    /// Composition size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Final image, in [`OUTPUT_FORMAT`]. Recreated when the size changes.
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output.view
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output.texture
    }

    fn output_groups(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        params: &wgpu::Buffer,
        accumulators: &[Target; 2],
    ) -> [wgpu::BindGroup; 2] {
        accumulators.each_ref().map(|acc| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("output"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&acc.view),
                    },
                ],
            })
        })
    }

    fn new_slot(&self, device: &wgpu::Device) -> LayerSlot {
        let target = Target::new(device, "layer", self.size, WORK_FORMAT);
        let generator_params =
            uniform_buffer(device, "generator params", size_of::<GeneratorParams>());
        let generator_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("generator"),
            layout: &self.generator_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: generator_params.as_entire_binding(),
            }],
        });
        let blend_params = uniform_buffer(device, "blend params", size_of::<BlendParams>());
        let blend_groups = self.accumulators.each_ref().map(|base| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("blend"),
                layout: &self.blend_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: blend_params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(&base.view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(&target.view),
                    },
                ],
            })
        });
        LayerSlot {
            target,
            generator_params,
            generator_group,
            blend_params,
            blend_groups,
        }
    }

    /// Recreates every size-dependent resource.
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return;
        }
        self.size = size;
        self.accumulators = [
            Target::new(device, "accumulator 0", size, WORK_FORMAT),
            Target::new(device, "accumulator 1", size, WORK_FORMAT),
        ];
        self.output = Target::new(device, "output", size, OUTPUT_FORMAT);
        self.output_groups = Self::output_groups(
            device,
            &self.output_layout,
            &self.output_params,
            &self.accumulators,
        );
        // Slots hold views of the old accumulators; rebuild lazily.
        self.slots.clear();
    }

    /// Renders `comp` at `time` seconds into the output texture.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        comp: &Composition,
        time: f32,
    ) {
        self.resize(device, (comp.width, comp.height));
        while self.slots.len() < comp.layers.len() {
            let slot = self.new_slot(device);
            self.slots.push(slot);
        }

        let aspect = self.size.0 as f32 / self.size.1 as f32;
        // Accumulator holding the result so far.
        let mut current = 0;
        drop(pass(encoder, &self.accumulators[current].view, true));

        for (index, (layer, slot)) in comp.layers.iter().zip(&self.slots).enumerate() {
            let gain = comp.layer_gain(index);
            let Some(clip) = layer.active_clip().filter(|_| gain > 0.0) else {
                continue;
            };
            queue.write_buffer(
                &slot.generator_params,
                0,
                bytemuck::bytes_of(&GeneratorParams::new(&clip.source, time, aspect)),
            );
            queue.write_buffer(
                &slot.blend_params,
                0,
                bytemuck::bytes_of(&BlendParams {
                    mode: layer.blend.shader_index(),
                    opacity: gain,
                    _pad: [0.0; 2],
                }),
            );

            {
                let mut rp = pass(encoder, &slot.target.view, true);
                rp.set_pipeline(&self.generator_pipeline);
                rp.set_bind_group(0, &slot.generator_group, &[]);
                rp.draw(0..3, 0..1);
            }

            let next = 1 - current;
            {
                let mut rp = pass(encoder, &self.accumulators[next].view, true);
                rp.set_pipeline(&self.blend_pipeline);
                rp.set_bind_group(0, &slot.blend_groups[current], &[]);
                rp.draw(0..3, 0..1);
            }
            current = next;
        }

        queue.write_buffer(
            &self.output_params,
            0,
            bytemuck::bytes_of(&OutputParams {
                opacity: comp.master.opacity,
                _pad: [0.0; 3],
            }),
        );
        let mut rp = pass(encoder, &self.output.view, true);
        rp.set_pipeline(&self.output_pipeline);
        rp.set_bind_group(0, &self.output_groups[current], &[]);
        rp.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::{BlendParams, GeneratorParams, OutputParams};

    /// Validates `source` and checks that its `params` uniform has the size of `T`.
    fn check<T>(source: &str) {
        let module = naga::front::wgsl::parse_str(source)
            .unwrap_or_else(|e| panic!("{}", e.emit_to_string(source)));
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::empty(),
        )
        .validate(&module)
        .unwrap();
        let params = module
            .global_variables
            .iter()
            .find(|(_, var)| var.name.as_deref() == Some("params"))
            .expect("shader has a `params` uniform")
            .1;
        let wgsl_size = module.types[params.ty].inner.size(module.to_ctx()) as usize;
        assert_eq!(wgsl_size, size_of::<T>(), "uniform size mismatch");
    }

    #[test]
    fn shaders_are_valid_and_match_uniform_structs() {
        check::<GeneratorParams>(include_str!("shaders/generator.wgsl"));
        check::<BlendParams>(include_str!("shaders/blend.wgsl"));
        check::<OutputParams>(include_str!("shaders/output.wgsl"));
    }
}
