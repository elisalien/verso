//! Advanced Output: renders each screen of an [`OutputSetup`] into its own
//! texture, drawing its slices with their warps.
//!
//! A slice samples the composition, or one layer or group image at its
//! opacity (group images need [`Renderer::keep_group_images`]). Soft-edged
//! slices blend where their inputs overlap (see `verso_core::output`).

use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use verso_core::Composition;
use verso_core::output::{ColorAdjust, InputSource, MAX_SOFT_SLICES, OutputSetup, Screen};

use crate::{OUTPUT_FORMAT, Renderer, Target, shader};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    /// Clip space.
    position: [f32; 2],
    uv: [f32; 2],
    /// Brightness, contrast, opacity, source gain.
    adjust: [f32; 4],
    /// RGB offsets; `w` is 1 on the highlighted slice.
    rgb: [f32; 4],
    /// Index in the soft edge quads, or -1.
    soft: f32,
}

/// `Soft` in `slice.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct SoftUniform {
    params: [f32; 4],
    gamma_rgb: [f32; 4],
    canvas: [f32; 4],
    quads: [[f32; 4]; 2 * MAX_SOFT_SLICES],
}

impl SoftUniform {
    /// The soft edge of `setup`, and the index of each soft-edged slice.
    fn new(setup: &OutputSetup) -> (Self, Vec<u64>) {
        let quads = setup.soft_quads();
        let soft = &setup.soft_edge;
        let mut uniform = Self {
            params: [
                soft.power as f32,
                soft.luminance as f32,
                soft.gamma as f32,
                quads.len() as f32,
            ],
            gamma_rgb: [
                soft.gamma_rgb[0] as f32,
                soft.gamma_rgb[1] as f32,
                soft.gamma_rgb[2] as f32,
                0.0,
            ],
            canvas: [setup.canvas.0 as f32, setup.canvas.1 as f32, 0.0, 0.0],
            quads: [[0.0; 4]; 2 * MAX_SOFT_SLICES],
        };
        for (i, (_, q)) in quads.iter().enumerate() {
            uniform.quads[2 * i] = [q[0][0], q[0][1], q[1][0], q[1][1]];
            uniform.quads[2 * i + 1] = [q[2][0], q[2][1], q[3][0], q[3][1]];
        }
        (uniform, quads.into_iter().map(|(id, _)| id).collect())
    }
}

/// A test pattern shown on a screen instead of its slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pattern {
    /// 100 px grid with numbered cells, centre axes, border and circle.
    Grid,
    /// Color bars over mid grey.
    Colors,
    /// Black to white ramp, and 10 % steps.
    Gradient,
    /// Each slice in its own color, with its number.
    Slices,
    /// The screen's color, border, number (from 1) and resolution.
    Identify { number: u32 },
}

impl Pattern {
    /// Patterns offered in the interface (Identify has its own button).
    pub const CHOICES: [Pattern; 4] = [
        Pattern::Grid,
        Pattern::Colors,
        Pattern::Gradient,
        Pattern::Slices,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Pattern::Grid => "Grid",
            Pattern::Colors => "Colors",
            Pattern::Gradient => "Gradient",
            Pattern::Slices => "Slices",
            Pattern::Identify { .. } => "Identify",
        }
    }

    fn mode(self) -> f32 {
        match self {
            Pattern::Grid => 0.0,
            Pattern::Colors => 1.0,
            Pattern::Gradient => 2.0,
            Pattern::Slices => 3.0,
            Pattern::Identify { .. } => 4.0,
        }
    }
}

/// Distinct hue for item `index`, 0–1.
fn hue_of(index: usize) -> f32 {
    (index as f32 * 0.618_034).fract()
}

/// Vertex of `pattern.wgsl`.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct PatternVertex {
    position: [f32; 2],
    /// Mode, number, center x, center y.
    info: [f32; 4],
    /// Screen width, height, hue.
    extra: [f32; 4],
}

struct ScreenTarget {
    id: u64,
    size: (u32, u32),
    target: Target,
}

pub struct ScreenRenderer {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    vertices: Option<wgpu::Buffer>,
    soft: wgpu::Buffer,
    pattern_pipeline: wgpu::RenderPipeline,
    pattern_vertices: Option<wgpu::Buffer>,
    /// Screens showing a test pattern instead of their slices.
    patterns: Vec<(u64, Pattern)>,
    screens: Vec<ScreenTarget>,
    /// Slice drawn with a striped tint on its screen, to find it on the
    /// physical output.
    highlight: Option<u64>,
}

