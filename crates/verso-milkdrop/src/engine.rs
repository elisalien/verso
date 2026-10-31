//! CPU side of a running MilkDrop preset: runs the per-frame, per-vertex,
//! shape and wave equations and produces everything the GPU passes need.
//! Port of Butterchurn's renderer, equation runner, shapes, waves, borders
//! and motion vectors (without preset blending).
//!
//! Geometry is in OpenGL normalized device coordinates (y up), as in
//! MilkDrop and Butterchurn; the renderer flips it for wgpu.

use crate::audio::{AudioState, NUM_SAMPS};
use crate::eel::{Env, Program, Rng, Symbols};
use crate::preset::{
    PRESET_DEFAULTS, Preset, SHAPE_DEFAULTS, Values, WAVE_DEFAULTS, with_defaults,
};

/// Warp mesh resolution (cells), as in Butterchurn.
pub const MESH_WIDTH: usize = 48;
pub const MESH_HEIGHT: usize = 36;
/// Composite pass mesh resolution (cells).
pub const COMP_WIDTH: usize = 32;
pub const COMP_HEIGHT: usize = 24;

const MAX_SAMPLES: usize = NUM_SAMPS;
const MAX_SHAPE_INSTANCES: f64 = 1024.0;

/// One vertex of the sprite geometry drawn after the warp pass.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Blend {
    /// `src_alpha, one_minus_src_alpha`
    Alpha,
    /// `src_alpha, one`
    Additive,
}

/// A run of triangles sharing blending and texturing.
#[derive(Debug, Clone, PartialEq)]
pub struct Draw {
    pub blend: Blend,
    /// Multiply the color by the previous frame sampled at `uv`.
    pub textured: bool,
    pub vertices: std::ops::Range<u32>,
}

/// Uniform values shared by the warp and comp shaders.
#[derive(Debug, Clone, Default)]
pub struct ShaderValues {
    pub time: f32,
    pub frame: f32,
    pub fps: f32,
    pub decay: f32,
    pub bass: f32,
    pub mid: f32,
    pub treb: f32,
    pub bass_att: f32,
    pub mid_att: f32,
    pub treb_att: f32,
    pub q: [f32; 32],
    pub rand_preset: [f32; 4],
    pub rand_frame: [f32; 4],
    pub gamma_adj: f32,
    pub echo_zoom: f32,
    pub echo_alpha: f32,
    pub echo_orient: f32,
    pub invert: bool,
    pub brighten: bool,
    pub darken: bool,
    pub solarize: bool,
    pub fshader: f32,
    /// Sample the feedback buffer with repeat instead of clamp.
    pub wrap: bool,
    pub blur_min: [f32; 3],
    pub blur_max: [f32; 3],
    pub b1ed: f32,
}

/// Everything the renderer needs for one frame.
#[derive(Debug, Clone, Default)]
pub struct FrameOutput {
    /// Warp texture coordinates, `(MESH_WIDTH + 1) * (MESH_HEIGHT + 1)`,
    /// row by row from the top.
    pub warp_uvs: Vec<[f32; 2]>,
    /// Sprite geometry (motion vectors, shapes, waves, borders) in order.
    pub vertices: Vec<Vertex>,
    pub draws: Vec<Draw>,
    /// Composite mesh colors, `(COMP_WIDTH + 1) * (COMP_HEIGHT + 1)`.
    pub comp_colors: Vec<[f32; 4]>,
    pub shader: ShaderValues,
}

/// Highest blur level a shader samples (0 to 3).
pub fn highest_blur(shader: &str) -> u32 {
    if shader.contains("sampler_blur3") {
        3
    } else if shader.contains("sampler_blur2") {
        2
    } else if shader.contains("sampler_blur1") {
        1
    } else {
        0
    }
}

/// Blur passes a preset needs.
pub fn blur_passes(preset: &Preset) -> u32 {
    let warp = preset.warp.trim();
    let comp = preset.comp.trim();
    let warp_blur = if warp.is_empty() { 0 } else { highest_blur(warp) };
    let comp_blur = if comp.is_empty() { 0 } else { highest_blur(comp) };
    warp_blur.max(comp_blur)
}

const GLOBAL_NAMES: [&str; 15] = [
    "frame", "time", "fps", "bass", "bass_att", "mid", "mid_att", "treb", "treb_att", "meshx",
    "meshy", "aspectx", "aspecty", "pixelsx", "pixelsy",
];

#[derive(Debug, Clone, Copy, Default)]
struct Globals {
    values: [f64; 15],
}

/// Variables and code of one equation namespace (preset, a shape or a wave).
struct Scope {
    symbols: Symbols,
    vars: Vec<f64>,
    megabuf: Vec<f64>,
    globals: [usize; 15],
    q: [usize; 32],
    t: [usize; 8],
    regs: [usize; 100],
    base: Vec<(usize, f64)>,
}

impl Scope {
    fn new(base: &Values) -> Scope {
        let mut symbols = Symbols::default();
        let globals = GLOBAL_NAMES.map(|n| symbols.slot(n));
        let q = std::array::from_fn(|i| symbols.slot(&format!("q{}", i + 1)));
        let t = std::array::from_fn(|i| symbols.slot(&format!("t{}", i + 1)));
        let regs = std::array::from_fn(|i| symbols.slot(&format!("reg{i:02}")));
        let base = base.iter().map(|(k, v)| (symbols.slot(k), *v)).collect();
        Scope {
            symbols,
            vars: Vec::new(),
            megabuf: Vec::new(),
            globals,
            q,
            t,
            regs,
            base,
        }
    }

    fn compile(&mut self, source: &str) -> Result<Program, String> {
        Program::compile(source, &mut self.symbols)
    }

    /// Sizes the variables once every program is compiled.
    fn finish(&mut self) {
        self.vars = vec![0.0; self.symbols.len()];
        self.reset_base();
    }

    fn slot(&mut self, name: &str) -> usize {
        let slot = self.symbols.slot(name);
        if slot >= self.vars.len() {
            self.vars.resize(slot + 1, 0.0);
        }
        slot
    }

    fn get(&self, name: &str) -> f64 {
        self.symbols
            .get(name)
            .and_then(|s| self.vars.get(s).copied())
            .unwrap_or(0.0)
    }

    fn reset_base(&mut self) {
        for &(slot, v) in &self.base {
            self.vars[slot] = v;
        }
    }

    fn set_globals(&mut self, g: &Globals) {
        for (slot, v) in self.globals.iter().zip(g.values) {
            self.vars[*slot] = v;
        }
    }

    fn set_q(&mut self, q: &[f64; 32]) {
        for (slot, v) in self.q.iter().zip(q) {
            self.vars[*slot] = *v;
        }
    }

    fn q(&self) -> [f64; 32] {
        self.q.map(|s| self.vars[s])
    }

    fn set_t(&mut self, t: &[f64; 8]) {
        for (slot, v) in self.t.iter().zip(t) {
            self.vars[*slot] = *v;
        }
    }

    fn t(&self) -> [f64; 8] {
        self.t.map(|s| self.vars[s])
    }

    fn load_regs(&mut self, regs: &[f64; 100]) {
        for (slot, v) in self.regs.iter().zip(regs) {
            self.vars[*slot] = *v;
        }
    }

