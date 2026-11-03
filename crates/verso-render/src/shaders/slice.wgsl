// Draws one slice of the composition on a screen, with its color correction
// and its soft edge.

struct VertexIn {
    @location(0) position: vec2<f32>,
    @location(1) uv: vec2<f32>,
    // Brightness, contrast, opacity, source gain.
    @location(2) adjust: vec4<f32>,
    @location(3) rgb: vec4<f32>,
    // Index of the slice in `soft.quads`, or -1 without soft edge.
    @location(4) soft: f32,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) adjust: vec4<f32>,
    @location(2) rgb: vec4<f32>,
    @location(3) @interpolate(flat) soft: f32,
}

// Soft-edged slices (see `verso_core::output`, which this mirrors).
struct Soft {
    // Power, luminance, gamma, number of quads.
    params: vec4<f32>,
    // RGB gammas; w unused.
    gamma_rgb: vec4<f32>,
    // Composition size in xy.
    canvas: vec4<f32>,
    // Input quads, two vec4 each: (p0, p1), (p2, p3).
    quads: array<vec4<f32>, 128>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;
@group(0) @binding(2) var<uniform> soft: Soft;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.adjust = in.adjust;
    out.rgb = in.rgb;
    out.soft = in.soft;
    return out;
}

// Distance from `p` to the nearest edge of quad `i`, positive inside.
fn edge_distance(i: u32, p: vec2<f32>) -> f32 {
    let a = soft.quads[2u * i];
    let b = soft.quads[2u * i + 1u];
    var points = array<vec2<f32>, 4>(a.xy, a.zw, b.xy, b.zw);
    var area = 0.0;
    for (var k = 0u; k < 4u; k++) {
        let q0 = points[k];
        let q1 = points[(k + 1u) % 4u];
        area += q0.x * q1.y - q1.x * q0.y;
    }
    let side = select(1.0, -1.0, area < 0.0);
    var distance = 1e30;
    for (var k = 0u; k < 4u; k++) {
        let q0 = points[k];
        let e = points[(k + 1u) % 4u] - q0;
        let length = length(e);
        if length > 0.0 {
            distance = min(distance, side * (e.x * (p.y - q0.y) - e.y * (p.x - q0.x)) / length);
        }
    }
    return distance;
}

// Light factor of the soft edge at this pixel, per channel.
fn soft_edge(own: u32, uv: vec2<f32>) -> vec3<f32> {
    let p = uv * soft.canvas.xy;
    let mine = max(edge_distance(own, p), 0.0);
    var total = 0.0;
    let count = u32(soft.params.w);
    for (var i = 0u; i < count; i++) {
        total += max(edge_distance(i, p), 0.0);
    }
    if total <= mine + 1e-6 {
        return vec3<f32>(1.0);
    }
    let w = clamp(mine / total, 0.0, 1.0);
    let power = soft.params.x;
    let luminance = soft.params.y;
    var s: f32;
    if w < 0.5 {
        s = luminance * pow(2.0 * w, power);
    } else {
        s = 1.0 - (1.0 - luminance) * pow(2.0 * (1.0 - w), power);
    }
    return pow(vec3<f32>(max(s, 0.0)), soft.params.z * soft.gamma_rgb.rgb / 2.0);
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
    var light = pow(c, vec3<f32>(2.2));
    // The soft edge scales the light, after the corrections.
    if in.soft >= 0.0 {
        light *= soft_edge(u32(in.soft), in.uv);
    }
    return vec4<f32>(light, in.adjust.z);
}