impl ScreenRenderer {
    pub fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("slice"),
            entries: &[
                crate::texture_entry(0, true),
                crate::sampler_entry(1),
                crate::uniform_entry(2),
            ],
        });
        let module = shader(device, "slice", include_str!("shaders/slice.wgsl"));
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("slice"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("slice"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, 1 => Float32x2, 2 => Float32x4, 3 => Float32x4,
                        4 => Float32
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pattern_module = shader(device, "pattern", include_str!("shaders/pattern.wgsl"));
        let pattern_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("pattern"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pattern_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("pattern"),
            layout: Some(&pattern_layout),
            vertex: wgpu::VertexState {
                module: &pattern_module,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<PatternVertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![
                        0 => Float32x2, 1 => Float32x4, 2 => Float32x4
                    ],
                })],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &pattern_module,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: OUTPUT_FORMAT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            layout,
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("slice"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            vertices: None,
            soft: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("soft edge"),
                size: std::mem::size_of::<SoftUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            pattern_pipeline,
            pattern_vertices: None,
            patterns: Vec::new(),
            screens: Vec::new(),
            highlight: None,
        }
    }

    /// Test patterns to show from the next render, by screen id. Other
    /// screens show their slices.
    pub fn set_patterns(&mut self, patterns: Vec<(u64, Pattern)>) {
        self.patterns = patterns;
    }

    /// Slice to highlight on the screens from the next render, or none.
    pub fn set_highlight(&mut self, slice: Option<u64>) {
        self.highlight = slice;
    }

    /// Renders the enabled video screens of `setup` from `source`, the
    /// images `renderer` made of `comp` this frame. Returns the ids of
    /// screens whose texture was created or recreated.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        setup: &OutputSetup,
        renderer: &Renderer,
        comp: &Composition,
    ) -> Vec<u64> {
        let screens: Vec<&Screen> = setup.screens.iter().filter(|s| renders(s)).collect();
        let mut created = Vec::new();
        self.screens
            .retain(|t| screens.iter().any(|s| s.id == t.id));
        for screen in &screens {
            let size = screen.size(setup.canvas);
            let size = (size.0.clamp(1, 8192), size.1.clamp(1, 8192));
            match self.screens.iter_mut().find(|t| t.id == screen.id) {
                Some(t) if t.size == size => {}
                Some(t) => {
                    *t = ScreenTarget::new(device, screen.id, size);
                    created.push(screen.id);
                }
                None => {
                    self.screens
                        .push(ScreenTarget::new(device, screen.id, size));
                    created.push(screen.id);
                }
            }
        }

        let (soft, soft_ids) = SoftUniform::new(setup);
        queue.write_buffer(&self.soft, 0, bytemuck::bytes_of(&soft));

        // One buffer for every screen: each slice draws its own range.
        let mut vertices = Vec::new();
        let mut draws: Vec<Vec<(Range<u32>, InputSource)>> = Vec::with_capacity(screens.len());
        for screen in &screens {
            let size = self.target(screen.id).expect("created above").size;
            let (w, h) = (size.0 as f32, size.1 as f32);
            let mut ranges = Vec::new();
            // The first slice of the list is on top: draw it last.
            for slice in screen.slices.iter().rev().filter(|s| s.enabled) {
                let source = slice.input_source();
                // Layer and group images are drawn at their opacity.
                let gain = match source {
                    InputSource::Composition => 1.0,
                    InputSource::Layer(i) => comp.layer_level(i),
                    InputSource::Group(g) => comp.group_gain(g),
                };
                let (mut adjust, mut rgb) = combine(&screen.color, &slice.color);
                adjust[3] = gain;
                if self.highlight == Some(slice.id) {
                    rgb[3] = 1.0;
                }
                let soft = soft_ids
                    .iter()
                    .position(|&id| id == slice.id)
                    .map_or(-1.0, |i| i as f32);
                let start = vertices.len() as u32;
                vertices.extend(slice.mesh(setup.canvas).into_iter().map(|v| Vertex {
                    position: [v.position[0] / w * 2.0 - 1.0, 1.0 - v.position[1] / h * 2.0],
                    uv: v.uv,
                    adjust,
                    rgb,
                    soft,
                }));
                ranges.push((start..vertices.len() as u32, source));
            }
            draws.push(ranges);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&vertices);
        if self
            .vertices
            .as_ref()
            .is_none_or(|b| b.size() < bytes.len() as u64)
        {
            self.vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("slices"),
                size: (bytes.len() as u64).next_power_of_two().max(4096),
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        let buffer = self.vertices.as_ref().expect("just created");
        if !bytes.is_empty() {
            queue.write_buffer(buffer, 0, bytes);
        }

        // Test patterns: one buffer for every screen showing one.
        let mut pattern_vertices: Vec<PatternVertex> = Vec::new();
        let mut pattern_draws: Vec<Option<Range<u32>>> = Vec::with_capacity(screens.len());
        for screen in &screens {
            let Some(&(_, pattern)) = self.patterns.iter().find(|(id, _)| *id == screen.id) else {
                pattern_draws.push(None);
                continue;
            };
            let size = self.target(screen.id).expect("created above").size;
            let (w, h) = (size.0 as f32, size.1 as f32);
            let clip = |p: [f32; 2]| [p[0] / w * 2.0 - 1.0, 1.0 - p[1] / h * 2.0];
            let start = pattern_vertices.len() as u32;
            if pattern == Pattern::Slices {
                for (i, slice) in screen.slices.iter().enumerate().rev() {
                    if !slice.enabled {
                        continue;
                    }
                    let outline = slice.screen_outline();
                    let n = outline.len().max(1) as f32;
                    let center = outline
                        .iter()
                        .fold([0.0, 0.0], |c, p| [c[0] + p[0] / n, c[1] + p[1] / n]);
                    pattern_vertices.extend(slice.mesh(setup.canvas).into_iter().map(|v| {
                        PatternVertex {
                            position: clip(v.position),
                            info: [pattern.mode(), (i + 1) as f32, center[0], center[1]],
                            extra: [w, h, hue_of(i), 0.0],
                        }
                    }));
                }
            } else {
                let (number, hue) = match pattern {
                    Pattern::Identify { number } => {
                        (number, hue_of(number.saturating_sub(1) as usize))
                    }
                    _ => (0, 0.0),
                };
                let corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
                for k in [0, 1, 2, 0, 2, 3] {
                    pattern_vertices.push(PatternVertex {
                        position: clip(corners[k]),
                        info: [pattern.mode(), number as f32, w * 0.5, h * 0.5],
                        extra: [w, h, hue, 0.0],
                    });
                }
            }
            pattern_draws.push(Some(start..pattern_vertices.len() as u32));
        }
        let pattern_bytes: &[u8] = bytemuck::cast_slice(&pattern_vertices);
        if !pattern_bytes.is_empty() {
            if self
                .pattern_vertices
                .as_ref()
                .is_none_or(|b| b.size() < pattern_bytes.len() as u64)
            {
                self.pattern_vertices = Some(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("patterns"),
                    size: (pattern_bytes.len() as u64).next_power_of_two().max(4096),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
            }
            let buffer = self.pattern_vertices.as_ref().expect("just created");
            queue.write_buffer(buffer, 0, pattern_bytes);
        }

        // A bind group per source in use this frame.
        let mut bind_groups: Vec<(InputSource, wgpu::BindGroup)> = Vec::new();
        for &(_, source) in draws.iter().flatten() {
            if bind_groups.iter().any(|(s, _)| *s == source) {
                continue;
            }
            let view = match source {
                InputSource::Composition => Some(renderer.output_view()),
                InputSource::Layer(i) => renderer.layer_view(i),
                InputSource::Group(g) => renderer.group_view(g),
            }
            .unwrap_or(renderer.blank_view());
            let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("slice"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.soft.as_entire_binding(),
                    },
                ],
            });
            bind_groups.push((source, group));
        }
        for ((screen, ranges), pattern) in screens.iter().zip(draws).zip(pattern_draws) {
            let target = &self.target(screen.id).expect("created above").target;
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("screen"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target.view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(range) = pattern {
                // The pattern replaces the slices.
                if let Some(vertices) = &self.pattern_vertices
                    && !range.is_empty()
                {
                    rp.set_pipeline(&self.pattern_pipeline);
                    rp.set_vertex_buffer(0, vertices.slice(..));
                    rp.draw(range, 0..1);
                }
                continue;
            }
            rp.set_pipeline(&self.pipeline);
            rp.set_vertex_buffer(0, buffer.slice(..));
            for (range, source) in ranges {
                if range.is_empty() {
                    continue;
                }
                let (_, group) = bind_groups
                    .iter()
                    .find(|(s, _)| *s == source)
                    .expect("made above");
                rp.set_bind_group(0, group, &[]);
                rp.draw(range, 0..1);
            }
        }
        created
    }

    fn target(&self, id: u64) -> Option<&ScreenTarget> {
        self.screens.iter().find(|t| t.id == id)
    }

    /// Rendered image of screen `id`, in [`OUTPUT_FORMAT`].
    pub fn texture(&self, id: u64) -> Option<&wgpu::Texture> {
        self.target(id).map(|t| &t.target.texture)
    }

    pub fn view(&self, id: u64) -> Option<&wgpu::TextureView> {
        self.target(id).map(|t| &t.target.view)
    }

    pub fn size(&self, id: u64) -> Option<(u32, u32)> {
        self.target(id).map(|t| t.size)
    }
}

impl ScreenTarget {
    fn new(device: &wgpu::Device, id: u64, size: (u32, u32)) -> Self {
        Self {
            id,
            size,
            target: Target::new(device, "screen", size, OUTPUT_FORMAT),
        }
    }
}

/// Whether `screen` is drawn: enabled, and a video screen.
pub fn renders(screen: &Screen) -> bool {
    screen.enabled && !screen.dmx && screen.device.kind != verso_core::output::DeviceKind::Dmx
}

/// Screen and slice corrections as one: offsets add, contrasts multiply.
fn combine(screen: &ColorAdjust, slice: &ColorAdjust) -> ([f32; 4], [f32; 4]) {
    let contrast = (1.0 + screen.contrast) * (1.0 + slice.contrast) - 1.0;
    (
        [
            (screen.brightness + slice.brightness) as f32,
            contrast as f32,
            screen.opacity as f32,
            0.0,
        ],
        [
            (screen.rgb[0] + slice.rgb[0]) as f32,
            (screen.rgb[1] + slice.rgb[1]) as f32,
            (screen.rgb[2] + slice.rgb[2]) as f32,
            0.0,
        ],
    )
}