    fn store_regs(&self, regs: &mut [f64; 100]) {
        for (slot, v) in self.regs.iter().zip(regs.iter_mut()) {
            *v = self.vars[*slot];
        }
    }

    fn run(&mut self, program: &Program, gmegabuf: &mut Vec<f64>, rng: &mut Rng) {
        program.run(&mut Env {
            vars: &mut self.vars,
            megabuf: &mut self.megabuf,
            gmegabuf,
            rng,
        });
    }
}

struct ShapeState {
    scope: Scope,
    frame: Program,
    base: Values,
    t_init: [f64; 8],
    /// Slots of the values reset before each instance.
    instance_base: Vec<(usize, f64)>,
    instance: usize,
}

struct WaveState {
    scope: Scope,
    frame: Program,
    point: Program,
    base: Values,
    t_init: [f64; 8],
    /// sample, value1, value2, x, y, r, g, b, a
    point_slots: [usize; 9],
}

/// A preset running on one feedback buffer.
pub struct Engine {
    base: Values,
    frame_scope: Scope,
    pixel_vars: Vec<f64>,
    frame_eqs: Program,
    pixel_eqs: Program,
    /// x, y, rad, ang, then the ten warp values reset per vertex.
    pixel_slots: [usize; 14],
    shapes: Vec<ShapeState>,
    waves: Vec<WaveState>,
    q_init: [f64; 32],
    q_after: [f64; 32],
    regs: [f64; 100],
    gmegabuf: Vec<f64>,
    rng: Rng,
    rand_start: [f64; 4],
    rand_preset: [f32; 4],
    texsize: [f64; 2],
    aspect: [f64; 2],
    warp_uvs: Vec<[f32; 2]>,
}

const WARP_NAMES: [&str; 10] = [
    "zoom", "zoomexp", "rot", "warp", "cx", "cy", "dx", "dy", "sx", "sy",
];

const SHAPE_INSTANCE_NAMES: [&str; 21] = [
    "x", "y", "rad", "ang", "r", "g", "b", "a", "r2", "g2", "b2", "a2", "border_r", "border_g",
    "border_b", "border_a", "thickoutline", "textured", "tex_zoom", "tex_ang", "additive",
];

impl Engine {
    /// Compiles `preset` for a `width` × `height` feedback buffer.
    pub fn new(preset: &Preset, width: u32, height: u32, seed: u64) -> Result<Engine, String> {
        let mut rng = Rng::new(seed);
        let base = with_defaults(PRESET_DEFAULTS, &preset.base_vals);
        let texsize = [width.max(1) as f64, height.max(1) as f64];
        let aspect = [
            if texsize[1] > texsize[0] { texsize[0] / texsize[1] } else { 1.0 },
            if texsize[0] > texsize[1] { texsize[1] / texsize[0] } else { 1.0 },
        ];

        let mut frame_scope = Scope::new(&base);
        let init_eqs = frame_scope
            .compile(&preset.init_eqs_eel)
            .map_err(|e| format!("per-frame init: {e}"))?;
        let frame_eqs = frame_scope
            .compile(&preset.frame_eqs_eel)
            .map_err(|e| format!("per-frame: {e}"))?;
        let pixel_eqs = frame_scope
            .compile(&preset.pixel_eqs_eel)
            .map_err(|e| format!("per-vertex: {e}"))?;
        let pixel_slots = {
            let mut names = vec!["x", "y", "rad", "ang"];
            names.extend(WARP_NAMES);
            let slots: Vec<usize> = names.iter().map(|n| frame_scope.symbols.slot(n)).collect();
            slots.try_into().unwrap()
        };
        frame_scope.finish();

        let rand_start = std::array::from_fn(|_| rng.next_f64());
        let rand_preset = std::array::from_fn(|_| rng.next_f64() as f32);
        let mut engine = Engine {
            base,
            frame_scope,
            pixel_vars: Vec::new(),
            frame_eqs,
            pixel_eqs,
            pixel_slots,
            shapes: Vec::new(),
            waves: Vec::new(),
            q_init: [0.0; 32],
            q_after: [0.0; 32],
            regs: [0.0; 100],
            gmegabuf: Vec::new(),
            rng,
            rand_start,
            rand_preset,
            texsize,
            aspect,
            warp_uvs: vec![[0.0; 2]; (MESH_WIDTH + 1) * (MESH_HEIGHT + 1)],
        };

        let globals = engine.globals(0, 0.0, 45.0, &AudioLevels::default());
        // Init equations, then one run of the frame equations so user
        // variables and q values exist before shapes and waves initialize.
        {
            let Engine {
                frame_scope,
                gmegabuf,
                rng,
                ..
            } = &mut engine;
            frame_scope.set_globals(&globals);
            frame_scope.run(&init_eqs, gmegabuf, rng);
        }
        engine.q_init = engine.frame_scope.q();
        engine.frame_scope.store_regs(&mut engine.regs);
        engine.run_frame_eqs(&globals);

        for (i, shape) in preset.shapes.iter().enumerate() {
            let base = with_defaults(SHAPE_DEFAULTS, &shape.base_vals);
            if base["enabled"] == 0.0 {
                continue;
            }
            let mut scope = Scope::new(&base);
            let init = scope
                .compile(&shape.init_eqs_eel)
                .map_err(|e| format!("shape {i} init: {e}"))?;
            let frame = scope
                .compile(&shape.frame_eqs_eel)
                .map_err(|e| format!("shape {i}: {e}"))?;
            let instance = scope.slot("instance");
            scope.finish();
            let instance_base = SHAPE_INSTANCE_NAMES
                .iter()
                .map(|n| (scope.symbols.get(n).unwrap(), base[*n]))
                .collect();
            scope.set_globals(&globals);
            scope.set_q(&engine.q_after);
            scope.load_regs(&engine.regs);
            scope.run(&init, &mut engine.gmegabuf, &mut engine.rng);
            scope.store_regs(&mut engine.regs);
            scope.reset_base();
            let t_init = scope.t();
            engine.shapes.push(ShapeState {
                scope,
                frame,
                base,
                t_init,
                instance_base,
                instance,
            });
        }

        for (i, wave) in preset.waves.iter().enumerate() {
            let base = with_defaults(WAVE_DEFAULTS, &wave.base_vals);
            if base["enabled"] == 0.0 {
                continue;
            }
            let mut scope = Scope::new(&base);
            let init = scope
                .compile(&wave.init_eqs_eel)
                .map_err(|e| format!("wave {i} init: {e}"))?;
            let frame = scope
                .compile(&wave.frame_eqs_eel)
                .map_err(|e| format!("wave {i}: {e}"))?;
            let point = scope
                .compile(&wave.point_eqs_eel)
                .map_err(|e| format!("wave {i} per-point: {e}"))?;
            let point_slots = ["sample", "value1", "value2", "x", "y", "r", "g", "b", "a"]
                .map(|n| scope.symbols.slot(n));
            scope.finish();
            scope.set_globals(&globals);
            scope.set_q(&engine.q_after);
            scope.load_regs(&engine.regs);
            scope.run(&init, &mut engine.gmegabuf, &mut engine.rng);
            scope.store_regs(&mut engine.regs);
            scope.reset_base();
            let t_init = scope.t();
            engine.waves.push(WaveState {
                scope,
                frame,
                point,
                base,
                t_init,
                point_slots,
            });
        }
        Ok(engine)
    }

