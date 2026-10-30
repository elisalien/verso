// Built-in generative sources. Output is premultiplied RGBA.

struct Params {
    time: f32,
    speed: f32,
    kind: u32,
    aspect: f32,
    color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    // Single triangle covering the viewport.
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

const TAU: f32 = 6.28318530718;

fn plasma(p: vec2<f32>, t: f32) -> f32 {
    var v = sin(p.x * 6.0 + t);
    v += sin((p.y * 5.0 + t) * 0.8);
    v += sin((p.x * 4.0 + p.y * 4.0 + t) * 0.7);
    let c = p + 0.5 * vec2<f32>(sin(t * 0.3), cos(t * 0.5));
    v += sin(sqrt(100.0 * dot(c, c) + 1.0) + t);
    return 0.5 + 0.5 * sin(v * 0.5 * TAU / 2.0);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = params.time * params.speed;
    // Centered coordinates with square pixels.
    let p = (in.uv - 0.5) * vec2<f32>(params.aspect, 1.0);

    var value: f32;
    switch params.kind {
        case 0u: {
            value = plasma(p, t);
        }
        case 1u: {
            value = 0.5 + 0.5 * sin(in.uv.x * TAU + t);
        }
        case 2u: {
            value = step(0.5, fract(p.x * 8.0 + p.y * 2.0 - t));
        }
        case 3u: {
            value = 0.5 + 0.5 * sin(length(p) * 30.0 - t * 4.0);
        }
        // Solid color.
        default: {
            value = 1.0;
        }
    }

    let rgb = params.color.rgb * value;
    let a = params.color.a;
    return vec4<f32>(rgb * a, a);
}
