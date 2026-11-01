//! GPU passes of MilkDrop presets: warp mesh into a feedback buffer, blur
//! chain, sprites (shapes, waves, borders), then the composite pass into the
//! layer. CPU work comes from [`verso_milkdrop::Engine`].
//!
//! Each running clip keeps its own feedback buffer and engine, keyed by
//! (layer, column); they are dropped when the clip stops showing.

pub mod glsl;

use std::collections::{HashMap, HashSet};

use verso_milkdrop::eel::Rng;
use verso_milkdrop::preset::BUILTIN_SAMPLERS;
use verso_milkdrop::{
    AudioState, Blend, BlendPattern, COMP_HEIGHT, COMP_WIDTH, Engine, Family, FrameOutput,
    MESH_HEIGHT, MESH_WIDTH, MilkLibrary, Vertex, blur_passes,
};

use glsl::{MilkUniforms, Stage};

/// Feedback and blur buffers: 8 bits per channel, as in MilkDrop, so values
/// saturate and quantize the same way.
const FEEDBACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Butterchurn's blur buffer sizes relative to the feedback buffer,
/// (horizontal pass, vertical pass) per level.
const BLUR_RATIOS: [[f32; 2]; 3] = [[0.5, 0.25], [0.125, 0.125], [0.0625, 0.0625]];
const BLUR_WEIGHTS: [f32; 8] = [4.0, 3.8, 3.5, 2.9, 1.9, 1.2, 0.7, 0.3];

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct WarpVertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CompVertex {
    pos: [f32; 2],
    color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
struct BlurParams {
    texsize: [f32; 4],
    ws: [f32; 4],
    ds: [f32; 4],
    wdiv: f32,
    scale: f32,
    bias: f32,
    ed1: f32,
    ed2: f32,
    ed3: f32,
    _pad: [f32; 2],
}

struct Tex {
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Tex {
    fn new(device: &wgpu::Device, label: &str, size: (u32, u32)) -> Tex {
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
            format: FEEDBACK_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        Tex {
            view: texture.create_view(&Default::default()),
            size,
        }
    }
}

/// Butterchurn's blur buffer size for a fraction of the feedback size.
fn blur_size(size: (u32, u32), ratio: f32) -> (u32, u32) {
    let w = (size.0 as f32 * ratio).max(16.0);
    let h = (size.1 as f32 * ratio).max(16.0);
    (
        (((w + 3.0) / 16.0).floor() * 16.0) as u32,
        (((h + 3.0) / 4.0).floor() * 4.0) as u32,
    )
}

/// Noise texture data, ported from Butterchurn (cubic-interpolated lattice).
fn noise_2d(size: usize, zoom: usize, rng: &mut Rng) -> Vec<u8> {
    let n = size;
    let range = if zoom > 1 { 216.0 } else { 256.0 };
    let mut tex: Vec<u8> = (0..n * n * 4)
        .map(|_| ((rng.next_f64() * range + range * 0.5).floor() as i64).rem_euclid(256) as u8)
        .collect();
    if zoom > 1 {
        for y in (0..n).step_by(zoom) {
            for x in 0..n {
                if x % zoom != 0 {
                    let base_x = x / zoom * zoom + n;
                    let base_y = y * n;
                    let at = |off: usize, c: usize| {
                        tex[base_y * 4 + ((base_x + off - zoom) % n) * 4 + c]
                    };
                    let t = (x % zoom) as f32 / zoom as f32;
                    let v: [u8; 4] = std::array::from_fn(|c| {
                        cubic(at(0, c), at(zoom, c), at(2 * zoom, c), at(3 * zoom, c), t)
                    });
                    tex[(y * n + x) * 4..(y * n + x) * 4 + 4].copy_from_slice(&v);
                }
            }
        }
        for x in 0..n {
            for y in 0..n {
                if y % zoom != 0 {
                    let base_y = y / zoom * zoom + n;
                    let at =
                        |off: usize, c: usize| tex[((base_y + off - zoom) % n) * n * 4 + x * 4 + c];
                    let t = (y % zoom) as f32 / zoom as f32;
                    let v: [u8; 4] = std::array::from_fn(|c| {
                        cubic(at(0, c), at(zoom, c), at(2 * zoom, c), at(3 * zoom, c), t)
                    });
                    tex[(y * n + x) * 4..(y * n + x) * 4 + 4].copy_from_slice(&v);
                }
            }
        }
    }
    tex
}

fn noise_3d(size: usize, zoom: usize, rng: &mut Rng) -> Vec<u8> {
    let n = size;
    let range = if zoom > 1 { 216.0 } else { 256.0 };
    let mut tex: Vec<u8> = (0..n * n * n * 4)
        .map(|_| ((rng.next_f64() * range + range * 0.5).floor() as i64).rem_euclid(256) as u8)
        .collect();
    if zoom > 1 {
        let slice = n * n;
        for z in (0..n).step_by(zoom) {
            for y in (0..n).step_by(zoom) {
                for x in 0..n {
                    if x % zoom != 0 {
                        let base_x = x / zoom * zoom + n;
                        let base = z * slice + y * n;
                        let at = |off: usize, c: usize| {
                            tex[base * 4 + ((base_x + off - zoom) % n) * 4 + c]
                        };
                        let t = (x % zoom) as f32 / zoom as f32;
                        let v: [u8; 4] = std::array::from_fn(|c| {
                            cubic(at(0, c), at(zoom, c), at(2 * zoom, c), at(3 * zoom, c), t)
                        });
                        let i = (base + x) * 4;
                        tex[i..i + 4].copy_from_slice(&v);
                    }
                }
            }
        }
        for z in (0..n).step_by(zoom) {
            for x in 0..n {
                for y in 0..n {
                    if y % zoom != 0 {
                        let base_y = y / zoom * zoom + n;
                        let at = |off: usize, c: usize| {
                            tex[((base_y + off - zoom) % n) * n * 4 + x * 4 + z * slice * 4 + c]
                        };
                        let t = (y % zoom) as f32 / zoom as f32;
                        let v: [u8; 4] = std::array::from_fn(|c| {
                            cubic(at(0, c), at(zoom, c), at(2 * zoom, c), at(3 * zoom, c), t)
                        });
                        // Butterchurn writes this slice without the z offset.
                        let i = y * n * 4 + x * 4 + z * slice * 4;
                        tex[i..i + 4].copy_from_slice(&v);
                    }
                }
            }
        }
        for x in 0..n {
            for y in 0..n {
                for z in 0..n {
                    if z % zoom != 0 {
                        let base_z = z / zoom * zoom + n;
                        let at = |off: usize, c: usize| {
                            tex[((base_z + off - zoom) % n) * slice * 4 + x * 4 + y * n * 4 + c]
                        };
                        // Butterchurn interpolates with the y fraction here.
                        let t = (y % zoom) as f32 / zoom as f32;
                        let v: [u8; 4] = std::array::from_fn(|c| {
                            cubic(at(0, c), at(zoom, c), at(2 * zoom, c), at(3 * zoom, c), t)
                        });
                        let i = z * slice * 4 + x * 4 + y * n * 4;
                        tex[i..i + 4].copy_from_slice(&v);
                    }
                }
            }
        }
    }
    tex
}

fn cubic(y0: u8, y1: u8, y2: u8, y3: u8, t: f32) -> u8 {
    let (y0, y1, y2, y3) = (
        y0 as f32 / 255.0,
        y1 as f32 / 255.0,
        y2 as f32 / 255.0,
        y3 as f32 / 255.0,
    );
    let t2 = t * t;
    let t3 = t * t2;
    let a0 = y3 - y2 - y0 + y1;
    let a1 = y0 - y1 - a0;
    let a2 = y2 - y0;
    let a3 = y1;
    ((a0 * t3 + a1 * t2 + a2 * t + a3).clamp(0.0, 1.0) * 255.0) as u8
}

fn upload_noise(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    size: u32,
    depth: Option<u32>,
    data: &[u8],
) -> wgpu::TextureView {
    let extent = wgpu::Extent3d {
        width: size,
        height: size,
        depth_or_array_layers: depth.unwrap_or(1),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("milkdrop noise"),
        size: extent,
        mip_level_count: 1,
        sample_count: 1,
        dimension: if depth.is_some() {
            wgpu::TextureDimension::D3
        } else {
            wgpu::TextureDimension::D2
        },
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        texture.as_image_copy(),
        data,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(size * 4),
            rows_per_image: Some(size),
        },
        extent,
    );
    texture.create_view(&Default::default())
}

fn layout_texture(
    binding: u32,
    dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

fn layout_sampler(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        count: None,
    }
}

fn layout_uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

fn sampler(
    device: &wgpu::Device,
    filter: wgpu::FilterMode,
    address: wgpu::AddressMode,
) -> wgpu::Sampler {
    device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("milkdrop"),
        address_mode_u: address,
        address_mode_v: address,
        address_mode_w: address,
        mag_filter: filter,
        min_filter: filter,
        ..Default::default()
    })
}