    fn globals(&self, frame: u64, time: f64, fps: f64, audio: &AudioLevels) -> Globals {
        Globals {
            values: [
                frame as f64,
                time,
                fps,
                audio.bass,
                audio.bass_att,
                audio.mid,
                audio.mid_att,
                audio.treb,
                audio.treb_att,
                MESH_WIDTH as f64,
                MESH_HEIGHT as f64,
                1.0 / self.aspect[0],
                1.0 / self.aspect[1],
                self.texsize[0],
                self.texsize[1],
            ],
        }
    }

    fn run_frame_eqs(&mut self, globals: &Globals) {
        let Engine {
            frame_scope,
            frame_eqs,
            gmegabuf,
            rng,
            q_init,
            regs,
            ..
        } = self;
        frame_scope.reset_base();
        frame_scope.set_q(q_init);
        frame_scope.load_regs(regs);
        frame_scope.set_globals(globals);
        frame_scope.run(frame_eqs, gmegabuf, rng);
        frame_scope.store_regs(regs);
        self.q_after = self.frame_scope.q();
    }

    /// Advances one frame. `time` is in seconds since the preset started.
    pub fn frame(&mut self, audio: &AudioState, time: f64, fps: f64, frame: u64) -> FrameOutput {
        let levels = AudioLevels::from(audio);
        let globals = self.globals(frame, time, fps, &levels);
        self.run_frame_eqs(&globals);
        self.run_pixel_eqs(time);

        let rand_frame = std::array::from_fn(|_| self.rng.next_f64() as f32);
        let f = |name: &str| self.frame_scope.get(name);
        let mut out = FrameOutput {
            warp_uvs: self.warp_uvs.clone(),
            ..FrameOutput::default()
        };
        let (blur_min, blur_max) = blur_values(&f);
        out.shader = ShaderValues {
            time: time as f32,
            frame: frame as f32,
            fps: fps as f32,
            decay: f("decay") as f32,
            bass: f("bass") as f32,
            mid: f("mid") as f32,
            treb: f("treb") as f32,
            bass_att: f("bass_att") as f32,
            mid_att: f("mid_att") as f32,
            treb_att: f("treb_att") as f32,
            q: self.q_after.map(|q| q as f32),
            rand_preset: self.rand_preset,
            rand_frame,
            gamma_adj: f("gammaadj") as f32,
            echo_zoom: f("echo_zoom") as f32,
            echo_alpha: f("echo_alpha") as f32,
            echo_orient: f("echo_orient") as f32,
            invert: f("invert") != 0.0,
            brighten: f("brighten") != 0.0,
            darken: f("darken") != 0.0,
            solarize: f("solarize") != 0.0,
            fshader: f("fshader") as f32,
            wrap: f("wrap") != 0.0,
            blur_min,
            blur_max,
            b1ed: f("b1ed") as f32,
        };
        out.comp_colors = self.comp_colors(time);
        let darken_center = f("darken_center") != 0.0;
        let color = |p: &str| {
            [
                f(&format!("{p}_r")) as f32,
                f(&format!("{p}_g")) as f32,
                f(&format!("{p}_b")) as f32,
                f(&format!("{p}_a")) as f32,
            ]
        };
        let (ob, ib) = (color("ob"), color("ib"));
        let (ob_size, ib_size) = (f("ob_size") as f32, f("ib_size") as f32);

        let mut sprites = Sprites::new(self.texsize);
        self.motion_vectors(&mut sprites);
        self.shapes(&globals, &mut sprites);
        self.waves(&globals, audio, &mut sprites);
        self.basic_waveform(audio, &mut sprites);
        if darken_center {
            sprites.darken_center(self.aspect[1] as f32);
        }
        sprites.border(ob, ob_size, 0.0);
        sprites.border(ib, ib_size, ob_size);
        out.vertices = sprites.vertices;
        out.draws = sprites.draws;
        out
    }

    fn run_pixel_eqs(&mut self, time: f64) {
        let f = |name: &str| self.frame_scope.get(name);
        let warp_time = time * f("warpanimspeed");
        let warp_scale_inv = 1.0 / f("warpscale");
        let warpf0 = 11.68 + 4.0 * (warp_time * 1.413 + 10.0).cos();
        let warpf1 = 8.77 + 3.0 * (warp_time * 1.113 + 7.0).cos();
        let warpf2 = 10.54 + 3.0 * (warp_time * 1.233 + 3.0).cos();
        let warpf3 = 11.49 + 4.0 * (warp_time * 0.933 + 5.0).cos();
        let [aspectx, aspecty] = self.aspect;
        let frame_warp: [f64; 10] = WARP_NAMES.map(f);
        let run_eqs = !self.pixel_eqs.is_empty();

        self.pixel_vars.clone_from(&self.frame_scope.vars);
        let Engine {
            pixel_vars,
            pixel_eqs,
            pixel_slots,
            frame_scope,
            gmegabuf,
            rng,
            warp_uvs,
            ..
        } = self;
        let mut warp = frame_warp;
        let mut i = 0;
        for iz in 0..=MESH_HEIGHT {
            for ix in 0..=MESH_WIDTH {
                let x = ix as f64 / MESH_WIDTH as f64 * 2.0 - 1.0;
                let y = iz as f64 / MESH_HEIGHT as f64 * 2.0 - 1.0;
                let rad = (x * x * aspectx * aspectx + y * y * aspecty * aspecty).sqrt();
                if run_eqs {
                    let ang = if iz == MESH_HEIGHT / 2 && ix == MESH_WIDTH / 2 {
                        0.0
                    } else {
                        (y * aspecty).atan2(x * aspectx)
                    };
                    pixel_vars[pixel_slots[0]] = x * 0.5 * aspectx + 0.5;
                    pixel_vars[pixel_slots[1]] = y * -0.5 * aspecty + 0.5;
                    pixel_vars[pixel_slots[2]] = rad;
                    pixel_vars[pixel_slots[3]] = ang;
                    for (k, v) in frame_warp.iter().enumerate() {
                        pixel_vars[pixel_slots[4 + k]] = *v;
                    }
                    pixel_eqs.run(&mut Env {
                        vars: pixel_vars,
                        megabuf: &mut frame_scope.megabuf,
                        gmegabuf,
                        rng,
                    });
                    for (k, w) in warp.iter_mut().enumerate() {
                        *w = pixel_vars[pixel_slots[4 + k]];
                    }
                }
                let [w, zoom, zoom_exp, rot, cx, cy, dx, dy, sx, sy] = [
                    warp[3], warp[0], warp[1], warp[2], warp[4], warp[5], warp[6], warp[7],
                    warp[8], warp[9],
                ];
                let zoom2 = zoom.powf(zoom_exp.powf(rad * 2.0 - 1.0));
                let zoom2_inv = 1.0 / zoom2;
                let mut u = x * 0.5 * aspectx * zoom2_inv + 0.5;
                let mut v = -y * 0.5 * aspecty * zoom2_inv + 0.5;
                u = (u - cx) / sx + cx;
                v = (v - cy) / sy + cy;
                if w != 0.0 {
                    u += w * 0.0035 * (warp_time * 0.333 + warp_scale_inv * (x * warpf0 - y * warpf3)).sin();
                    v += w * 0.0035 * (warp_time * 0.375 - warp_scale_inv * (x * warpf2 + y * warpf1)).cos();
                    u += w * 0.0035 * (warp_time * 0.753 - warp_scale_inv * (x * warpf1 - y * warpf2)).cos();
                    v += w * 0.0035 * (warp_time * 0.825 + warp_scale_inv * (x * warpf0 + y * warpf3)).sin();
                }
                let (u2, v2) = (u - cx, v - cy);
                let (sin_rot, cos_rot) = rot.sin_cos();
                u = u2 * cos_rot - v2 * sin_rot + cx;
                v = u2 * sin_rot + v2 * cos_rot + cy;
                u -= dx;
                v -= dy;
                u = (u - 0.5) / aspectx + 0.5;
                v = (v - 0.5) / aspecty + 0.5;
                warp_uvs[i] = [u as f32, v as f32];
                i += 1;
            }
        }
        if run_eqs {
            for (slot, reg) in frame_scope.regs.iter().zip(self.regs.iter_mut()) {
                *reg = self.pixel_vars[*slot];
            }
        }
    }

