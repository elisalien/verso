// Mixes a layer onto the accumulated layers below it.
// Both inputs and the output are premultiplied RGBA.

struct Params {
    mode: u32,
    opacity: f32,
    _pad: vec2<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var base_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_tex: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

fn unpremultiply(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}

// Separable blend function B(cb, cs) on straight colors.
fn blend_fn(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 2u: { return cb * cs; }
        case 3u: { return cb + cs - cb * cs; }
        case 4u: { return max(cb, cs); }
        case 5u: { return min(cb, cs); }
        case 6u: { return abs(cb - cs); }
        default: { return cs; }
    }
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let b = textureLoad(base_tex, pixel, 0);
    let s = textureLoad(layer_tex, pixel, 0) * params.opacity;
    let a = s.a + b.a * (1.0 - s.a);

    switch params.mode {
        // Alpha: source over.
        case 0u: {
            return s + b * (1.0 - s.a);
        }
        // Add: sum the light.
        case 1u: {
            return vec4<f32>(b.rgb + s.rgb, a);
        }
        default: {
            let mixed = blend_fn(params.mode, unpremultiply(b), unpremultiply(s));
            let rgb = (1.0 - s.a) * b.rgb + (1.0 - b.a) * s.rgb + s.a * b.a * mixed;
            return vec4<f32>(rgb, a);
        }
    }
}
