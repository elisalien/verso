// Feeds another layer into a MilkDrop preset. `convert` turns a Verso image
// (linear, premultiplied) into MilkDrop's display colors; `feed` copies that
// onto the feedback buffer, mixed with the blend constant. Rows stay in
// place: the composite pass reads the feedback buffer upside down from the
// warp mesh, so an unflipped copy shows the right way up.

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var src_sampler: sampler;

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

fn linear_to_srgb(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

@fragment
fn convert(in: VsOut) -> @location(0) vec4<f32> {
    let c = textureSampleLevel(src, src_sampler, in.uv, 0.0);
    return vec4<f32>(linear_to_srgb(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0))), 1.0);
}

@fragment
fn feed(in: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src, src_sampler, in.uv, 0.0);
}