    fn comp_colors(&self, time: f64) -> Vec<[f32; 4]> {
        let mut hue = [0.0f64; 12];
        for i in 0..4 {
            let fi = i as f64;
            hue[i * 3] = 0.6 + 0.3 * (time * 30.0 * 0.0143 + 3.0 + fi * 21.0 + self.rand_start[3]).sin();
            hue[i * 3 + 1] = 0.6 + 0.3 * (time * 30.0 * 0.0107 + 1.0 + fi * 13.0 + self.rand_start[1]).sin();
            hue[i * 3 + 2] = 0.6 + 0.3 * (time * 30.0 * 0.0129 + 6.0 + fi * 9.0 + self.rand_start[2]).sin();
            let max = hue[i * 3].max(hue[i * 3 + 1]).max(hue[i * 3 + 2]);
            for k in 0..3 {
                hue[i * 3 + k] = 0.5 + 0.5 * hue[i * 3 + k] / max;
            }
        }
        let mut colors = Vec::with_capacity((COMP_WIDTH + 1) * (COMP_HEIGHT + 1));
        for j in 0..=COMP_HEIGHT {
            for i in 0..=COMP_WIDTH {
                let x = i as f64 / COMP_WIDTH as f64;
                let y = j as f64 / COMP_HEIGHT as f64;
                let c = |k: usize| {
                    (hue[k] * x * y
                        + hue[3 + k] * (1.0 - x) * y
                        + hue[6 + k] * x * (1.0 - y)
                        + hue[9 + k] * (1.0 - x) * (1.0 - y)) as f32
                };
                colors.push([c(0), c(1), c(2), 1.0]);
            }
        }
        colors
    }

    fn motion_vectors(&self, sprites: &mut Sprites) {
        let f = |name: &str| self.frame_scope.get(name);
        let mv_a = if f("bmotionvectorson") == 0.0 { 0.0 } else { f("mv_a") };
        let mut nx = f("mv_x").floor();
        let mut ny = f("mv_y").floor();
        if !(mv_a > 0.001 && nx > 0.0 && ny > 0.0) {
            return;
        }
        let mut dx = f("mv_x") - nx;
        let mut dy = f("mv_y") - ny;
        if nx > 64.0 {
            nx = 64.0;
            dx = 0.0;
        }
        if ny > 48.0 {
            ny = 48.0;
            dy = 0.0;
        }
        let (dx2, dy2, len_mult) = (f("mv_dx"), f("mv_dy"), f("mv_l"));
        let min_len = 1.0 / self.texsize[0];
        let color = [f("mv_r") as f32, f("mv_g") as f32, f("mv_b") as f32, mv_a as f32];
        let mut segments = Vec::new();
        for j in 0..ny as usize {
            let fy = (j as f64 + 0.25) / (ny + dy + 0.25 - 1.0) - dy2;
            if !(fy > 0.0001 && fy < 0.9999) {
                continue;
            }
            for i in 0..nx as usize {
                let fx = (i as f64 + 0.25) / (nx + dx + 0.25 - 1.0) + dx2;
                if !(fx > 0.0001 && fx < 0.9999) {
                    continue;
                }
                let [fx2, fy2] = self.motion_dir(fx, fy);
                let mut dxi = (fx2 - fx) * len_mult;
                let mut dyi = (fy2 - fy) * len_mult;
                let fdist = (dxi * dxi + dyi * dyi).sqrt();
                if fdist < min_len && fdist > 0.000_000_01 {
                    let s = min_len / fdist;
                    dxi *= s;
                    dyi *= s;
                } else {
                    // Butterchurn assigns dxi twice here.
                    dxi = min_len;
                }
                let p1 = [(2.0 * fx - 1.0) as f32, (2.0 * fy - 1.0) as f32];
                let p2 = [(2.0 * (fx + dxi) - 1.0) as f32, (2.0 * (fy + dyi) - 1.0) as f32];
                segments.push((p1, p2));
            }
        }
        for (p1, p2) in segments {
            sprites.line(&[p1, p2], &[color, color], 1.0, [0.0, 0.0]);
        }
        sprites.flush(Blend::Alpha, false);
    }

    fn motion_dir(&self, fx: f64, fy: f64) -> [f64; 2] {
        let y0 = (fy * MESH_HEIGHT as f64).floor();
        let dy = fy * MESH_HEIGHT as f64 - y0;
        let x0 = (fx * MESH_WIDTH as f64).floor();
        let dx = fx * MESH_WIDTH as f64 - x0;
        let (x0, y0) = (x0 as usize, y0 as usize);
        let at = |x: usize, y: usize| {
            let i = (y.min(MESH_HEIGHT) * (MESH_WIDTH + 1) + x.min(MESH_WIDTH)).min(self.warp_uvs.len() - 1);
            let uv = self.warp_uvs[i];
            [uv[0] as f64, uv[1] as f64]
        };
        let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
        let mix = |k: usize| {
            a[k] * (1.0 - dx) * (1.0 - dy) + b[k] * dx * (1.0 - dy) + c[k] * (1.0 - dx) * dy + d[k] * dx * dy
        };
        [mix(0), 1.0 - mix(1)]
    }