fn blend_state(blend: Blend) -> wgpu::BlendState {
    let dst = match blend {
        Blend::Alpha => wgpu::BlendFactor::OneMinusSrcAlpha,
        Blend::Additive => wgpu::BlendFactor::One,
    };
    let component = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: dst,
        operation: wgpu::BlendOperation::Add,
    };
    wgpu::BlendState {
        color: component,
        alpha: component,
    }
}

#[allow(clippy::too_many_arguments)]
fn mesh_pipeline(
    device: &wgpu::Device,
    label: &str,
    layout: &wgpu::BindGroupLayout,
    vertex: (&wgpu::ShaderModule, &str),
    fragment: (&wgpu::ShaderModule, &str),
    buffers: &[Option<wgpu::VertexBufferLayout<'_>>],
    format: wgpu::TextureFormat,
    blend: Option<wgpu::BlendState>,
) -> wgpu::RenderPipeline {
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some(label),
        bind_group_layouts: &[Some(layout)],
        immediate_size: 0,
    });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(label),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: vertex.0,
            entry_point: Some(vertex.1),
            compilation_options: Default::default(),
            buffers,
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: fragment.0,
            entry_point: Some(fragment.1),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}

const WARP_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4];
const COMP_ATTRIBUTES: [wgpu::VertexAttribute; 2] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4];
const SPRITE_ATTRIBUTES: [wgpu::VertexAttribute; 3] =
    wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4, 2 => Float32x2];

fn warp_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout {
        array_stride: size_of::<WarpVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &WARP_ATTRIBUTES,
    })]
}

fn comp_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 1] {
    [Some(wgpu::VertexBufferLayout {
        array_stride: size_of::<CompVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &COMP_ATTRIBUTES,
    })]
}

/// Triangle indices of a `w` × `h` cell grid, row by row.
fn grid_indices(w: usize, h: usize) -> Vec<u32> {
    let mut indices = Vec::with_capacity(w * h * 6);
    for iy in 0..h {
        for ix in 0..w {
            let a = (ix + (w + 1) * iy) as u32;
            let b = (ix + (w + 1) * (iy + 1)) as u32;
            let c = (ix + 1 + (w + 1) * (iy + 1)) as u32;
            let d = (ix + 1 + (w + 1) * iy) as u32;
            indices.extend_from_slice(&[a, b, d, b, c, d]);
        }
    }
    indices
}

/// Grid vertex positions in OpenGL clip space, row 0 at the top.
fn grid_position(ix: usize, iy: usize, w: usize, h: usize) -> [f32; 2] {
    [
        ix as f32 / w as f32 * 2.0 - 1.0,
        -(iy as f32 / h as f32 * 2.0 - 1.0),
    ]
}

