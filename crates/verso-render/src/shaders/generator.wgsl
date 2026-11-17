// Built-in generative sources. Output is premultiplied RGBA.

struct Params {
    time: f32,
    speed: f32,
    kind: u32,
    aspect: f32,
    color: vec4<f32>,
    // Color where the pattern is dark.
    color2: vec4<f32>,
    // Offset x and y, angle in radians, scale.
    transform: vec4<f32>,
    // Values specific to the kind; their defaults give the original look.
    shape: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: Params;

const TAU: f32 = 6.28318530718;

fn plasma(p: vec2<f32>, t: f32, complexity: f32, warp: f32, bands: f32) -> f32 {
    let q = p * complexity;
    var v = sin(q.x * 6.0 + t);
    v += sin((q.y * 5.0 + t) * 0.8);
    v += sin((q.x * 4.0 + q.y * 4.0 + t) * 0.7);
    let c = q + warp * vec2<f32>(sin(t * 0.3), cos(t * 0.5));
    v += sin(sqrt(100.0 * dot(c, c) + 1.0) + t);
    return 0.5 + 0.5 * sin(v * 0.5 * TAU / 2.0 * bands);
}

// Hash without sin, which loses precision on some GPUs and shows blocks.
fn hash2(p: vec2<f32>) -> vec2<f32> {
    var q = fract(vec3<f32>(p.x, p.y, p.x) * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.xx + q.yz) * q.zy);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i).x;
    let b = hash2(i + vec2<f32>(1.0, 0.0)).x;
    let c = hash2(i + vec2<f32>(0.0, 1.0)).x;
    let d = hash2(i + vec2<f32>(1.0, 1.0)).x;
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(p: vec2<f32>, t: f32, octaves: i32, gain: f32, frequency: f32) -> f32 {
    var v = 0.0;
    var amplitude = 0.5;
    var q = p * frequency + vec2<f32>(t * 0.3, t * 0.2);
    for (var i = 0; i < min(octaves, 8); i++) {
        v += amplitude * value_noise(q);
        q = q * 2.03 + vec2<f32>(17.0, 9.0);
        amplitude *= gain;
    }
    return v;
}

// Soft or hard bands from a 0..1 wave: `thickness` is the share above the
// threshold; `hardness` 0 with `thickness` 0.5 keeps the wave as is.
fn band(v: f32, thickness: f32, hardness: f32) -> f32 {
    let half_width = mix(0.5, 0.005, hardness);
    return clamp((v - (1.0 - thickness)) / (2.0 * half_width) + 0.5, 0.0, 1.0);
}

fn gradient(p: vec2<f32>, t: f32, repeats: f32, radial: f32) -> f32 {
    // Across the frame like the uv coordinate, or out from the center.
    let x = p.x / params.aspect + 0.5;
    let coord = mix(x, length(p), radial);
    return 0.5 + 0.5 * sin(coord * TAU * repeats + t);
}

fn stripes(p: vec2<f32>, t: f32, count: f32, slant: f32, width: f32, softness: f32) -> f32 {
    let f = fract(p.x * count + p.y * slant - t);
    if softness <= 0.0 {
        return step(width, f);
    }
    // Distance to the middle of the lit part, around the period.
    let center = (width + 1.0) * 0.5;
    let half_width = (1.0 - width) * 0.5;
    let d = abs(fract(f - center + 0.5) - 0.5);
    return 1.0 - smoothstep(half_width - softness, half_width + softness, d);
}

fn rings(p: vec2<f32>, t: f32, count: f32, thickness: f32, hardness: f32) -> f32 {
    let v = 0.5 + 0.5 * sin(length(p) * count - t * 4.0);
    if hardness <= 0.0 && thickness == 0.5 {
        return v;
    }
    return band(v, thickness, hardness);
}

fn voronoi(p: vec2<f32>, t: f32, cells: f32, jitter: f32, edge: f32) -> f32 {
    let q = p * cells;
    let cell = floor(q);
    var nearest = 8.0;
    var second = 8.0;
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let n = cell + vec2<f32>(f32(x), f32(y));
            let point = n + 0.5 + jitter * sin(t + TAU * hash2(n));
            let d = length(q - point);
            second = min(second, max(nearest, d));
            nearest = min(nearest, d);
        }
    }
    var v = clamp(nearest, 0.0, 1.0);
    if edge > 0.0 {
        // Bright lines where two cells meet.
        v = max(v, 1.0 - smoothstep(0.0, edge, second - nearest));
    }
    return v;
}

fn checker(p: vec2<f32>, t: f32, squares: f32, spin: f32) -> f32 {
    let a = t * spin;
    let r = vec2<f32>(p.x * cos(a) - p.y * sin(a), p.x * sin(a) + p.y * cos(a));
    let c = floor(r * squares);
    return f32((i32(c.x) + i32(c.y)) & 1);
}

