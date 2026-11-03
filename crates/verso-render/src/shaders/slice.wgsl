// Draws one mapping slice: a warped mesh sampling a rectangle of the
// composition, with soft edges for projector overlaps and a polygon mask.

struct Params {
    // Part of the composition shown: x, y, width, height (0..1).
    input: vec4<f32>,
    // Edge fade widths: left, right, top, bottom (fraction of the slice).
    blend: vec4<f32>,
    gamma: f32,
    opacity: f32,
    // 1 when the mask is inverted.
    invert: u32,
    _pad: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var comp_tex: texture_2d<f32>;
@group(0) @binding(2) var comp_sampler: sampler;
@group(0) @binding(3) var mask_tex: texture_2d<f32>;

struct VsIn {
    // Position on the screen, 0..1 from the top left.
    @location(0) position: vec2<f32>,
    // Point of the slice, 0..1.
    @location(1) uv: vec2<f32>,
}

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) screen: vec2<f32>,
}

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    out.position = vec4<f32>(in.position.x * 2.0 - 1.0, 1.0 - in.position.y * 2.0, 0.0, 1.0);
    out.uv = in.uv;
    out.screen = in.position;
    return out;
}

fn ramp(distance: f32, width: f32) -> f32 {
    if width <= 0.0 {
        return 1.0;
    }
    let t = clamp(distance / width, 0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    return pow(s, 1.0 / max(params.gamma, 0.1));
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let source = params.input.xy + in.uv * params.input.zw;
    let c = textureSample(comp_tex, comp_sampler, source);
    let dims = vec2<f32>(textureDimensions(mask_tex));
    let m = textureLoad(mask_tex, vec2<i32>(clamp(in.screen, vec2<f32>(0.0), vec2<f32>(0.9999)) * dims), 0).r;
    let mask = select(m, 1.0 - m, params.invert == 1u);
    let edge = ramp(in.uv.x, params.blend.x) * ramp(1.0 - in.uv.x, params.blend.y)
        * ramp(in.uv.y, params.blend.z) * ramp(1.0 - in.uv.y, params.blend.w);
    let outside = any(source < vec2<f32>(0.0)) || any(source > vec2<f32>(1.0));
    let level = select(edge * mask * params.opacity, 0.0, outside);
    return vec4<f32>(c.rgb * level, level);
}

// Mask pass: fills the polygon's triangles in white.
@vertex
fn vs_mask(@location(0) position: vec2<f32>) -> @builtin(position) vec4<f32> {
    return vec4<f32>(position.x * 2.0 - 1.0, 1.0 - position.y * 2.0, 0.0, 1.0);
}

@fragment
fn fs_mask() -> @location(0) vec4<f32> {
    return vec4<f32>(1.0);
}