fn index_buffer(device: &wgpu::Device, queue: &wgpu::Queue, indices: &[u32]) -> wgpu::Buffer {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("milkdrop indices"),
        size: (indices.len() * 4) as u64,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, bytemuck::cast_slice(indices));
    buffer
}

fn buffer(
    device: &wgpu::Device,
    label: &str,
    size: u64,
    usage: wgpu::BufferUsages,
) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Pipelines and textures shared by every instance.
struct Shared {
    shader_layout: wgpu::BindGroupLayout,
    blur_layout: wgpu::BindGroupLayout,
    sprite_layout: wgpu::BindGroupLayout,
    fixed: wgpu::ShaderModule,
    blur_h: wgpu::RenderPipeline,
    blur_v: wgpu::RenderPipeline,
    /// By (blend, textured).
    sprites: HashMap<(Blend, bool), wgpu::RenderPipeline>,
    input_convert: wgpu::RenderPipeline,
    input_feed: wgpu::RenderPipeline,
    noise: [wgpu::TextureView; 6],
    /// linear repeat, linear clamp, nearest repeat, nearest clamp
    samplers: [wgpu::Sampler; 4],
    warp_indices: wgpu::Buffer,
    comp_indices: wgpu::Buffer,
    warp_index_count: u32,
    comp_index_count: u32,
}

impl Shared {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Shared {
        let d2 = wgpu::TextureViewDimension::D2;
        let d3 = wgpu::TextureViewDimension::D3;
        let mut entries = vec![layout_uniform(0)];
        for binding in 1..=10 {
            entries.push(layout_texture(binding, if binding >= 9 { d3 } else { d2 }));
        }
        for binding in 11..=15 {
            entries.push(layout_sampler(binding));
        }
        entries.push(layout_texture(16, d2));
        let shader_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("milkdrop shader"),
            entries: &entries,
        });
        let blur_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("milkdrop blur"),
            entries: &[layout_uniform(0), layout_texture(1, d2), layout_sampler(2)],
        });
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("milkdrop sprite"),
            entries: &[layout_texture(0, d2), layout_sampler(1)],
        });
        let fixed = crate::shader(device, "milkdrop fixed", include_str!("milk.wgsl"));
        let blur = crate::shader(device, "milkdrop blur", include_str!("blur.wgsl"));
        let blur_pipeline = |entry| {
            mesh_pipeline(
                device,
                "milkdrop blur",
                &blur_layout,
                (&blur, "vs_main"),
                (&blur, entry),
                &[],
                FEEDBACK_FORMAT,
                None,
            )
        };
        let (blur_h, blur_v) = (blur_pipeline("horizontal"), blur_pipeline("vertical"));
        let sprite_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &SPRITE_ATTRIBUTES,
        })];
        let mut sprites = HashMap::new();
        for blend in [Blend::Alpha, Blend::Additive] {
            for textured in [false, true] {
                let pipeline = mesh_pipeline(
                    device,
                    "milkdrop sprite",
                    &sprite_layout,
                    (&fixed, "sprite_vs"),
                    (
                        &fixed,
                        if textured {
                            "sprite_textured"
                        } else {
                            "sprite_flat"
                        },
                    ),
                    &sprite_buffers,
                    FEEDBACK_FORMAT,
                    Some(blend_state(blend)),
                );
                sprites.insert((blend, textured), pipeline);
            }
        }
        let input = crate::shader(device, "milkdrop input", include_str!("input.wgsl"));
        let input_convert = mesh_pipeline(
            device,
            "milkdrop input",
            &sprite_layout,
            (&input, "vs_main"),
            (&input, "convert"),
            &[],
            FEEDBACK_FORMAT,
            None,
        );
        let constant = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let input_feed = mesh_pipeline(
            device,
            "milkdrop input feed",
            &sprite_layout,
            (&input, "vs_main"),
            (&input, "feed"),
            &[],
            FEEDBACK_FORMAT,
            Some(wgpu::BlendState {
                color: constant,
                alpha: constant,
            }),
        );
        let mut rng = Rng::new(0x5eed_d00d);
        let noise = [
            upload_noise(device, queue, 256, None, &noise_2d(256, 1, &mut rng)),
            upload_noise(device, queue, 32, None, &noise_2d(32, 1, &mut rng)),
            upload_noise(device, queue, 256, None, &noise_2d(256, 4, &mut rng)),
            upload_noise(device, queue, 256, None, &noise_2d(256, 8, &mut rng)),
            upload_noise(device, queue, 32, Some(32), &noise_3d(32, 1, &mut rng)),
            upload_noise(device, queue, 32, Some(32), &noise_3d(32, 4, &mut rng)),
        ];
        use wgpu::{AddressMode::*, FilterMode::*};
        let samplers = [
            sampler(device, Linear, Repeat),
            sampler(device, Linear, ClampToEdge),
            sampler(device, Nearest, Repeat),
            sampler(device, Nearest, ClampToEdge),
        ];
        let warp = grid_indices(MESH_WIDTH, MESH_HEIGHT);
        let comp = grid_indices(COMP_WIDTH, COMP_HEIGHT);
        Shared {
            warp_indices: index_buffer(device, queue, &warp),
            comp_indices: index_buffer(device, queue, &comp),
            warp_index_count: warp.len() as u32,
            comp_index_count: comp.len() as u32,
            shader_layout,
            blur_layout,
            sprite_layout,
            fixed,
            blur_h,
            blur_v,
            sprites,
            input_convert,
            input_feed,
            noise,
            samplers,
        }
    }

    /// Pipeline of a preset warp or comp shader; `Err` when it does not
    /// translate.
    fn preset_pipeline(
        &self,
        device: &wgpu::Device,
        stage: Stage,
        text: &str,
        format: wgpu::TextureFormat,
        blend: Option<wgpu::BlendState>,
    ) -> Result<wgpu::RenderPipeline, String> {
        let wgsl = glsl::translate(stage, text)?;
        let module = crate::shader(device, "milkdrop preset", &wgsl);
        let (vertex, buffers) = match stage {
            Stage::Warp => ("warp_vs", warp_buffers()),
            Stage::Comp => ("comp_vs", comp_buffers()),
        };
        Ok(mesh_pipeline(
            device,
            "milkdrop preset",
            &self.shader_layout,
            (&self.fixed, vertex),
            (&module, "main"),
            &buffers,
            format,
            blend,
        ))
    }

    /// Bind group of a warp or comp pass reading `main`; image textures of
    /// the preset read `input`, else noise.
    fn shader_group(
        &self,
        device: &wgpu::Device,
        uniforms: &wgpu::Buffer,
        main: &wgpu::TextureView,
        blur: [&wgpu::TextureView; 3],
        wrap: bool,
        input: Option<&wgpu::TextureView>,
    ) -> wgpu::BindGroup {
        let s = &self.samplers;
        let main_sampler = if wrap { &s[0] } else { &s[1] };
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.as_entire_binding(),
        }];
        let textures = [main, blur[0], blur[1], blur[2]]
            .into_iter()
            .chain(self.noise.iter());
        for (i, view) in textures.enumerate() {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32 + 1,
                resource: wgpu::BindingResource::TextureView(view),
            });
        }
        for (i, sampler) in [main_sampler, &s[0], &s[1], &s[2], &s[3]]
            .into_iter()
            .enumerate()
        {
            entries.push(wgpu::BindGroupEntry {
                binding: i as u32 + 11,
                resource: wgpu::BindingResource::Sampler(sampler),
            });
        }
        entries.push(wgpu::BindGroupEntry {
            binding: 16,
            resource: wgpu::BindingResource::TextureView(input.unwrap_or(&self.noise[0])),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("milkdrop shader"),
            layout: &self.shader_layout,
            entries: &entries,
        })
    }
}

