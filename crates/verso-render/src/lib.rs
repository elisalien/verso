//! wgpu compositing engine for Verso.
//!
//! Per layer: the clip's source renders into the layer's textures, clip effects
//! run, a transition pass crossfades with the previous clip, layer effects run,
//! and the result blends onto its destination. Destinations are ping-pong
//! texture pairs, so each blend can read what lies below. Grouped layers blend
//! into a group pair first; the group then gets its effects and blends onto the
//! composition. Composition effects run last, then the master writes the output.

mod effects;
mod grab;
mod media;
pub mod milkdrop;
mod slices;
mod thumbnails;

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use verso_core::{BlendMode, Composition, Effect, Source};

pub use effects::{EffectDef, EffectLibrary, ParamDef, ParamKind, ShaderKind};
pub use grab::{FrameGrabber, GrabbedFrame};
pub use media::{MediaTexture, MediaView};
pub use slices::{Pattern, ScreenRenderer};
pub use thumbnails::Thumbnails;

/// Internal format: HDR so successive blends do not band.
const WORK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Format of [`Renderer::output_view`].
pub const OUTPUT_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;

/// Shader index of a solid color in `generator.wgsl`.
const SOLID_KIND: u32 = 255;
/// Size of each pooled uniform buffer; fits every uniform struct.
const UNIFORM_SIZE: u64 = 256;

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

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct MediaParams {
    scale: [f32; 2],
    _pad: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct TransitionParams {
    progress: f32,
    _pad: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct EffectParams {
    time: f32,
    aspect: f32,
    width: f32,
    height: f32,
    values: [f32; effects::PARAM_SLOTS],
}

impl MediaParams {
    /// Fits an image of `media` size inside a target of `target` size.
    fn fit(media: (u32, u32), target: (u32, u32)) -> Self {
        let media_aspect = media.0 as f32 / media.1.max(1) as f32;
        let target_aspect = target.0 as f32 / target.1.max(1) as f32;
        let scale = if media_aspect > target_aspect {
            [1.0, target_aspect / media_aspect]
        } else {
            [media_aspect / target_aspect, 1.0]
        };
        Self {
            scale,
            _pad: [0.0; 2],
        }
    }
}

impl GeneratorParams {
    /// Parameters for a procedural source; `None` for file-based sources.
    fn new(source: &Source, time: f32, aspect: f32) -> Option<Self> {
        let (kind, color, speed) = match *source {
            Source::Solid { color } => (SOLID_KIND, color, 0.0),
            Source::Generator { kind, color, speed } => (kind.shader_index(), color, speed),
            Source::Image { .. }
            | Source::Video { .. }
            | Source::Shader(_)
            | Source::Milkdrop { .. } => return None,
        };
        Some(Self {
            time,
            speed,
            kind,
            aspect,
            color,
        })
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
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        Self { texture, view }
    }
}

/// Two same-size textures used alternately: one is read while the other is
/// written. `current` holds the latest result.
struct Pair {
    targets: [Target; 2],
    current: usize,
}

impl Pair {
    fn new(device: &wgpu::Device, label: &str, size: (u32, u32)) -> Self {
        Self {
            targets: [
                Target::new(device, label, size, WORK_FORMAT),
                Target::new(device, label, size, WORK_FORMAT),
            ],
            current: 0,
        }
    }

    fn current(&self) -> &Target {
        &self.targets[self.current]
    }

    /// (latest result, free texture to write the next one).
    fn split(&self) -> (&Target, &Target) {
        (&self.targets[self.current], &self.targets[1 - self.current])
    }

    fn flip(&mut self) {
        self.current = 1 - self.current;
    }

    fn clear(&mut self, encoder: &mut wgpu::CommandEncoder) {
        drop(pass(encoder, &self.current().view, true));
    }
}

/// Uniform buffers handed out during a frame and reused on the next one.
/// Each draw gets its own buffer because `write_buffer` calls all land before
/// the frame's commands run.
#[derive(Default)]
struct UniformPool {
    buffers: Vec<wgpu::Buffer>,
    next: usize,
}

impl UniformPool {
    fn push(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, data: &[u8]) -> usize {
        if self.next == self.buffers.len() {
            self.buffers
                .push(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("uniforms"),
                    size: UNIFORM_SIZE,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                }));
        }
        queue.write_buffer(&self.buffers[self.next], 0, data);
        self.next += 1;
        self.next - 1
    }
}

