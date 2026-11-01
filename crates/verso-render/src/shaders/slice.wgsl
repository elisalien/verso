// Draws one slice of the composition on a screen, with its color correction.

struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    // Brightness, contrast, opacity, source gain.
    @location(2) adjust: vec4<f32>,
    @location(3) rgb: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) adjust: vec4<f32>,
    @location(2) rgb: vec4<f32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.adjust = in.adjust;
    out.rgb = in.rgb;
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Layer and group images are premultiplied: their gain fades to black.
    let linear = textureSample(source, source_sampler, in.uv).rgb * in.adjust.w;
    // Corrections apply to the encoded values, as the user sees them.
    var c = pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
    c = (c - 0.5) * (1.0 + in.adjust.y) + 0.5 + in.adjust.x + in.rgb.rgb;
    c = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    // Highlighted slice: diagonal stripes over a tint, visible on any image.
    if in.rgb.w > 0.5 {
        let stripe = step(0.5, fract((in.position.x + in.position.y) / 32.0));
        let tint = vec3<f32>(0.0, 0.75, 1.0);
        c = mix(c, tint, 0.35 + 0.35 * stripe);
        return vec4<f32>(pow(c, vec3<f32>(2.2)), 1.0);
    }
    return vec4<f32>(pow(c, vec3<f32>(2.2)), in.adjust.z);
}