    fn shapes(&mut self, globals: &Globals, sprites: &mut Sprites) {
        let aspecty = self.aspect[1];
        let Engine {
            shapes,
            q_after,
            regs,
            gmegabuf,
            rng,
            ..
        } = self;
        for shape in shapes.iter_mut() {
            let scope = &mut shape.scope;
            scope.reset_base();
            scope.set_globals(globals);
            scope.load_regs(regs);
            let has_eqs = !shape.frame.is_empty();
            if !has_eqs {
                scope.set_q(q_after);
                scope.set_t(&shape.t_init);
            }
            let num_inst = shape.base["num_inst"].clamp(1.0, MAX_SHAPE_INSTANCES) as usize;
            for j in 0..num_inst {
                scope.vars[shape.instance] = j as f64;
                for &(slot, v) in &shape.instance_base {
                    scope.vars[slot] = v;
                }
                if has_eqs {
                    scope.set_q(q_after);
                    scope.set_t(&shape.t_init);
                    scope.run(&shape.frame, gmegabuf, rng);
                }
                let g = |name: &str| scope.get(name);
                let sides = g("sides").clamp(3.0, 100.0).floor() as usize;
                let rad = g("rad");
                let ang = g("ang");
                let x = g("x") * 2.0 - 1.0;
                let y = g("y") * -2.0 + 1.0;
                let center = [g("r") as f32, g("g") as f32, g("b") as f32, g("a") as f32];
                let rim = [g("r2") as f32, g("g2") as f32, g("b2") as f32, g("a2") as f32];
                let border = [
                    g("border_r") as f32,
                    g("border_g") as f32,
                    g("border_b") as f32,
                    g("border_a") as f32,
                ];
                let textured = g("textured").abs() >= 1.0;
                let thick = g("thickoutline").abs() >= 1.0;
                let additive = g("additive").abs() >= 1.0;
                let (tex_zoom, tex_ang) = (g("tex_zoom"), g("tex_ang"));
                let quarter_pi = std::f64::consts::FRAC_PI_4;
                let rim_points: Vec<([f32; 2], [f32; 2])> = (0..=sides)
                    .map(|k| {
                        let p = k as f64 / sides as f64 * std::f64::consts::TAU;
                        let a = p + ang + quarter_pi;
                        let pos = [(x + rad * a.cos() * aspecty) as f32, (y + rad * a.sin()) as f32];
                        let ta = p + tex_ang + quarter_pi;
                        let uv = [
                            (0.5 + 0.5 * ta.cos() / tex_zoom * aspecty) as f32,
                            (0.5 + 0.5 * ta.sin() / tex_zoom) as f32,
                        ];
                        (pos, uv)
                    })
                    .collect();
                let center_v = Vertex {
                    pos: [x as f32, y as f32],
                    color: center,
                    uv: [0.5, 0.5],
                };
                for k in 0..sides {
                    let (p1, uv1) = rim_points[k];
                    let (p2, uv2) = rim_points[k + 1];
                    sprites.vertices.push(center_v);
                    sprites.vertices.push(Vertex { pos: p1, color: rim, uv: uv1 });
                    sprites.vertices.push(Vertex { pos: p2, color: rim, uv: uv2 });
                }
                sprites.flush(
                    if additive { Blend::Additive } else { Blend::Alpha },
                    textured,
                );
                if border[3] > 0.0 {
                    let points: Vec<[f32; 2]> = rim_points.iter().map(|(p, _)| *p).collect();
                    let colors = vec![border; points.len()];
                    sprites.thick_line(&points, &colors, thick);
                    sprites.flush(Blend::Alpha, false);
                }
            }
        }
    }

    fn waves(&mut self, globals: &Globals, audio: &AudioState, sprites: &mut Sprites) {
        let inv_aspect = [1.0 / self.aspect[0], 1.0 / self.aspect[1]];
        let wave_scale = self.base["wave_scale"];
        let big = self.texsize[0] >= 1024.0;
        let Engine {
            waves,
            q_after,
            regs,
            gmegabuf,
            rng,
            ..
        } = self;
        for wave in waves.iter_mut() {
            let scope = &mut wave.scope;
            scope.reset_base();
            scope.set_q(q_after);
            scope.set_t(&wave.t_init);
            scope.set_globals(globals);
            scope.load_regs(regs);
            scope.run(&wave.frame, gmegabuf, rng);

            let g = |name: &str| scope.get(name);
            let mut samples = (g("samples").min(MAX_SAMPLES as f64)).floor();
            let sep = g("sep").floor();
            let scaling = g("scaling");
            let spectrum = g("spectrum") != 0.0;
            let smoothing = g("smoothing");
            let usedots = wave.base["usedots"] != 0.0;
            let frame_color = [g("r"), g("g"), g("b"), g("a")];
            samples -= sep;
            if !(samples >= 2.0 || (usedots && samples >= 1.0)) {
                continue;
            }
            let n = samples as usize;
            let scale = if spectrum { 0.15 } else { 0.004 } * scaling * wave_scale;
            let (left, right) = if spectrum {
                (&audio.freq_l, &audio.freq_r)
            } else {
                (&audio.time_l, &audio.time_r)
            };
            let max = MAX_SAMPLES as f64;
            let j0 = if spectrum { 0.0 } else { ((max - samples) / 2.0 - sep / 2.0).floor() };
            let j1 = if spectrum { 0.0 } else { ((max - samples) / 2.0 + sep / 2.0).floor() };
            let t = if spectrum { (max - sep) / samples } else { 1.0 };
            let mix1 = (smoothing * 0.98).max(0.0).sqrt();
            let mix2 = 1.0 - mix1;
            let at = |arr: &Vec<f32>, idx: f64| -> f64 {
                if idx < 0.0 {
                    0.0
                } else {
                    arr.get(idx as usize).copied().unwrap_or(0.0) as f64
                }
            };
            let mut pl = vec![0.0f64; n];
            let mut pr = vec![0.0f64; n];
            pl[0] = at(left, j0);
            pr[0] = at(right, j1);
            for j in 1..n {
                let l = at(left, (j as f64 * t + j0).floor());
                let r = at(right, (j as f64 * t + j1).floor());
                pl[j] = l * mix2 + pl[j - 1] * mix1;
                pr[j] = r * mix2 + pr[j - 1] * mix1;
            }
            for j in (0..n.saturating_sub(1)).rev() {
                pl[j] = pl[j] * mix2 + pl[j + 1] * mix1;
                pr[j] = pr[j] * mix2 + pr[j + 1] * mix1;
            }
            let has_points = !wave.point.is_empty();
            let [s_sample, s_v1, s_v2, s_x, s_y, s_r, s_g, s_b, s_a] = wave.point_slots;
            let mut points = Vec::with_capacity(n);
            let mut colors = Vec::with_capacity(n);
            for j in 0..n {
                let (v1, v2) = (pl[j] * scale, pr[j] * scale);
                let vars = &mut scope.vars;
                vars[s_sample] = if n > 1 { j as f64 / (n - 1) as f64 } else { 0.0 };
                vars[s_v1] = v1;
                vars[s_v2] = v2;
                vars[s_x] = 0.5 + v1;
                vars[s_y] = 0.5 + v2;
                vars[s_r] = frame_color[0];
                vars[s_g] = frame_color[1];
                vars[s_b] = frame_color[2];
                vars[s_a] = frame_color[3];
                if has_points {
                    scope.run(&wave.point, gmegabuf, rng);
                }
                let vars = &scope.vars;
                points.push([
                    ((vars[s_x] * 2.0 - 1.0) * inv_aspect[0]) as f32,
                    ((vars[s_y] * -2.0 + 1.0) * inv_aspect[1]) as f32,
                ]);
                colors.push([vars[s_r] as f32, vars[s_g] as f32, vars[s_b] as f32, vars[s_a] as f32]);
            }
            scope.store_regs(regs);
            let dots = scope.get("usedots") != 0.0;
            let thick = scope.get("thick") != 0.0;
            let blend = if scope.get("additive") != 0.0 {
                Blend::Additive
            } else {
                Blend::Alpha
            };
            if dots {
                let size = if thick { 2.0 } else { 1.0 } + if big { 1.0 } else { 0.0 };
                for (p, c) in points.iter().zip(&colors) {
                    sprites.dot(*p, *c, size);
                }
            } else {
                let (points, colors) = smooth_wave(&points, Some(&colors));
                sprites.thick_line(&points, &colors.unwrap(), thick);
            }
            sprites.flush(blend, false);
        }
    }

