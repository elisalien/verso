// MilkDrop's separable blur: a horizontal pass that also remaps the range
// (scale, bias), then a vertical pass that can darken the edges (b1ed).

struct Params {
    // Source size and its inverse.
    texsize: vec4<f32>,
    // Horizontal: 4 weights; vertical: 2 weights then 2 distances.
    ws: vec4<f32>,
    // Horizontal: 4 distances.
    ds: vec4<f32>,
    wdiv: f32,
    scale: f32,
    bias: f32,
    ed1: f32,
    ed2: f32,
    ed3: f32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var src_sampler: sampler;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src, src_sampler, uv, 0.0).rgb;
}

@fragment
fn horizontal(in: VsOut) -> @location(0) vec4<f32> {
    let dx = params.texsize.z;
    var blur = (tap(in.uv + vec2<f32>(params.ds.x * dx, 0.0)) + tap(in.uv - vec2<f32>(params.ds.x * dx, 0.0))) * params.ws.x
        + (tap(in.uv + vec2<f32>(params.ds.y * dx, 0.0)) + tap(in.uv - vec2<f32>(params.ds.y * dx, 0.0))) * params.ws.y
        + (tap(in.uv + vec2<f32>(params.ds.z * dx, 0.0)) + tap(in.uv - vec2<f32>(params.ds.z * dx, 0.0))) * params.ws.z
        + (tap(in.uv + vec2<f32>(params.ds.w * dx, 0.0)) + tap(in.uv - vec2<f32>(params.ds.w * dx, 0.0))) * params.ws.w;
    blur = blur * params.wdiv * params.scale + params.bias;
    return vec4<f32>(blur, 1.0);
}

@fragment
fn vertical(in: VsOut) -> @location(0) vec4<f32> {
    let dy = params.texsize.w;
    var blur = (tap(in.uv + vec2<f32>(0.0, params.ws.z * dy)) + tap(in.uv - vec2<f32>(0.0, params.ws.z * dy))) * params.ws.x
        + (tap(in.uv + vec2<f32>(0.0, params.ws.w * dy)) + tap(in.uv - vec2<f32>(0.0, params.ws.w * dy))) * params.ws.y;
    blur = blur * params.wdiv;
    var t = min(min(in.uv.x, in.uv.y), 1.0 - max(in.uv.x, in.uv.y));
    t = sqrt(t);
    t = params.ed1 + params.ed2 * clamp(t * params.ed3, 0.0, 1.0);
    return vec4<f32>(blur * t, 1.0);
}