/// Warp and comp pipelines of one preset variant: opaque, and fading in over
/// another preset during a blend.
struct Compiled {
    warp: wgpu::RenderPipeline,
    warp_fade: wgpu::RenderPipeline,
    comp: wgpu::RenderPipeline,
    comp_fade: wgpu::RenderPipeline,
    blur_passes: u32,
}

/// One preset running on an instance's feedback buffer.
struct Run {
    family: String,
    variant: usize,
    engine: Engine,
    /// Time the preset started, for its own clock.
    start: f32,
    uniforms: wgpu::Buffer,
    comp_vertices: wgpu::Buffer,
    sprite_vertices: wgpu::Buffer,
}

impl Run {
    fn new(
        device: &wgpu::Device,
        family: &str,
        variant: usize,
        preset: &verso_milkdrop::Preset,
        size: (u32, u32),
        time: f32,
    ) -> Result<Run, String> {
        let seed = (time.to_bits() as u64) << 16 | variant as u64;
        let vertex = wgpu::BufferUsages::VERTEX;
        Ok(Run {
            family: family.to_owned(),
            variant,
            engine: Engine::new(preset, size.0, size.1, seed)?,
            start: time,
            uniforms: buffer(
                device,
                "milkdrop uniforms",
                size_of::<MilkUniforms>() as u64,
                wgpu::BufferUsages::UNIFORM,
            ),
            comp_vertices: buffer(
                device,
                "milkdrop comp",
                ((COMP_WIDTH + 1) * (COMP_HEIGHT + 1) * size_of::<CompVertex>()) as u64,
                vertex,
            ),
            sprite_vertices: buffer(
                device,
                "milkdrop sprites",
                4096 * size_of::<Vertex>() as u64,
                vertex,
            ),
        })
    }

    fn key(&self) -> (String, usize) {
        (self.family.clone(), self.variant)
    }

    /// Uploads the composite colors, with `weights` as alpha when fading in.
    fn write_comp(&self, queue: &wgpu::Queue, out: &FrameOutput, weights: Option<&[f32]>) {
        let mut comp = Vec::with_capacity(out.comp_colors.len());
        for iy in 0..=COMP_HEIGHT {
            for ix in 0..=COMP_WIDTH {
                let i = iy * (COMP_WIDTH + 1) + ix;
                let mut color = out.comp_colors[i];
                color[3] = weights.map_or(1.0, |w| w[i]);
                comp.push(CompVertex {
                    pos: grid_position(ix, iy, COMP_WIDTH, COMP_HEIGHT),
                    color,
                });
            }
        }
        queue.write_buffer(&self.comp_vertices, 0, bytemuck::cast_slice(&comp));
    }

    /// Uploads the sprites with their alpha scaled by `alpha`.
    fn write_sprites(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        out: &FrameOutput,
        alpha: f32,
    ) {
        let vertices: Vec<Vertex> = out
            .vertices
            .iter()
            .map(|v| Vertex {
                color: [v.color[0], v.color[1], v.color[2], v.color[3] * alpha],
                ..*v
            })
            .collect();
        let bytes = (vertices.len() * size_of::<Vertex>()) as u64;
        if bytes > self.sprite_vertices.size() {
            self.sprite_vertices = buffer(
                device,
                "milkdrop sprites",
                bytes.next_power_of_two(),
                wgpu::BufferUsages::VERTEX,
            );
        }
        queue.write_buffer(&self.sprite_vertices, 0, bytemuck::cast_slice(&vertices));
    }
}

/// A preset fading out while the next one takes over.
struct Fading {
    run: Run,
    start: f32,
    duration: f32,
    pattern: BlendPattern,
}

/// One clip's feedback buffers and the presets drawing into them.
struct Instance {
    size: (u32, u32),
    run: Run,
    fading: Option<Fading>,
    audio: AudioState,
    /// Feedback buffers; `current` holds the last frame.
    feedback: [Tex; 2],
    current: usize,
    /// (horizontal, vertical) per blur level.
    blur: Vec<(Tex, Tex)>,
    /// The input layer converted to MilkDrop's display colors.
    input: Option<Tex>,
    blur_uniforms: Vec<wgpu::Buffer>,
    warp_vertices: wgpu::Buffer,
    last: f32,
    frame: u64,
    fps: f32,
    rng: Rng,
}

