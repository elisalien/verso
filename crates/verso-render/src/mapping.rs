//! Renders a mapping [`Screen`]: its slices, warped, soft-edged and masked,
//! drawn from the composition output into a texture of the screen's size.

use bytemuck::{Pod, Zeroable};
use verso_core::mapping::{Mask, Screen, Slice, Warp};
use wgpu::util::DeviceExt;

use crate::OUTPUT_FORMAT;

const MASK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R8Unorm;
/// Masks are drawn at most this large on either side.
const MAX_MASK: u32 = 2048;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SliceParams {
    input: [f32; 4],
    blend: [f32; 4],
    gamma: f32,
    opacity: f32,
    invert: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 2],
    uv: [f32; 2],
}

/// GPU data of one slice, rebuilt when its warp or mask changes.
struct SliceCache {
    warp: Warp,
    mesh: wgpu::Buffer,
    vertices: u32,
    mask: Option<(Mask, wgpu::TextureView)>,
    params: wgpu::Buffer,
}

pub struct ScreenRenderer {
    slice_pipeline: wgpu::RenderPipeline,
    mask_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Mask of slices without one: everything visible.
    no_mask: wgpu::TextureView,
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
    slices: Vec<Option<SliceCache>>,
}

fn texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

impl ScreenRenderer {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, size: (u32, u32)) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("slice"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/mapping.wgsl").into()),
        });
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slice"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                texture_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                texture_entry(3, false),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slice"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let slice_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slice"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let mask_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slice mask"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let mask_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slice mask"),
            layout: Some(&mask_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_mask"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 8,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_mask"),
                compilation_options: Default::default(),
                targets: &[Some(MASK_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let no_mask = texture(device, "no mask", (1, 1), MASK_FORMAT);
        queue.write_texture(
            no_mask.as_image_copy(),
            &[255],
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(1),
                rows_per_image: Some(1),
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        let target = texture(device, "screen", size, OUTPUT_FORMAT);
        Self {
            slice_pipeline,
            mask_pipeline,
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("slice"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            no_mask: no_mask.create_view(&Default::default()),
            view: target.create_view(&Default::default()),
            texture: target,
            size: (size.0.max(1), size.1.max(1)),
            slices: Vec::new(),
        }
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    /// Updates the cached mesh and mask of slice `index`.
    fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        index: usize,
        slice: &Slice,
    ) {
        if self.slices.len() <= index {
            self.slices.resize_with(index + 1, || None);
        }
        let stale = self.slices[index]
            .as_ref()
            .is_none_or(|c| c.warp != slice.warp);
        if stale {
            let vertices: Vec<Vertex> = slice
                .warp
                .mesh()
                .into_iter()
                .map(|(position, uv)| Vertex { position, uv })
                .collect();
            let mesh = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("slice mesh"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let params = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slice params"),
                size: size_of::<SliceParams>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let mask = self.slices[index].take().and_then(|c| c.mask);
            self.slices[index] = Some(SliceCache {
                warp: slice.warp.clone(),
                mesh,
                vertices: vertices.len() as u32,
                mask,
                params,
            });
        }
        let size = (self.size.0.min(MAX_MASK), self.size.1.min(MAX_MASK));
        let Some(cache) = self.slices[index].as_mut() else {
            return;
        };
        let wanted = slice.mask.as_ref().filter(|m| m.points.len() >= 3);
        let current = cache.mask.as_ref().map(|(m, _)| m);
        if wanted != current {
            cache.mask = wanted.map(|mask| {
                let target = texture(device, "slice mask", size, MASK_FORMAT);
                let view = target.create_view(&Default::default());
                let triangles = mask.triangles();
                let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("slice mask"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                if !triangles.is_empty() {
                    let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("slice mask mesh"),
                        contents: bytemuck::cast_slice(&triangles),
                        usage: wgpu::BufferUsages::VERTEX,
                    });
                    rp.set_pipeline(&self.mask_pipeline);
                    rp.set_vertex_buffer(0, buffer.slice(..));
                    rp.draw(0..triangles.len() as u32, 0..1);
                }
                drop(rp);
                (mask.clone(), view)
            });
        }
    }

    /// Draws `screen` from `source`, the composition output.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        screen: &Screen,
        source: &wgpu::TextureView,
    ) {
        let size = (screen.size.0.max(1), screen.size.1.max(1));
        if size != self.size {
            let target = texture(device, "screen", size, OUTPUT_FORMAT);
            self.view = target.create_view(&Default::default());
            self.texture = target;
            self.size = size;
            // Masks are drawn at the screen size.
            for cache in self.slices.iter_mut().flatten() {
                cache.mask = None;
            }
        }
        for (index, slice) in screen.slices.iter().enumerate() {
            self.prepare(device, encoder, index, slice);
        }
        self.slices.truncate(screen.slices.len());

        let mut groups = Vec::new();
        for (slice, cache) in screen.slices.iter().zip(&self.slices) {
            let Some(cache) = cache.as_ref().filter(|_| slice.enabled) else {
                continue;
            };
            let b = slice.blend;
            let params = SliceParams {
                input: [slice.input.x, slice.input.y, slice.input.w, slice.input.h],
                blend: [b.left, b.right, b.top, b.bottom],
                gamma: b.gamma,
                opacity: slice.opacity,
                invert: slice.mask.as_ref().is_some_and(|m| m.invert) as u32,
                _pad: 0,
            };
            queue.write_buffer(&cache.params, 0, bytemuck::bytes_of(&params));
            let mask = cache.mask.as_ref().map(|(_, v)| v).unwrap_or(&self.no_mask);
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("slice"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: cache.params.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(mask),
                    },
                ],
            });
            groups.push((group, cache));
        }
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("screen"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            ..Default::default()
        });
        rp.set_pipeline(&self.slice_pipeline);
        for (group, cache) in &groups {
            rp.set_bind_group(0, group, &[]);
            rp.set_vertex_buffer(0, cache.mesh.slice(..));
            rp.draw(0..cache.vertices, 0..1);
        }
    }
}