fn tunnel(p: vec2<f32>, t: f32, depth_scale: f32, ring_count: f32, ribs: f32, twist: f32) -> f32 {
    let r = max(length(p), 1e-3);
    let depth = depth_scale / r + t;
    let angle = atan2(p.y, p.x) / TAU + twist * depth;
    let ring = 0.5 + 0.5 * sin(depth * TAU * ring_count);
    let rib = 0.5 + 0.5 * sin(angle * TAU * ribs);
    // Darker in the distance.
    return mix(ring, ring * rib, 0.5) * smoothstep(0.0, 0.4, r);
}

fn starfield(p: vec2<f32>, t: f32, density: f32, star_size: f32, layers: i32) -> f32 {
    var v = 0.0;
    let count = clamp(layers, 1, 8);
    let step_z = 1.0 / f32(count);
    for (var layer = 0; layer < count; layer++) {
        // Each layer zooms in and wraps around, so stars fly out from the center.
        let z = fract(t * 0.15 + f32(layer) * step_z);
        let scale = mix(20.0, 0.5, z);
        let q = p * scale + f32(layer) * 13.7;
        let cell = floor(q);
        let star = hash2(cell);
        let d = length(fract(q) - star);
        let size = (0.02 + 0.06 * star.y) * star_size;
        v += smoothstep(size, 0.0, d) * z * step(1.0 - density, star.x);
    }
    return clamp(v, 0.0, 1.0);
}

fn waves(p: vec2<f32>, t: f32, frequency: f32, spread: f32, contrast: f32) -> f32 {
    let a = vec2<f32>(-spread + 0.1 * sin(t * 0.4), 0.0);
    let b = vec2<f32>(spread, 0.1 * cos(t * 0.3));
    let v = sin(length(p - a) * frequency - t * 3.0) + sin(length(p - b) * frequency - t * 3.0);
    return clamp(0.5 + 0.25 * contrast * v, 0.0, 1.0);
}

fn spiral(p: vec2<f32>, t: f32, arms: f32, tightness: f32, hardness: f32) -> f32 {
    let r = length(p);
    let angle = atan2(p.y, p.x);
    let v = 0.5 + 0.5 * sin(angle * arms + log(max(r, 1e-3)) * tightness - t * 2.0);
    if hardness <= 0.0 {
        return v;
    }
    return band(v, 0.5, hardness);
}

fn metaballs(p: vec2<f32>, t: f32, count: i32, size: f32, orbit: f32, glow: f32) -> f32 {
    var field = 0.0;
    for (var i = 0; i < min(count, 12); i++) {
        let k = f32(i);
        let center = orbit * vec2<f32>(sin(t * (0.5 + k * 0.13) + k * 2.0), cos(t * (0.4 + k * 0.17) + k));
        let d = p - center;
        field += 0.012 * size / max(dot(d, d), 1e-4);
    }
    return smoothstep(0.9, 1.1, field) * (1.0 - glow) + clamp(field * glow, 0.0, glow);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let t = params.time * params.speed;
    // Centered coordinates with square pixels, moved (y up), rotated and
    // zoomed.
    let centered = (in.uv - 0.5) * vec2<f32>(params.aspect, 1.0);
    let moved = centered - vec2<f32>(params.transform.x, -params.transform.y);
    let c = cos(params.transform.z);
    let s = sin(params.transform.z);
    let p = vec2<f32>(moved.x * c + moved.y * s, moved.y * c - moved.x * s) / params.transform.w;
    let k = params.shape;

    var value: f32;
    switch params.kind {
        case 0u: {
            value = plasma(p, t, k.x, k.y, k.z);
        }
        case 1u: {
            value = gradient(p, t, k.x, k.y);
        }
        case 2u: {
            value = stripes(p, t, k.x, k.y, k.z, k.w);
        }
        case 3u: {
            value = rings(p, t, k.x, k.y, k.z);
        }
        case 4u: {
            value = fbm(p, t, i32(round(k.x)), k.y, k.z);
        }
        case 5u: {
            value = voronoi(p, t, k.x, k.y, k.z);
        }
        case 6u: {
            value = checker(p, t, k.x, k.y);
        }
        case 7u: {
            value = tunnel(p, t, k.x, k.y, k.z, k.w);
        }
        case 8u: {
            value = starfield(p, t, k.x, k.y, i32(round(k.z)));
        }
        case 9u: {
            value = waves(p, t, k.x, k.y, k.z);
        }
        case 10u: {
            value = spiral(p, t, k.x, k.y, k.z);
        }
        case 11u: {
            value = metaballs(p, t, i32(round(k.x)), k.y, k.z, k.w);
        }
        // Solid color.
        default: {
            value = 1.0;
        }
    }

    // From the second color to the first, written so that the default
    // opaque black second color gives exactly `color * value`.
    let low = params.color2;
    let rgb = low.rgb + (params.color.rgb - low.rgb) * value;
    let a = params.color.a * (1.0 - (1.0 - low.a) * (1.0 - value));
    return vec4<f32>(rgb * a, a);
}