/// A clip change being crossfaded.
#[derive(Clone, Copy)]
struct Transition {
    start: f32,
    duration: f32,
}

/// GPU resources and playback state of one layer.
struct LayerState {
    /// Source render and effect passes.
    work: Pair,
    /// Result after the transition pass, before layer effects. Kept between
    /// frames so a clip change can freeze it as the outgoing image.
    mixed: Target,
    outgoing: Target,
    transition: Option<Transition>,
    /// Column shown last frame.
    shown: Option<usize>,
    /// Where this frame's image of the layer is, after its effects: `None`
    /// when the layer drew nothing.
    image: Option<LayerImage>,
}

#[derive(Clone, Copy)]
enum LayerImage {
    Mixed,
    Work,
}

impl LayerState {
    fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        Self {
            work: Pair::new(device, "layer", size),
            mixed: Target::new(device, "layer mixed", size, WORK_FORMAT),
            outgoing: Target::new(device, "layer outgoing", size, WORK_FORMAT),
            transition: None,
            shown: None,
            image: None,
        }
    }
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

fn texture_entry(binding: u32, filterable: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable },
            view_dimension: wgpu::TextureViewDimension::D2,
            multisampled: false,
        },
        count: None,
    }
}

fn sampler_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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

fn pipeline(
    device: &wgpu::Device,
    label: &str,
    vertex: (&wgpu::ShaderModule, &str),
    fragment: (&wgpu::ShaderModule, &str),
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
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
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: fragment.0,
            entry_point: Some(fragment.1),
            compilation_options: Default::default(),
            targets: &[Some(format.into())],
        }),
        multiview_mask: None,
        cache: None,
    })
}

fn shader(device: &wgpu::Device, label: &str, source: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    })
}

/// Pipeline for a shader holding both `vs_main` and `fs_main`.
fn fullscreen_pipeline(
    device: &wgpu::Device,
    label: &str,
    source: &str,
    layout: &wgpu::BindGroupLayout,
    format: wgpu::TextureFormat,
) -> wgpu::RenderPipeline {
    let module = shader(device, label, source);
    pipeline(
        device,
        label,
        (&module, "vs_main"),
        (&module, "fs_main"),
        layout,
        format,
    )
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

/// Pipelines and layouts; immutable while a frame records.
struct Pipelines {
    generator: wgpu::RenderPipeline,
    generator_layout: wgpu::BindGroupLayout,
    media: wgpu::RenderPipeline,
    media_layout: wgpu::BindGroupLayout,
    blend: wgpu::RenderPipeline,
    /// Uniform + two unfilterable textures: blend and transition passes.
    two_textures_layout: wgpu::BindGroupLayout,
    transition: wgpu::RenderPipeline,
    output: wgpu::RenderPipeline,
    output_layout: wgpu::BindGroupLayout,
    /// Uniform + filterable texture + sampler: media and effect passes.
    effect_layout: wgpu::BindGroupLayout,
    effect_vs: wgpu::ShaderModule,
    sampler: wgpu::Sampler,
}

/// Per-frame values shared by every pass.
struct Frame<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    pipelines: &'a Pipelines,
    effects: &'a HashMap<(String, wgpu::TextureFormat), wgpu::RenderPipeline>,
    library: &'a EffectLibrary,
    /// Input of generator shaders.
    blank: &'a wgpu::TextureView,
    size: (u32, u32),
    time: f32,
}