    fn basic_waveform(&self, audio: &AudioState, sprites: &mut Sprites) {
        let f = |name: &str| self.frame_scope.get(name);
        let mut alpha = f("wave_a");
        let vol = (f("bass") + f("mid") + f("treb")) / 3.0;
        if !(vol > -0.01 && alpha > 0.001) {
            return;
        }
        let process = |time: &[f32]| -> Vec<f64> {
            let scale = f("wave_scale") / 128.0;
            let smooth = f("wave_smoothing");
            let smooth2 = scale * (1.0 - smooth);
            let mut out = Vec::with_capacity(time.len());
            out.push(time[0] as f64 * scale);
            for i in 1..time.len() {
                let prev = out[i - 1];
                out.push(time[i] as f64 * smooth2 + prev * smooth);
            }
            out
        };
        let wave_l = process(&audio.time_l);
        let wave_r = process(&audio.time_r);
        let len = wave_l.len();
        let l = |i: usize| wave_l.get(i).copied().unwrap_or(0.0);
        let r = |i: usize| wave_r.get(i).copied().unwrap_or(0.0);
        let mode = (f("wave_mode").floor() as i64).rem_euclid(8);
        let pos_x = f("wave_x") * 2.0 - 1.0;
        let pos_y = f("wave_y") * 2.0 - 1.0;
        let [aspectx, aspecty] = self.aspect;
        let time = f("time");
        let mut param = f("wave_mystery");
        if matches!(mode, 0 | 1 | 4) && !(-1.0..=1.0).contains(&param) {
            param = param * 0.5 + 0.5;
            param -= param.floor();
            param = param.abs() * 2.0 - 1.0;
        }
        let tex_alpha = |small: f64, mid: f64, large: f64| {
            if self.texsize[0] < 1024.0 {
                small
            } else if self.texsize[0] < 2048.0 {
                mid
            } else {
                large
            }
        };
        let mod_alpha = |alpha: f64| {
            let mut alpha = alpha;
            if f("modwavealphabyvolume") > 0.0 {
                let diff = f("modwavealphaend") - f("modwavealphastart");
                alpha *= (vol - f("modwavealphastart")) / diff;
            }
            alpha.clamp(0.0, 1.0)
        };
        let mut positions: Vec<[f64; 2]> = Vec::new();
        let mut positions2: Vec<[f64; 2]> = Vec::new();
        match mode {
            0 => {
                alpha = mod_alpha(alpha);
                let num = len / 2 + 1;
                let inv = 1.0 / (num - 1) as f64;
                let offset = (len - num) / 2;
                for i in 0..num - 1 {
                    let mut rad = 0.5 + 0.4 * r(i + offset) + param;
                    let ang = i as f64 * inv * std::f64::consts::TAU + time * 0.2;
                    if (i as f64) < num as f64 / 10.0 {
                        let mut mix = i as f64 / (num as f64 * 0.1);
                        mix = 0.5 - 0.5 * (mix * std::f64::consts::PI).cos();
                        let rad2 = 0.5 + 0.4 * r(i + num + offset) + param;
                        rad = (1.0 - mix) * rad2 + rad * mix;
                    }
                    positions.push([rad * ang.cos() * aspecty + pos_x, rad * ang.sin() * aspectx + pos_y]);
                }
                positions.push(positions[0]);
            }
            1 => {
                alpha = mod_alpha(alpha * 1.25);
                for i in 0..len / 2 {
                    let rad = 0.53 + 0.43 * r(i) + param;
                    let ang = l(i + 32) * 0.5 * std::f64::consts::PI + time * 2.3;
                    positions.push([rad * ang.cos() * aspecty + pos_x, rad * ang.sin() * aspectx + pos_y]);
                }
            }
            2 | 3 => {
                alpha *= if mode == 2 {
                    tex_alpha(0.09, 0.11, 0.13)
                } else {
                    tex_alpha(0.15, 0.22, 0.33) * 1.3 * f("treb") * f("treb")
                };
                alpha = mod_alpha(alpha);
                for i in 0..len {
                    positions.push([r(i) * aspecty + pos_x, l((i + 32) % len) * aspectx + pos_y]);
                }
            }
            4 => {
                alpha = mod_alpha(alpha);
                let num = len.min((self.texsize[0] / 3.0) as usize);
                let inv = 1.0 / num as f64;
                let offset = (len - num) / 2;
                let w1 = 0.45 + 0.5 * (param * 0.5 + 0.5);
                let w2 = 1.0 - w1;
                for i in 0..num {
                    let mut x = 2.0 * i as f64 * inv + (pos_x - 1.0) + r((i + 25 + offset) % len) * 0.44;
                    let mut y = l(i + offset) * 0.47 + pos_y;
                    if i > 1 {
                        x = x * w2 + w1 * (positions[i - 1][0] * 2.0 - positions[i - 2][0]);
                        y = y * w2 + w1 * (positions[i - 1][1] * 2.0 - positions[i - 2][1]);
                    }
                    positions.push([x, y]);
                }
            }
            5 => {
                alpha = mod_alpha(alpha * tex_alpha(0.09, 0.11, 0.13));
                let (sin_rot, cos_rot) = (time * 0.3).sin_cos();
                for i in 0..len {
                    let ioff = (i + 32) % len;
                    let x0 = r(i) * l(ioff) + l(i) * r(ioff);
                    let y0 = r(i) * r(i) - l(ioff) * l(ioff);
                    positions.push([
                        (x0 * cos_rot - y0 * sin_rot) * (aspecty + pos_x),
                        (x0 * sin_rot + y0 * cos_rot) * (aspectx + pos_y),
                    ]);
                }
            }
            _ => {
                alpha = mod_alpha(alpha);
                let num = (len / 2).min((self.texsize[0] / 3.0) as usize);
                let offset = (len - num) / 2;
                let ang = std::f64::consts::FRAC_PI_2 * param;
                let (mut dx, mut dy) = (ang.cos(), ang.sin());
                let perp = ang + std::f64::consts::FRAC_PI_2;
                let mut edgex = [pos_x * perp.cos() - dx * 3.0, pos_x * perp.cos() + dx * 3.0];
                let mut edgey = [pos_x * perp.sin() - dy * 3.0, pos_x * perp.sin() + dy * 3.0];
                for i in 0..2 {
                    for j in 0..4 {
                        let o = 1 - i;
                        let t = match j {
                            0 if edgex[i] > 1.1 => Some((1.1 - edgex[o]) / (edgex[i] - edgex[o])),
                            1 if edgex[i] < -1.1 => Some((-1.1 - edgex[o]) / (edgex[i] - edgex[o])),
                            2 if edgey[i] > 1.1 => Some((1.1 - edgey[o]) / (edgey[i] - edgey[o])),
                            3 if edgey[i] < -1.1 => Some((-1.1 - edgey[o]) / (edgey[i] - edgey[o])),
                            _ => None,
                        };
                        if let Some(t) = t {
                            let dxi = edgex[i] - edgex[o];
                            let dyi = edgey[i] - edgey[o];
                            edgex[i] = edgex[o] + dxi * t;
                            edgey[i] = edgey[o] + dyi * t;
                        }
                    }
                }
                dx = (edgex[1] - edgex[0]) / num as f64;
                dy = (edgey[1] - edgey[0]) / num as f64;
                let ang2 = dy.atan2(dx) + std::f64::consts::FRAC_PI_2;
                let (perp_dx, perp_dy) = (ang2.cos(), ang2.sin());
                if mode == 6 {
                    for i in 0..num {
                        let s = l(i + offset);
                        positions.push([
                            edgex[0] + dx * i as f64 + perp_dx * 0.25 * s,
                            edgey[0] + dy * i as f64 + perp_dy * 0.25 * s,
                        ]);
                    }
                } else {
                    let sep = (pos_y * 0.5 + 0.5).powi(2);
                    for i in 0..num {
                        let s = l(i + offset);
                        positions.push([
                            edgex[0] + dx * i as f64 + perp_dx * (0.25 * s + sep),
                            edgey[0] + dy * i as f64 + perp_dy * (0.25 * s + sep),
                        ]);
                        let s = r(i + offset);
                        positions2.push([
                            edgex[0] + dx * i as f64 + perp_dx * (0.25 * s - sep),
                            edgey[0] + dy * i as f64 + perp_dy * (0.25 * s - sep),
                        ]);
                    }
                }
            }
        }
        if positions.len() < 2 {
            return;
        }
        let mut c = [
            f("wave_r").clamp(0.0, 1.0),
            f("wave_g").clamp(0.0, 1.0),
            f("wave_b").clamp(0.0, 1.0),
        ];
        if f("wave_brighten") != 0.0 {
            let max = c[0].max(c[1]).max(c[2]);
            if max > 0.01 {
                c = c.map(|v| v / max);
            }
        }
        let color = [c[0] as f32, c[1] as f32, c[2] as f32, alpha as f32];
        let dots = f("wave_dots") != 0.0;
        let thick = f("wave_thick") != 0.0 || dots;
        let blend = if f("additivewave") != 0.0 {
            Blend::Additive
        } else {
            Blend::Alpha
        };
        for line in [positions, positions2] {
            if line.len() < 2 {
                continue;
            }
            let flipped: Vec<[f32; 2]> = line.iter().map(|p| [p[0] as f32, -p[1] as f32]).collect();
            let (smoothed, _) = smooth_wave(&flipped, None);
            if dots {
                for offset in thick_offsets(thick) {
                    for p in &smoothed {
                        sprites.dot([p[0] + offset[0] * sprites.px[0], p[1] + offset[1] * sprites.px[1]], color, 1.0);
                    }
                }
            } else {
                let colors = vec![color; smoothed.len()];
                sprites.thick_line(&smoothed, &colors, thick);
            }
        }
        sprites.flush(blend, false);
    }
}

