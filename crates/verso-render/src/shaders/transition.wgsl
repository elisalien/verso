// Crossfades from the outgoing clip to the incoming one. Premultiplied RGBA.

struct Params {
    progress: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var outgoing_tex: texture_2d<f32>;
@group(0) @binding(2) var incoming_tex: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let a = textureLoad(outgoing_tex, pixel, 0);
    let b = textureLoad(incoming_tex, pixel, 0);
    return mix(a, b, clamp(params.progress, 0.0, 1.0));
}