impl Frame<'_> {
    fn bind(
        &self,
        layout: &wgpu::BindGroupLayout,
        uniforms: &UniformPool,
        uniform: usize,
        resources: &[wgpu::BindingResource<'_>],
    ) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry {
            binding: 0,
            resource: uniforms.buffers[uniform].as_entire_binding(),
        }];
        entries.extend(
            resources
                .iter()
                .enumerate()
                .map(|(i, resource)| wgpu::BindGroupEntry {
                    binding: i as u32 + 1,
                    resource: resource.clone(),
                }),
        );
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        })
    }

    /// Mixes `layer` onto `dest` with `mode` at `opacity`.
    fn blend(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &mut UniformPool,
        dest: &mut Pair,
        layer: &wgpu::TextureView,
        mode: BlendMode,
        opacity: f32,
    ) {
        let params = BlendParams {
            mode: mode.shader_index(),
            opacity,
            _pad: [0.0; 2],
        };
        let u = uniforms.push(self.device, self.queue, bytemuck::bytes_of(&params));
        let (base, out) = dest.split();
        let group = self.bind(
            &self.pipelines.two_textures_layout,
            uniforms,
            u,
            &[
                wgpu::BindingResource::TextureView(&base.view),
                wgpu::BindingResource::TextureView(layer),
            ],
        );
        let mut rp = pass(encoder, &out.view, true);
        rp.set_pipeline(&self.pipelines.blend);
        rp.set_bind_group(0, &group, &[]);
        rp.draw(0..3, 0..1);
        drop(rp);
        dest.flip();
    }

    /// Runs the active effects of `chain`. The first reads `input`, or the
    /// pair's current texture when `None`; the result ends in `pair.current()`.
    /// Returns `false` when no effect ran, leaving the pair untouched.
    fn run_chain(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &mut UniformPool,
        chain: &[Effect],
        input: Option<&wgpu::TextureView>,
        pair: &mut Pair,
    ) -> bool {
        let mut ran = false;
        for effect in chain.iter().filter(|e| !e.bypass) {
            let (current, out) = pair.split();
            let source = match (ran, input) {
                (false, Some(view)) => view,
                _ => &current.view,
            };
            if self.draw_effect(encoder, uniforms, effect, source, &out.view, WORK_FORMAT) {
                pair.flip();
                ran = true;
            }
        }
        ran
    }

    /// Draws one effect from `input` into `output`. Returns `false` when the
    /// effect is unknown or not compiled for `format`.
    fn draw_effect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        uniforms: &mut UniformPool,
        effect: &Effect,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        format: wgpu::TextureFormat,
    ) -> bool {
        let (Some(def), Some(pipeline)) = (
            self.library.get(&effect.id),
            self.effects.get(&(effect.id.clone(), format)),
        ) else {
            return false;
        };
        let params = EffectParams {
            time: self.time,
            aspect: self.size.0 as f32 / self.size.1 as f32,
            width: self.size.0 as f32,
            height: self.size.1 as f32,
            values: def.pack(effect),
        };
        let u = uniforms.push(self.device, self.queue, bytemuck::bytes_of(&params));
        let group = self.bind(
            &self.pipelines.effect_layout,
            uniforms,
            u,
            &[
                wgpu::BindingResource::TextureView(input),
                wgpu::BindingResource::Sampler(&self.pipelines.sampler),
            ],
        );
        let mut rp = pass(encoder, output, true);
        rp.set_pipeline(pipeline);
        rp.set_bind_group(0, &group, &[]);
        rp.draw(0..3, 0..1);
        true
    }
}

pub struct Renderer {
    size: (u32, u32),
    pipelines: Pipelines,
    library: EffectLibrary,
    /// Compiled effect pipelines by effect id and target format.
    effects: HashMap<(String, wgpu::TextureFormat), wgpu::RenderPipeline>,
    /// 1×1 transparent texture: the input of generator shaders.
    blank: Target,
    composition: Pair,
    /// One pair per group nesting depth.
    groups: Vec<Pair>,
    /// A copy of each group's image, kept when [`Self::keep_group_images`]
    /// is on: group pairs are reused by sibling groups.
    group_images: Vec<Target>,
    keep_groups: bool,
    output: Target,
    layers: Vec<LayerState>,
    uniforms: UniformPool,
    milk: milkdrop::MilkRenderer,
}