#[derive(Debug, Clone, Copy)]
struct AudioLevels {
    bass: f64,
    bass_att: f64,
    mid: f64,
    mid_att: f64,
    treb: f64,
    treb_att: f64,
}

impl Default for AudioLevels {
    fn default() -> Self {
        AudioLevels {
            bass: 1.0,
            bass_att: 1.0,
            mid: 1.0,
            mid_att: 1.0,
            treb: 1.0,
            treb_att: 1.0,
        }
    }
}

impl From<&AudioState> for AudioLevels {
    fn from(a: &AudioState) -> Self {
        AudioLevels {
            bass: a.bass as f64,
            bass_att: a.bass_att as f64,
            mid: a.mid as f64,
            mid_att: a.mid_att as f64,
            treb: a.treb as f64,
            treb_att: a.treb_att as f64,
        }
    }
}

/// Blur ranges, with Butterchurn's handling of ranges that are too narrow.
fn blur_values(f: &dyn Fn(&str) -> f64) -> ([f32; 3], [f32; 3]) {
    let mut min = [f("b1n"), f("b2n"), f("b3n")];
    let mut max = [f("b1x"), f("b2x"), f("b3x")];
    let min_dist = 0.1;
    for i in 0..3 {
        if i > 0 {
            max[i] = max[i - 1].min(max[i]);
            min[i] = min[i - 1].max(min[i]);
        }
        if max[i] - min[i] < min_dist {
            let avg = (min[i] + max[i]) * 0.5;
            min[i] = avg - min_dist * 0.5;
            // Butterchurn subtracts here too, which collapses the range.
            max[i] = avg - min_dist * 0.5;
        }
    }
    (min.map(|v| v as f32), max.map(|v| v as f32))
}

/// MilkDrop's waveform smoothing: inserts a cubic midpoint between samples.
fn smooth_wave(points: &[[f32; 2]], colors: Option<&[[f32; 4]]>) -> (Vec<[f32; 2]>, Option<Vec<[f32; 4]>>) {
    let n = points.len();
    if n < 2 {
        return (points.to_vec(), colors.map(<[_]>::to_vec));
    }
    let (c1, c2, c3, c4) = (-0.15f32, 1.15f32, 1.15f32, -0.15f32);
    let inv_sum = 1.0 / (c1 + c2 + c3 + c4);
    let mut out = Vec::with_capacity(n * 2 - 1);
    let mut out_colors = colors.map(|_| Vec::with_capacity(n * 2 - 1));
    let mut below = 0;
    let mut above2 = 1;
    for i in 0..n - 1 {
        let above = above2;
        above2 = (i + 2).min(n - 1);
        out.push(points[i]);
        let mid = |k: usize| {
            (c1 * points[below][k] + c2 * points[i][k] + c3 * points[above][k] + c4 * points[above2][k]) * inv_sum
        };
        out.push([mid(0), mid(1)]);
        if let (Some(oc), Some(c)) = (out_colors.as_mut(), colors) {
            oc.push(c[i]);
            oc.push(c[i]);
        }
        below = i;
    }
    out.push(points[n - 1]);
    if let (Some(oc), Some(c)) = (out_colors.as_mut(), colors) {
        oc.push(c[n - 1]);
    }
    (out, out_colors)
}

/// Pixel offsets of the copies MilkDrop draws for thick lines.
fn thick_offsets(thick: bool) -> &'static [[f32; 2]] {
    if thick {
        &[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]]
    } else {
        &[[0.0, 0.0]]
    }
}

/// Collects sprite triangles and groups them into draws.
struct Sprites {
    vertices: Vec<Vertex>,
    draws: Vec<Draw>,
    flushed: usize,
    /// Size of one pixel in NDC units, x and y.
    px: [f32; 2],
}

impl Sprites {
    fn new(texsize: [f64; 2]) -> Self {
        Sprites {
            vertices: Vec::new(),
            draws: Vec::new(),
            flushed: 0,
            px: [(2.0 / texsize[0]) as f32, (2.0 / texsize[1]) as f32],
        }
    }

    /// Ends the current draw.
    fn flush(&mut self, blend: Blend, textured: bool) {
        let end = self.vertices.len();
        if end > self.flushed {
            self.draws.push(Draw {
                blend,
                textured,
                vertices: self.flushed as u32..end as u32,
            });
        }
        self.flushed = end;
    }

    fn quad(&mut self, corners: [[f32; 2]; 4], colors: [[f32; 4]; 4]) {
        for i in [0, 1, 2, 2, 1, 3] {
            self.vertices.push(Vertex {
                pos: corners[i],
                color: colors[i],
                uv: [0.0, 0.0],
            });
        }
    }