impl Instance {
    fn new(
        device: &wgpu::Device,
        run: Run,
        size: (u32, u32),
        time: f32,
        sample_rate: f32,
    ) -> Instance {
        let blur = BLUR_RATIOS
            .iter()
            .map(|ratios| {
                (
                    Tex::new(device, "milkdrop blur", blur_size(size, ratios[0])),
                    Tex::new(device, "milkdrop blur", blur_size(size, ratios[1])),
                )
            })
            .collect();
        Instance {
            size,
            fading: None,
            audio: AudioState::new(sample_rate),
            feedback: [
                Tex::new(device, "milkdrop feedback", size),
                Tex::new(device, "milkdrop feedback", size),
            ],
            current: 0,
            blur,
            input: None,
            blur_uniforms: (0..6)
                .map(|_| {
                    buffer(
                        device,
                        "milkdrop blur",
                        size_of::<BlurParams>() as u64,
                        wgpu::BufferUsages::UNIFORM,
                    )
                })
                .collect(),
            warp_vertices: buffer(
                device,
                "milkdrop warp",
                ((MESH_WIDTH + 1) * (MESH_HEIGHT + 1) * size_of::<WarpVertex>()) as u64,
                wgpu::BufferUsages::VERTEX,
            ),
            last: time,
            frame: 0,
            fps: 60.0,
            rng: Rng::new(time.to_bits() as u64 | 1),
            run,
        }
    }
}

/// Uniform values for one frame.
fn uniforms(out: &FrameOutput, size: (u32, u32), input_size: (u32, u32)) -> MilkUniforms {
    let s = &out.shader;
    let (w, h) = (size.0 as f32, size.1 as f32);
    let aspect = [
        if h > w { w / h } else { 1.0 },
        if w > h { h / w } else { 1.0 },
    ];
    let roam = |speeds: [f32; 4], f: fn(f32) -> f32| speeds.map(|k| 0.5 + 0.5 * f(s.time * k));
    let bool_f = |b: bool| if b { 1.0 } else { 0.0 };
    let (iw, ih) = (input_size.0 as f32, input_size.1 as f32);
    MilkUniforms {
        q: std::array::from_fn(|i| std::array::from_fn(|k| s.q[i * 4 + k])),
        texsize: [w, h, 1.0 / w, 1.0 / h],
        aspect: [aspect[0], aspect[1], 1.0 / aspect[0], 1.0 / aspect[1]],
        rand_frame: s.rand_frame,
        rand_preset: s.rand_preset,
        slow_roam_cos: roam([0.005, 0.008, 0.013, 0.022], f32::cos),
        roam_cos: roam([0.3, 1.3, 5.0, 20.0], f32::cos),
        slow_roam_sin: roam([0.005, 0.008, 0.013, 0.022], f32::sin),
        roam_sin: roam([0.3, 1.3, 5.0, 20.0], f32::sin),
        audio: [s.bass, s.mid, s.treb, (s.bass + s.mid + s.treb) / 3.0],
        audio_att: [
            s.bass_att,
            s.mid_att,
            s.treb_att,
            (s.bass_att + s.mid_att + s.treb_att) / 3.0,
        ],
        timing: [s.time, s.frame, s.fps, s.decay],
        blur_min: [s.blur_min[0], s.blur_min[1], s.blur_min[2], s.fshader],
        blur_max: [s.blur_max[0], s.blur_max[1], s.blur_max[2], s.gamma_adj],
        echo: [s.echo_zoom, s.echo_alpha, s.echo_orient, 0.0],
        flags: [
            bool_f(s.invert),
            bool_f(s.brighten),
            bool_f(s.darken),
            bool_f(s.solarize),
        ],
        input_size: [iw, ih, 1.0 / iw, 1.0 / ih],
    }
}

/// Scale and bias of the horizontal blur at `level` (Butterchurn).
fn blur_scale_bias(level: usize, min: [f32; 3], max: [f32; 3]) -> (f32, f32) {
    let safe = |a: f32| if a.abs() < 1e-6 { 1e-6 } else { a };
    let scale0 = 1.0 / safe(max[0] - min[0]);
    let bias0 = -min[0] * scale0;
    match level {
        0 => (scale0, bias0),
        _ => {
            let (lo, hi) = (min[level - 1], max[level - 1]);
            let t_min = (min[level] - lo) / safe(hi - lo);
            let t_max = (max[level] - lo) / safe(hi - lo);
            let scale = 1.0 / safe(t_max - t_min);
            (scale, -t_min * scale)
        }
    }
}

/// How a clip plays its preset, besides which preset it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PlayOptions<'a> {
    /// Seconds of MilkDrop blend when the preset changes; 0 cuts.
    pub blend: f32,
    /// Image of another layer, in the work format, fed to the preset.
    pub input: Option<&'a wgpu::TextureView>,
    /// How much of the input is mixed into the feedback each frame.
    pub input_mix: f32,
}

/// Runs MilkDrop presets as clip sources.
pub struct MilkRenderer {
    library: MilkLibrary,
    shared: Option<Shared>,
    compiled: HashMap<(String, usize), Compiled>,
    instances: HashMap<(usize, usize), Instance>,
    drawn: HashSet<(usize, usize)>,
    /// Last error per (family, variant), for the UI.
    errors: HashMap<(String, usize), String>,
    samples: Vec<f32>,
    sample_rate: f32,
}

impl Default for MilkRenderer {
    fn default() -> Self {
        MilkRenderer {
            library: MilkLibrary::default(),
            shared: None,
            compiled: HashMap::new(),
            instances: HashMap::new(),
            drawn: HashSet::new(),
            errors: HashMap::new(),
            samples: Vec::new(),
            sample_rate: 44100.0,
        }
    }
}

