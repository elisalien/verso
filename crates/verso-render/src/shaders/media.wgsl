// Draws a decoded image or video frame, fitted inside the target with its
// aspect ratio kept. Output is premultiplied RGBA; outside the image is clear.
// HAP Q frames hold scaled YCoCg and are converted to RGB here.

struct Params {
    // Size of the image relative to the target, at most 1 on each axis.
    scale: vec2<f32>,
    // Part of the texture holding the image (padded block textures).
    crop: vec2<f32>,
    // 1 for scaled YCoCg (HAP Q).
    ycocg: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var media_tex: texture_2d<f32>;
@group(0) @binding(2) var media_sampler: sampler;

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

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

// Scaled YCoCg-DXT5 (HAP Q) to linear RGB.
fn ycocg_to_rgb(s: vec4<f32>) -> vec4<f32> {
    let offset = 128.0 / 255.0;
    let scale = s.b * (255.0 / 8.0) + 1.0;
    let co = (s.r - offset) / scale;
    let cg = (s.g - offset) / scale;
    let y = s.a;
    let rgb = clamp(vec3<f32>(y + co - cg, y + cg, y - co - cg), vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(srgb_to_linear(rgb), 1.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let uv = (in.uv - 0.5) / params.scale + 0.5;
    // Sample before branching: texture sampling needs uniform control flow.
    let sampled = textureSample(
        media_tex,
        media_sampler,
        clamp(uv, vec2<f32>(0.0), vec2<f32>(1.0)) * params.crop,
    );
    let c = select(sampled, ycocg_to_rgb(sampled), params.ycocg == 1u);
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), vec4<f32>(c.rgb * c.a, c.a), inside);
}