    /// A line strip `width` pixels wide, shifted by `offset` pixels.
    fn line(&mut self, points: &[[f32; 2]], colors: &[[f32; 4]], width: f32, offset: [f32; 2]) {
        let [px, py] = self.px;
        let half = width * 0.5;
        for i in 0..points.len().saturating_sub(1) {
            let (a, b) = (points[i], points[i + 1]);
            if !(a[0].is_finite() && a[1].is_finite() && b[0].is_finite() && b[1].is_finite()) {
                continue;
            }
            // Perpendicular in pixel space.
            let (dx, dy) = ((b[0] - a[0]) / px, (b[1] - a[1]) / py);
            let len = (dx * dx + dy * dy).sqrt();
            let (nx, ny) = if len > 0.0 { (-dy / len, dx / len) } else { (0.0, 1.0) };
            let (ox, oy) = (nx * half * px, ny * half * py);
            let (sx, sy) = (offset[0] * px, offset[1] * py);
            self.quad(
                [
                    [a[0] + ox + sx, a[1] + oy + sy],
                    [a[0] - ox + sx, a[1] - oy + sy],
                    [b[0] + ox + sx, b[1] + oy + sy],
                    [b[0] - ox + sx, b[1] - oy + sy],
                ],
                [colors[i], colors[i], colors[i + 1], colors[i + 1]],
            );
        }
    }

    fn thick_line(&mut self, points: &[[f32; 2]], colors: &[[f32; 4]], thick: bool) {
        for offset in thick_offsets(thick) {
            self.line(points, colors, 1.0, *offset);
        }
    }

    fn dot(&mut self, p: [f32; 2], color: [f32; 4], size: f32) {
        if !(p[0].is_finite() && p[1].is_finite()) {
            return;
        }
        let (hx, hy) = (size * 0.5 * self.px[0], size * 0.5 * self.px[1]);
        self.quad(
            [
                [p[0] - hx, p[1] - hy],
                [p[0] + hx, p[1] - hy],
                [p[0] - hx, p[1] + hy],
                [p[0] + hx, p[1] + hy],
            ],
            [color; 4],
        );
    }

    fn darken_center(&mut self, aspecty: f32) {
        let h = 0.05;
        let rim = [[-h * aspecty, 0.0], [0.0, -h], [h * aspecty, 0.0], [0.0, h], [-h * aspecty, 0.0]];
        let center = Vertex {
            pos: [0.0, 0.0],
            color: [0.0, 0.0, 0.0, 3.0 / 32.0],
            uv: [0.0; 2],
        };
        for k in 0..4 {
            self.vertices.push(center);
            for p in [rim[k], rim[k + 1]] {
                self.vertices.push(Vertex {
                    pos: p,
                    color: [0.0; 4],
                    uv: [0.0; 2],
                });
            }
        }
        self.flush(Blend::Alpha, false);
    }

    /// Frame border `size` wide, inside a border `prev_size` wide.
    fn border(&mut self, color: [f32; 4], size: f32, prev_size: f32) {
        if !(size > 0.0 && color[3] > 0.0) {
            return;
        }
        let prev = prev_size / 2.0 * 2.0;
        let edge = (size / 2.0 + prev_size / 2.0) * 2.0;
        let mut tri = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| {
            for p in [a, b, c] {
                self.vertices.push(Vertex {
                    pos: p,
                    color,
                    uv: [0.0; 2],
                });
            }
        };
        // Left, right, top and bottom bands, as in Butterchurn.
        let (p1, p2, p3, p4) = ([-1.0 + prev, -1.0 + edge], [-1.0 + prev, 1.0 - edge], [-1.0 + edge, 1.0 - edge], [-1.0 + edge, -1.0 + edge]);
        tri(p4, p2, p1);
        tri(p4, p3, p2);
        let (p1, p2, p3, p4) = ([1.0 - prev, -1.0 + edge], [1.0 - prev, 1.0 - edge], [1.0 - edge, 1.0 - edge], [1.0 - edge, -1.0 + edge]);
        tri(p1, p2, p4);
        tri(p2, p3, p4);
        let (p1, p2, p3, p4) = ([-1.0 + prev, -1.0 + prev], [-1.0 + prev, edge - 1.0], [1.0 - prev, edge - 1.0], [1.0 - prev, -1.0 + prev]);
        tri(p4, p2, p1);
        tri(p4, p3, p2);
        let (p1, p2, p3, p4) = ([-1.0 + prev, 1.0 - prev], [-1.0 + prev, 1.0 - edge], [1.0 - prev, 1.0 - edge], [1.0 - prev, 1.0 - prev]);
        tri(p1, p2, p4);
        tri(p2, p3, p4);
        self.flush(Blend::Alpha, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn preset(frame: &str, pixel: &str) -> Preset {
        Preset {
            frame_eqs_eel: frame.into(),
            pixel_eqs_eel: pixel.into(),
            ..Preset::default()
        }
    }

    #[test]
    fn identity_warp_maps_mesh_to_itself() {
        let p = preset("zoom = 1; rot = 0; warp = 0;", "");
        let mut engine = Engine::new(&p, 64, 64, 1).unwrap();
        let audio = AudioState::new(44100.0);
        let out = engine.frame(&audio, 0.0, 60.0, 1);
        let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-5 && (a[1] - b[1]).abs() < 1e-5;
        // Top-left mesh vertex samples the top of the GL texture (v = 1).
        assert!(close(out.warp_uvs[0], [0.0, 1.0]), "{:?}", out.warp_uvs[0]);
        let last = out.warp_uvs.len() - 1;
        assert!(close(out.warp_uvs[last], [1.0, 0.0]), "{:?}", out.warp_uvs[last]);
    }

    #[test]
    fn pixel_eqs_change_the_mesh_and_shapes_draw() {
        let mut p = preset("", "rot = rad * 0.1;");
        let mut shape = crate::preset::Shape::default();
        shape.base_vals.insert("enabled".into(), 1.0);
        shape.base_vals.insert("num_inst".into(), 3.0);
        shape.frame_eqs_eel = "x = 0.2 + instance * 0.3; sides = 6;".into();
        p.shapes.push(shape);
        let mut engine = Engine::new(&p, 64, 64, 1).unwrap();
        let audio = AudioState::new(44100.0);
        let out = engine.frame(&audio, 0.5, 60.0, 1);
        assert!(out.warp_uvs[0] != [0.0, 1.0]);
        // Three hexagon fans (6 triangles each) plus their borders.
        let fans: Vec<_> = out.draws.iter().filter(|d| d.vertices.len() == 18).collect();
        assert_eq!(fans.len(), 3);
    }

    #[test]
    fn user_vars_persist_and_base_vals_reset() {
        let mut p = preset("counter = counter + 1; zoom = zoom + 1; q1 = counter;", "");
        p.base_vals.insert("zoom".into(), 1.0);
        let mut engine = Engine::new(&p, 64, 64, 1).unwrap();
        let audio = AudioState::new(44100.0);
        for frame in 1..=3 {
            engine.frame(&audio, frame as f64 / 60.0, 60.0, frame);
        }
        // One run at init plus three frames.
        assert_eq!(engine.frame_scope.get("counter"), 4.0);
        assert_eq!(engine.frame_scope.get("zoom"), 2.0);
        assert_eq!(engine.q_after[0], 4.0);
    }
}