impl MilkRenderer {
    pub fn library(&self) -> &MilkLibrary {
        &self.library
    }

    /// Changing the library drops compiled pipelines and running presets.
    pub fn library_mut(&mut self) -> &mut MilkLibrary {
        self.compiled.clear();
        self.instances.clear();
        self.errors.clear();
        &mut self.library
    }

    /// Latest mono audio samples (about 1024) in -1..1.
    pub fn set_audio(&mut self, samples: Vec<f32>, sample_rate: f32) {
        self.samples = samples;
        self.sample_rate = sample_rate;
    }

    /// Why a variant cannot run, if it failed.
    pub fn error(&self, family: &str, variant: usize) -> Option<&str> {
        self.errors
            .get(&(family.to_owned(), variant))
            .map(String::as_str)
    }

    pub(crate) fn begin_frame(&mut self) {
        self.drawn.clear();
    }

    /// Drops the presets that were not drawn this frame.
    pub(crate) fn end_frame(&mut self) {
        let drawn = &self.drawn;
        self.instances.retain(|key, _| drawn.contains(key));
    }

    /// Drops every running preset, e.g. after a size change.
    pub(crate) fn reset(&mut self) {
        self.instances.clear();
    }

    /// Compiles a variant's pipelines; `false` when it cannot run.
    fn compile(
        &mut self,
        device: &wgpu::Device,
        family_id: &str,
        variant: usize,
        family: &Family,
        format: wgpu::TextureFormat,
    ) -> bool {
        let key = (family_id.to_owned(), variant);
        if self.compiled.contains_key(&key) {
            return true;
        }
        if self.errors.contains_key(&key) {
            return false;
        }
        let shared = self.shared.as_ref().unwrap();
        let preset = &family.variants[variant].preset;
        let fade = Some(fade_state());
        let build = |stage, text: &str, format| -> Result<_, String> {
            Ok((
                shared.preset_pipeline(device, stage, text, format, None)?,
                shared.preset_pipeline(device, stage, text, format, fade)?,
            ))
        };
        match (
            build(Stage::Warp, &preset.warp, FEEDBACK_FORMAT),
            build(Stage::Comp, &preset.comp, format),
        ) {
            (Ok((warp, warp_fade)), Ok((comp, comp_fade))) => {
                self.compiled.insert(
                    key,
                    Compiled {
                        warp,
                        warp_fade,
                        comp,
                        comp_fade,
                        blur_passes: blur_passes(preset),
                    },
                );
                true
            }
            (Err(e), _) | (_, Err(e)) => {
                self.errors.insert(key, e);
                false
            }
        }
    }

    /// Advances and draws the preset playing at `key` into `output` (a
    /// `format` texture of `size`). A new variant on the same clip blends
    /// from the previous one over the same feedback buffer. Returns `false`
    /// when nothing can run.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        key: (usize, usize),
        family_id: &str,
        variant: usize,
        options: PlayOptions<'_>,
        output: &wgpu::TextureView,
        format: wgpu::TextureFormat,
        size: (u32, u32),
        time: f32,
    ) -> bool {
        if self.shared.is_none() {
            self.shared = Some(Shared::new(device, queue));
        }
        let wanted = self.library.get(family_id).cloned().and_then(|family| {
            let variant = variant.min(family.variants.len() - 1);
            self.compile(device, family_id, variant, &family, format)
                .then_some((family, variant))
        });
        let fresh = self
            .instances
            .get(&key)
            .is_none_or(|i| i.size != size || time < i.last);
        if fresh {
            self.instances.remove(&key);
        }
        if let Some((family, variant)) = &wanted {
            let preset = &family.variants[*variant].preset;
            let changed = self
                .instances
                .get(&key)
                .is_none_or(|i| i.run.key() != (family_id.to_owned(), *variant));
            if changed {
                match Run::new(device, family_id, *variant, preset, size, time) {
                    Ok(run) => match self.instances.get_mut(&key) {
                        Some(instance) => {
                            let old = std::mem::replace(&mut instance.run, run);
                            instance.fading = (options.blend > 0.0).then(|| Fading {
                                run: old,
                                start: time,
                                duration: options.blend,
                                pattern: BlendPattern::new(size.0, size.1, &mut instance.rng),
                            });
                        }
                        None => {
                            let instance = Instance::new(device, run, size, time, self.sample_rate);
                            self.instances.insert(key, instance);
                        }
                    },
                    Err(e) => {
                        self.errors.insert((family_id.to_owned(), *variant), e);
                    }
                }
            }
        }
        // A preset that cannot run keeps the previous one playing.
        let Some(instance) = self.instances.get_mut(&key) else {
            return false;
        };
        self.drawn.insert(key);
        let shared = self.shared.as_ref().unwrap();
        render_instance(
            device,
            queue,
            encoder,
            shared,
            &self.compiled,
            instance,
            &self.samples,
            options,
            output,
            time,
        );
        true
    }
}

/// Blend that fades a pass in over the destination color, keeping the
/// destination alpha.
fn fade_state() -> wgpu::BlendState {
    wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::SrcAlpha,
            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    }
}

fn texture_group<'a>(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("milkdrop texture"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(sampler),
            },
        ],
    })
}