impl Renderer {
    pub fn new(device: &wgpu::Device, size: (u32, u32)) -> Self {
        let generator_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("generator"),
            entries: &[uniform_entry(0)],
        });
        let two_textures_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("two textures"),
                entries: &[
                    uniform_entry(0),
                    texture_entry(1, false),
                    texture_entry(2, false),
                ],
            });
        let effect_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("effect"),
            entries: &[uniform_entry(0), texture_entry(1, true), sampler_entry(2)],
        });
        let output_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("output"),
            entries: &[uniform_entry(0), texture_entry(1, false)],
        });
        let pipelines = Pipelines {
            generator: fullscreen_pipeline(
                device,
                "generator",
                include_str!("shaders/generator.wgsl"),
                &generator_layout,
                WORK_FORMAT,
            ),
            media: fullscreen_pipeline(
                device,
                "media",
                include_str!("shaders/media.wgsl"),
                &effect_layout,
                WORK_FORMAT,
            ),
            blend: fullscreen_pipeline(
                device,
                "blend",
                include_str!("shaders/blend.wgsl"),
                &two_textures_layout,
                WORK_FORMAT,
            ),
            transition: fullscreen_pipeline(
                device,
                "transition",
                include_str!("shaders/transition.wgsl"),
                &two_textures_layout,
                WORK_FORMAT,
            ),
            output: fullscreen_pipeline(
                device,
                "output",
                include_str!("shaders/output.wgsl"),
                &output_layout,
                OUTPUT_FORMAT,
            ),
            effect_vs: shader(
                device,
                "effect vertex",
                include_str!("shaders/effect_vs.wgsl"),
            ),
            sampler: device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("linear"),
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            }),
            generator_layout,
            media_layout: effect_layout.clone(),
            two_textures_layout,
            output_layout,
            effect_layout,
        };
        Self {
            size,
            pipelines,
            library: EffectLibrary::builtin(),
            effects: HashMap::new(),
            blank: Target::new(device, "blank", (1, 1), WORK_FORMAT),
            composition: Pair::new(device, "composition", size),
            groups: Vec::new(),
            group_images: Vec::new(),
            keep_groups: false,
            output: Target::new(device, "output", size, OUTPUT_FORMAT),
            layers: Vec::new(),
            uniforms: UniformPool::default(),
            milk: milkdrop::MilkRenderer::default(),
        }
    }

    /// Composition size in pixels.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    /// Keeps a copy of each group's image for [`Self::group_view`], at the
    /// cost of one texture copy per group and frame.
    pub fn keep_group_images(&mut self, keep: bool) {
        self.keep_groups = keep;
        if !keep {
            self.group_images.clear();
        }
    }

    /// This frame's image of layer `index` after its effects, before its
    /// opacity: linear, premultiplied, in a float format. `None` when the
    /// layer drew nothing.
    pub fn layer_view(&self, index: usize) -> Option<&wgpu::TextureView> {
        let state = self.layers.get(index)?;
        Some(match state.image? {
            LayerImage::Mixed => &state.mixed.view,
            LayerImage::Work => &state.work.current().view,
        })
    }

    /// This frame's image of group `index` after its effects, like
    /// [`Self::layer_view`]. Needs [`Self::keep_group_images`].
    pub fn group_view(&self, index: usize) -> Option<&wgpu::TextureView> {
        self.group_images.get(index).map(|t| &t.view)
    }

    /// A transparent texture, for sources that drew nothing.
    pub fn blank_view(&self) -> &wgpu::TextureView {
        &self.blank.view
    }

    /// Final image, in [`OUTPUT_FORMAT`]. Recreated when the size changes.
    pub fn output_view(&self) -> &wgpu::TextureView {
        &self.output.view
    }

    pub fn output_texture(&self) -> &wgpu::Texture {
        &self.output.texture
    }

    pub fn library(&self) -> &EffectLibrary {
        &self.library
    }

    /// Changing the library drops compiled pipelines; they rebuild on demand.
    pub fn library_mut(&mut self) -> &mut EffectLibrary {
        self.effects.clear();
        &mut self.library
    }

    /// MilkDrop presets and their running state.
    pub fn milkdrop(&self) -> &milkdrop::MilkRenderer {
        &self.milk
    }

    pub fn milkdrop_mut(&mut self) -> &mut milkdrop::MilkRenderer {
        &mut self.milk
    }

    /// Recreates every size-dependent resource.
    pub fn resize(&mut self, device: &wgpu::Device, size: (u32, u32)) {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return;
        }
        self.size = size;
        self.composition = Pair::new(device, "composition", size);
        self.groups.clear();
        self.group_images.clear();
        self.output = Target::new(device, "output", size, OUTPUT_FORMAT);
        self.layers.clear();
        self.milk.reset();
    }

    /// Compiles the pipelines of every effect `comp` uses.
    fn prepare_effects(&mut self, device: &wgpu::Device, comp: &Composition) {
        let layers = comp.layers.iter().flat_map(|l| {
            l.effects
                .iter()
                .chain(l.clips.iter().flatten().flat_map(|c| {
                    let source = match &c.source {
                        Source::Shader(effect) => Some(effect),
                        _ => None,
                    };
                    c.effects.iter().chain(source)
                }))
        });
        let used = comp
            .effects
            .iter()
            .chain(comp.groups.iter().flat_map(|g| &g.effects))
            .chain(layers);
        let ids: Vec<String> = used.map(|e| e.id.clone()).collect();
        for id in ids {
            self.compile_effect(device, &id, WORK_FORMAT);
        }
    }

    /// Compiles the pipeline of effect `id` for `format` if needed.
    fn compile_effect(&mut self, device: &wgpu::Device, id: &str, format: wgpu::TextureFormat) {
        let key = (id.to_owned(), format);
        if self.effects.contains_key(&key) {
            return;
        }
        let Some(def) = self.library.get(id) else {
            return;
        };
        let fragment = shader(device, &def.name, &def.fragment);
        let pipeline = pipeline(
            device,
            &def.name,
            (&self.pipelines.effect_vs, "vs_main"),
            (&fragment, def.entry),
            &self.pipelines.effect_layout,
            format,
        );
        self.effects.insert(key, pipeline);
    }

    /// Renders a generator shader into `output`, a [`OUTPUT_FORMAT`] texture of
    /// `size`, for clip previews. Call after [`Self::render`] in the same frame.
    #[allow(clippy::too_many_arguments)]
    pub fn render_shader_preview(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        effect: &Effect,
        output: &wgpu::TextureView,
        size: (u32, u32),
        time: f32,
    ) -> bool {
        self.compile_effect(device, &effect.id, OUTPUT_FORMAT);
        let frame = Frame {
            device,
            queue,
            pipelines: &self.pipelines,
            effects: &self.effects,
            library: &self.library,
            blank: &self.blank.view,
            size,
            time,
        };
        drop(pass(encoder, &self.blank.view, true));
        frame.draw_effect(
            encoder,
            &mut self.uniforms,
            effect,
            &self.blank.view,
            output,
            OUTPUT_FORMAT,
        )
    }

    /// Renders `comp` at `time` seconds into the output texture.
    ///
    /// `media[i]` is the decoded frame of layer `i` when its playing clip is an
    /// image or video. A layer whose media is not ready yet shows nothing.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        comp: &Composition,
        media: &[Option<MediaView<'_>>],
        time: f32,
    ) {
        self.resize(device, (comp.width, comp.height));
        self.prepare_effects(device, comp);
        while self.layers.len() < comp.layers.len() {
            self.layers.push(LayerState::new(device, self.size));
        }
        self.layers.truncate(comp.layers.len());
        let depth = (0..comp.groups.len())
            .map(|g| comp.group_depth(g) + 1)
            .max()
            .unwrap_or(0);
        while self.groups.len() < depth {
            self.groups.push(Pair::new(device, "group", self.size));
        }
        self.uniforms.next = 0;

        let frame = Frame {
            device,
            queue,
            pipelines: &self.pipelines,
            effects: &self.effects,
            library: &self.library,
            blank: &self.blank.view,
            size: self.size,
            time,
        };
        drop(pass(encoder, &self.blank.view, true));
        let uniforms = &mut self.uniforms;
        let composition = &mut self.composition;
        let group_pairs = &mut self.groups[..];
        if self.keep_groups {
            while self.group_images.len() < comp.groups.len() {
                self.group_images
                    .push(Target::new(device, "group image", self.size, WORK_FORMAT));
            }
            self.group_images.truncate(comp.groups.len());
        }
        let group_images = &mut self.group_images[..];
        composition.clear(encoder);
        self.milk.begin_frame();

        render_scope(
            &frame,
            encoder,
            uniforms,
            &mut self.milk,
            comp,
            media,
            None,
            composition,
            &mut self.layers,
            group_pairs,
            group_images,
        );
        self.milk.end_frame();
        frame.run_chain(encoder, uniforms, &comp.effects, None, composition);

        let params = OutputParams {
            opacity: comp.master.opacity,
            _pad: [0.0; 3],
        };
        let u = uniforms.push(device, queue, bytemuck::bytes_of(&params));
        let group = frame.bind(
            &self.pipelines.output_layout,
            uniforms,
            u,
            &[wgpu::BindingResource::TextureView(
                &composition.current().view,
            )],
        );
        let mut rp = pass(encoder, &self.output.view, true);
        rp.set_pipeline(&self.pipelines.output);
        rp.set_bind_group(0, &group, &[]);
        rp.draw(0..3, 0..1);
    }
}

/// Renders the layers of `scope` (a group, or the whole composition) onto
/// `dest`, bottom first. Each group inside renders into its own pair from
/// `spare`, gets its effects, and blends onto `dest` as one image.
#[allow(clippy::too_many_arguments)]
fn render_scope(
    frame: &Frame<'_>,
    encoder: &mut wgpu::CommandEncoder,
    uniforms: &mut UniformPool,
    milk: &mut milkdrop::MilkRenderer,
    comp: &Composition,
    media: &[Option<MediaView<'_>>],
    scope: Option<usize>,
    dest: &mut Pair,
    states: &mut [LayerState],
    spare: &mut [Pair],
    group_images: &mut [Target],
) {
    let mut done = vec![false; comp.groups.len()];
    for index in 0..comp.layers.len() {
        if scope.is_some_and(|s| !comp.layer_in_group(index, s)) {
            continue;
        }
        match comp.child_toward(scope, index) {
            None => {
                // A MilkDrop clip can read another layer's last image.
                let input = milk_input(comp, index)
                    .filter(|&i| i != index)
                    .and_then(|i| states.get(i))
                    .map(|s| s.mixed.view.clone());
                render_layer(
                    frame,
                    encoder,
                    uniforms,
                    milk,
                    comp,
                    media,
                    index,
                    &mut states[index],
                    input,
                    dest,
                )
            }
            Some(g) if !done[g] => {
                done[g] = true;
                let Some((pair, deeper)) = spare.split_first_mut() else {
                    continue;
                };
                pair.clear(encoder);
                render_scope(
                    frame,
                    encoder,
                    uniforms,
                    milk,
                    comp,
                    media,
                    Some(g),
                    pair,
                    states,
                    deeper,
                    group_images,
                );
                let group = &comp.groups[g];
                frame.run_chain(encoder, uniforms, &group.effects, None, pair);
                if let Some(image) = group_images.get(g) {
                    encoder.copy_texture_to_texture(
                        pair.current().texture.as_image_copy(),
                        image.texture.as_image_copy(),
                        image.texture.size(),
                    );
                }
                let gain = comp.group_gain(g);
                if gain > 0.0 {
                    frame.blend(
                        encoder,
                        uniforms,
                        dest,
                        &pair.current().view,
                        group.blend,
                        gain,
                    );
                }
            }
            Some(_) => {}
        }
    }
}

/// Layer whose image feeds the MilkDrop clip playing on layer `index`.
fn milk_input(comp: &Composition, index: usize) -> Option<usize> {
    match &comp.layers.get(index)?.active_clip()?.source {
        Source::Milkdrop { input, .. } => *input,
        _ => None,
    }
}

/// Renders layer `index` and blends it onto `dest`.
#[allow(clippy::too_many_arguments)]
fn render_layer(
    frame: &Frame<'_>,
    encoder: &mut wgpu::CommandEncoder,
    uniforms: &mut UniformPool,
    milk: &mut milkdrop::MilkRenderer,
    comp: &Composition,
    media: &[Option<MediaView<'_>>],
    index: usize,
    state: &mut LayerState,
    input: Option<wgpu::TextureView>,
    dest: &mut Pair,
) {
    let layer = &comp.layers[index];
    let active = layer.active.filter(|_| layer.active_clip().is_some());
    state.image = None;

    // A clip change starts a transition from the last shown image.
    if active != state.shown {
        let had_content = state.shown.is_some() || state.transition.is_some();
        // Between two MilkDrop clips, the preset blend does the transition on
        // the layer's shared feedback buffer instead of a crossfade.
        let is_milk = |column: Option<usize>| {
            column
                .and_then(|c| layer.clips.get(c))
                .and_then(Option::as_ref)
                .is_some_and(|clip| matches!(clip.source, Source::Milkdrop { .. }))
        };
        let milk_blend = is_milk(state.shown) && is_milk(active);
        state.transition = (layer.transition_time() > 0.0 && had_content && !milk_blend).then(|| {
            encoder.copy_texture_to_texture(
                state.mixed.texture.as_image_copy(),
                state.outgoing.texture.as_image_copy(),
                state.mixed.texture.size(),
            );
            Transition {
                start: frame.time,
                duration: layer.transition_time(),
            }
        });
        state.shown = active;
    }
    let progress = state
        .transition
        .map(|t| (frame.time - t.start) / t.duration);
    if progress.is_some_and(|p| p >= 1.0) {
        state.transition = None;
    }
    let transitioning = state.transition.is_some();
    if active.is_none() && !transitioning {
        return;
    }
    let level = comp.layer_level(index);

    // Incoming clip: source, then clip effects, in `state.work`.
    state.work.clear(encoder);
    if let Some(clip) = layer.active_clip() {
        let aspect = frame.size.0 as f32 / frame.size.1 as f32;
        if let Source::Shader(effect) = &clip.source {
            frame.draw_effect(
                encoder,
                uniforms,
                effect,
                frame.blank,
                &state.work.current().view,
                WORK_FORMAT,
            );
        }
        if let (
            Source::Milkdrop {
                family,
                variant,
                blend,
                input_mix,
                ..
            },
            Some(_),
        ) = (&clip.source, active)
        {
            let options = milkdrop::PlayOptions {
                blend: *blend,
                input: input.as_ref(),
                input_mix: *input_mix,
            };
            // One MilkDrop instance per layer: launching another MilkDrop clip
            // blends from the image the previous one left in the buffer.
            milk.draw(
                frame.device,
                frame.queue,
                encoder,
                (index, 0),
                family,
                *variant,
                options,
                &state.work.current().view,
                WORK_FORMAT,
                frame.size,
                frame.time,
            );
        }
        let drawn = match GeneratorParams::new(&clip.source, frame.time, aspect) {
            Some(params) => {
                let u = uniforms.push(frame.device, frame.queue, bytemuck::bytes_of(&params));
                Some(frame.bind(&frame.pipelines.generator_layout, uniforms, u, &[]))
                    .map(|g| (g, &frame.pipelines.generator))
            }
            None => media.get(index).copied().flatten().map(|view| {
                let params = MediaParams::fit(view.size, frame.size);
                let u = uniforms.push(frame.device, frame.queue, bytemuck::bytes_of(&params));
                let group = frame.bind(
                    &frame.pipelines.media_layout,
                    uniforms,
                    u,
                    &[
                        wgpu::BindingResource::TextureView(view.view),
                        wgpu::BindingResource::Sampler(&frame.pipelines.sampler),
                    ],
                );
                (group, &frame.pipelines.media)
            }),
        };
        if let Some((group, pipeline)) = drawn {
            let mut rp = pass(encoder, &state.work.current().view, true);
            rp.set_pipeline(pipeline);
            rp.set_bind_group(0, &group, &[]);
            rp.draw(0..3, 0..1);
        }
        frame.run_chain(encoder, uniforms, &clip.effects, None, &mut state.work);
    }

    // Transition into `state.mixed`.
    let params = TransitionParams {
        progress: progress.filter(|_| transitioning).unwrap_or(1.0),
        _pad: [0.0; 3],
    };
    let u = uniforms.push(frame.device, frame.queue, bytemuck::bytes_of(&params));
    let group = frame.bind(
        &frame.pipelines.two_textures_layout,
        uniforms,
        u,
        &[
            wgpu::BindingResource::TextureView(&state.outgoing.view),
            wgpu::BindingResource::TextureView(&state.work.current().view),
        ],
    );
    let mut rp = pass(encoder, &state.mixed.view, true);
    rp.set_pipeline(&frame.pipelines.transition);
    rp.set_bind_group(0, &group, &[]);
    rp.draw(0..3, 0..1);
    drop(rp);

    // Layer effects read `mixed` and write into `work`.
    let ran = frame.run_chain(
        encoder,
        uniforms,
        &layer.effects,
        Some(&state.mixed.view),
        &mut state.work,
    );
    if level > 0.0 {
        state.image = Some(if ran {
            LayerImage::Work
        } else {
            LayerImage::Mixed
        });
        let result = if ran {
            &state.work.current().view
        } else {
            &state.mixed.view
        };
        frame.blend(encoder, uniforms, dest, result, layer.blend, level);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        BlendParams, EffectParams, GeneratorParams, MediaParams, OutputParams, TransitionParams,
    };

    /// Validates `source` and checks that its `params` uniform has the size of `T`.
    fn check<T>(source: &str, uniform: &str) {
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
            .find(|(_, var)| var.name.as_deref() == Some(uniform))
            .expect("shader has the uniform")
            .1;
        let wgsl_size = module.types[params.ty].inner.size(module.to_ctx()) as usize;
        assert_eq!(wgsl_size, size_of::<T>(), "uniform size mismatch");
        assert!(size_of::<T>() as u64 <= super::UNIFORM_SIZE);
    }

    #[test]
    fn media_fit_letterboxes_and_pillarboxes() {
        assert_eq!(
            MediaParams::fit((1920, 1080), (1920, 1080)).scale,
            [1.0, 1.0]
        );
        // Wider image: full width, reduced height.
        assert_eq!(MediaParams::fit((200, 50), (100, 50)).scale, [1.0, 0.5]);
        // Taller image: full height, reduced width.
        assert_eq!(MediaParams::fit((50, 50), (100, 50)).scale, [0.5, 1.0]);
    }

    #[test]
    fn shaders_are_valid_and_match_uniform_structs() {
        check::<GeneratorParams>(include_str!("shaders/generator.wgsl"), "params");
        check::<BlendParams>(include_str!("shaders/blend.wgsl"), "params");
        check::<OutputParams>(include_str!("shaders/output.wgsl"), "params");
        check::<MediaParams>(include_str!("shaders/media.wgsl"), "params");
        check::<TransitionParams>(include_str!("shaders/transition.wgsl"), "params");
        let effect = super::EffectLibrary::builtin();
        let blur = effect.get("builtin:blur").unwrap();
        check::<EffectParams>(&blur.fragment, "verso");
    }
}