#[allow(clippy::too_many_arguments)]
fn render_instance(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    encoder: &mut wgpu::CommandEncoder,
    shared: &Shared,
    compiled: &HashMap<(String, usize), Compiled>,
    instance: &mut Instance,
    samples: &[f32],
    options: PlayOptions<'_>,
    output: &wgpu::TextureView,
    time: f32,
) {
    let size = instance.size;

    // Timing and audio, shared by both presets during a blend.
    let dt = time - instance.last;
    if dt > 0.0 && dt < 1.0 {
        instance.fps = instance.fps * 0.93 + (1.0 / dt) * 0.07;
    }
    instance.last = time;
    instance.frame += 1;
    instance.audio.update(samples, instance.fps, instance.frame);
    let (fps, frame) = (instance.fps as f64, instance.frame);

    let progress = instance
        .fading
        .as_ref()
        .map(|f| ((time - f.start) / f.duration).clamp(0.0, 1.0));
    if progress.is_some_and(|p| p >= 1.0) {
        instance.fading = None;
    }
    let progress = progress.filter(|p| *p < 1.0);
    let run = &mut instance.run;
    let out = run
        .engine
        .frame(&instance.audio, (time - run.start) as f64, fps, frame);
    let faded = instance.fading.as_mut().map(|f| {
        let out = f
            .run
            .engine
            .frame(&instance.audio, (time - f.run.start) as f64, fps, frame);
        (f, out)
    });
    let Some(current) = compiled.get(&run.key()) else {
        return;
    };

    // Inputs of the passes.
    let input_size = instance.input.as_ref().map_or((256, 256), |t| t.size);
    queue.write_buffer(
        &run.uniforms,
        0,
        bytemuck::bytes_of(&uniforms(&out, size, input_size)),
    );
    let mut warp = Vec::with_capacity(out.warp_uvs.len());
    for iy in 0..=MESH_HEIGHT {
        for ix in 0..=MESH_WIDTH {
            let i = iy * (MESH_WIDTH + 1) + ix;
            let (mut uv, mut alpha) = (out.warp_uvs[i], 1.0);
            if let (Some((fading, old)), Some(p)) = (&faded, progress) {
                let w = fading.pattern.weight(i, p);
                let o = old.warp_uvs[i];
                uv = [uv[0] * w + o[0] * (1.0 - w), uv[1] * w + o[1] * (1.0 - w)];
                alpha = w;
            }
            warp.push(WarpVertex {
                pos: grid_position(ix, iy, MESH_WIDTH, MESH_HEIGHT),
                uv,
                color: [1.0, 1.0, 1.0, alpha],
            });
        }
    }
    queue.write_buffer(&instance.warp_vertices, 0, bytemuck::cast_slice(&warp));
    let weights = progress
        .zip(faded.as_ref())
        .map(|(p, (f, _))| f.pattern.comp_weights(p));
    run.write_comp(queue, &out, weights.as_deref());
    run.write_sprites(device, queue, &out, progress.unwrap_or(1.0));
    let mut old = None;
    if let (Some((fading, old_out)), Some(p)) = (faded, progress) {
        let old_run = &mut fading.run;
        if let Some(old_compiled) = compiled.get(&old_run.key()) {
            queue.write_buffer(
                &old_run.uniforms,
                0,
                bytemuck::bytes_of(&uniforms(&old_out, size, input_size)),
            );
            old_run.write_comp(queue, &old_out, None);
            old_run.write_sprites(device, queue, &old_out, 1.0 - p);
            old = Some((&*old_run, old_compiled, old_out));
        }
    }

    let prev = instance.current;
    let target = 1 - prev;
    let blur_views = [
        &instance.blur[0].1.view,
        &instance.blur[1].1.view,
        &instance.blur[2].1.view,
    ];
    let wrap = out.shader.wrap;

    // The input layer, in MilkDrop's display colors.
    if let Some(input) = options.input {
        if instance.input.as_ref().is_none_or(|t| t.size != size) {
            instance.input = Some(Tex::new(device, "milkdrop input", size));
        }
        let group = texture_group(device, &shared.sprite_layout, input, &shared.samplers[1]);
        let mut rp = crate::pass(encoder, &instance.input.as_ref().unwrap().view, true);
        rp.set_pipeline(&shared.input_convert);
        rp.set_bind_group(0, &group, &[]);
        rp.draw(0..3, 0..1);
    } else {
        instance.input = None;
    }
    let input_view = instance.input.as_ref().map(|t| &t.view);

    // Warp: last frame through the mesh into the target buffer. During a
    // blend the old preset draws first and the new one fades in over it.
    {
        let mut rp = crate::pass(encoder, &instance.feedback[target].view, true);
        rp.set_vertex_buffer(0, instance.warp_vertices.slice(..));
        rp.set_index_buffer(shared.warp_indices.slice(..), wgpu::IndexFormat::Uint32);
        if let Some((old_run, old_compiled, old_out)) = &old {
            let group = shared.shader_group(
                device,
                &old_run.uniforms,
                &instance.feedback[prev].view,
                blur_views,
                old_out.shader.wrap,
                input_view,
            );
            rp.set_pipeline(&old_compiled.warp);
            rp.set_bind_group(0, &group, &[]);
            rp.draw_indexed(0..shared.warp_index_count, 0, 0..1);
        }
        let group = shared.shader_group(
            device,
            &run.uniforms,
            &instance.feedback[prev].view,
            blur_views,
            wrap,
            input_view,
        );
        rp.set_pipeline(if old.is_some() {
            &current.warp_fade
        } else {
            &current.warp
        });
        rp.set_bind_group(0, &group, &[]);
        rp.draw_indexed(0..shared.warp_index_count, 0, 0..1);
    }

    // Input mixed into the feedback, so presets warp it like their own image.
    if let Some(view) = input_view
        && options.input_mix > 0.0
    {
        let group = texture_group(device, &shared.sprite_layout, view, &shared.samplers[1]);
        let mut rp = crate::pass(encoder, &instance.feedback[target].view, false);
        rp.set_pipeline(&shared.input_feed);
        rp.set_bind_group(0, &group, &[]);
        let mix = options.input_mix.clamp(0.0, 1.0) as f64;
        rp.set_blend_constant(wgpu::Color {
            r: mix,
            g: mix,
            b: mix,
            a: mix,
        });
        rp.draw(0..3, 0..1);
    }

    // Blur chain from the warped image, before sprites are drawn.
    let ws = BLUR_WEIGHTS;
    let w_h = [ws[0] + ws[1], ws[2] + ws[3], ws[4] + ws[5], ws[6] + ws[7]];
    let d_h = [
        2.0 * ws[1] / w_h[0],
        2.0 + 2.0 * ws[3] / w_h[1],
        4.0 + 2.0 * ws[5] / w_h[2],
        6.0 + 2.0 * ws[7] / w_h[3],
    ];
    let w1v = ws[0] + ws[1] + ws[2] + ws[3];
    let w2v = ws[4] + ws[5] + ws[6] + ws[7];
    let wds = [
        w1v,
        w2v,
        2.0 * (ws[2] + ws[3]) / w1v,
        2.0 + 2.0 * (ws[6] + ws[7]) / w2v,
    ];
    let blur_passes = old
        .as_ref()
        .map_or(0, |(_, c, _)| c.blur_passes)
        .max(current.blur_passes);
    for level in 0..blur_passes as usize {
        let (src_view, src_size) = if level == 0 {
            (&instance.feedback[target].view, size)
        } else {
            let t = &instance.blur[level - 1].1;
            (&t.view, t.size)
        };
        let (scale, bias) = blur_scale_bias(level, out.shader.blur_min, out.shader.blur_max);
        let b1ed = if level == 0 { out.shader.b1ed } else { 0.0 };
        let (h, v) = &instance.blur[level];
        let texsize = |s: (u32, u32)| [s.0 as f32, s.1 as f32, 1.0 / s.0 as f32, 1.0 / s.1 as f32];
        let passes = [
            (
                BlurParams {
                    texsize: texsize(src_size),
                    ws: w_h,
                    ds: d_h,
                    wdiv: 0.5 / (w_h[0] + w_h[1] + w_h[2] + w_h[3]),
                    scale,
                    bias,
                    ..Default::default()
                },
                src_view,
                &h.view,
                &shared.blur_h,
            ),
            (
                BlurParams {
                    texsize: texsize(h.size),
                    ws: wds,
                    wdiv: 1.0 / ((w1v + w2v) * 2.0),
                    ed1: 1.0 - b1ed,
                    ed2: b1ed,
                    ed3: 5.0,
                    ..Default::default()
                },
                &h.view,
                &v.view,
                &shared.blur_v,
            ),
        ];
        for (i, (params, src, dst, pipeline)) in passes.into_iter().enumerate() {
            let buffer = &instance.blur_uniforms[level * 2 + i];
            queue.write_buffer(buffer, 0, bytemuck::bytes_of(&params));
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("milkdrop blur"),
                layout: &shared.blur_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(src),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&shared.samplers[1]),
                    },
                ],
            });
            let mut rp = crate::pass(encoder, dst, true);
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..3, 0..1);
        }
    }

    // Sprites over the warped image: the new preset's, then the old one's.
    let sprite_sampler = if wrap {
        &shared.samplers[0]
    } else {
        &shared.samplers[1]
    };
    let group = texture_group(
        device,
        &shared.sprite_layout,
        &instance.feedback[prev].view,
        sprite_sampler,
    );
    {
        let mut rp = crate::pass(encoder, &instance.feedback[target].view, false);
        rp.set_bind_group(0, &group, &[]);
        let runs = std::iter::once((&*run, &out)).chain(old.as_ref().map(|(r, _, o)| (*r, o)));
        for (r, o) in runs {
            if o.draws.is_empty() {
                continue;
            }
            rp.set_vertex_buffer(0, r.sprite_vertices.slice(..));
            for draw in &o.draws {
                rp.set_pipeline(&shared.sprites[&(draw.blend, draw.textured)]);
                rp.draw(draw.vertices.clone(), 0..1);
            }
        }
    }

    // Composite into the layer, the new preset fading in over the old one.
    {
        let mut rp = crate::pass(encoder, output, true);
        rp.set_index_buffer(shared.comp_indices.slice(..), wgpu::IndexFormat::Uint32);
        if let Some((old_run, old_compiled, old_out)) = &old {
            let group = shared.shader_group(
                device,
                &old_run.uniforms,
                &instance.feedback[target].view,
                blur_views,
                old_out.shader.wrap,
                input_view,
            );
            rp.set_pipeline(&old_compiled.comp);
            rp.set_bind_group(0, &group, &[]);
            rp.set_vertex_buffer(0, old_run.comp_vertices.slice(..));
            rp.draw_indexed(0..shared.comp_index_count, 0, 0..1);
        }
        let group = shared.shader_group(
            device,
            &run.uniforms,
            &instance.feedback[target].view,
            blur_views,
            wrap,
            input_view,
        );
        rp.set_pipeline(if old.is_some() {
            &current.comp_fade
        } else {
            &current.comp
        });
        rp.set_bind_group(0, &group, &[]);
        rp.set_vertex_buffer(0, run.comp_vertices.slice(..));
        rp.draw_indexed(0..shared.comp_index_count, 0, 0..1);
    }
    instance.current = target;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_sizes_follow_butterchurn() {
        assert_eq!(blur_size((1280, 720), 0.5), (640, 360));
        assert_eq!(blur_size((1280, 720), 0.0625), (80, 48));
        assert_eq!(blur_size((100, 100), 0.0625), (16, 16));
    }

    #[test]
    fn noise_is_deterministic_and_smooth() {
        let a = noise_2d(32, 4, &mut Rng::new(1));
        let b = noise_2d(32, 4, &mut Rng::new(1));
        assert_eq!(a, b);
        assert_eq!(noise_3d(8, 4, &mut Rng::new(1)).len(), 8 * 8 * 8 * 4);
    }

    #[test]
    fn grid_matches_engine_order() {
        assert_eq!(grid_position(0, 0, 4, 2), [-1.0, 1.0]);
        assert_eq!(grid_position(4, 2, 4, 2), [1.0, -1.0]);
        assert_eq!(grid_indices(1, 1), vec![0, 2, 1, 2, 3, 1]);
    }
}
