// Mixes a layer onto the accumulated layers below it.
// Both inputs and the output are premultiplied RGBA.

struct Params {
    mode: u32,
    opacity: f32,
    // Seconds, for animated modes.
    time: f32,
    _pad: f32,
    // Fine parameters of enhanced modes, in the order of their table in
    // `verso_core::blend`.
    values: array<vec4<f32>, 2>,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var base_tex: texture_2d<f32>;
@group(0) @binding(2) var layer_tex: texture_2d<f32>;

fn unpremultiply(c: vec4<f32>) -> vec3<f32> {
    if c.a <= 0.0 {
        return vec3<f32>(0.0);
    }
    return c.rgb / c.a;
}

// Rec. 709 luminance of linear (premultiplied) color, so transparent is 0.
fn luma(c: vec4<f32>) -> f32 {
    return clamp(dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0, 1.0);
}

fn overlay(cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    let low = 2.0 * cb * cs;
    let high = vec3<f32>(1.0) - 2.0 * (vec3<f32>(1.0) - cb) * (vec3<f32>(1.0) - cs);
    return select(high, low, cb <= vec3<f32>(0.5));
}

// Separable blend function B(cb, cs) on straight colors.
fn blend_fn(mode: u32, cb: vec3<f32>, cs: vec3<f32>) -> vec3<f32> {
    switch mode {
        case 2u: { return cb * cs; }
        case 3u: { return cb + cs - cb * cs; }
        case 4u: { return max(cb, cs); }
        case 5u: { return min(cb, cs); }
        case 6u: { return abs(cb - cs); }
        case 11u: { return overlay(cb, cs); }
        default: { return cs; }
    }
}

fn load_clamped(tex: texture_2d<f32>, p: vec2<i32>) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(tex));
    return textureLoad(tex, clamp(p, vec2<i32>(0), size - 1), 0);
}

// Texel at `p`, outside the image clamped (edge 0) or mirrored (edge 1).
fn load_edge(tex: texture_2d<f32>, p: vec2<i32>, edge: u32) -> vec4<f32> {
    if edge == 0u {
        return load_clamped(tex, p);
    }
    let size = vec2<i32>(textureDimensions(tex));
    let period = 2 * size;
    let m = ((p % period) + period) % period;
    let q = select(m, period - 1 - m, m >= size);
    return textureLoad(tex, q, 0);
}

// Bilinear read at pixel position `p` (pixel centers at +0.5). The work
// textures are not filterable, so filtering is done here.
fn sample_tex(tex: texture_2d<f32>, p: vec2<f32>, edge: u32) -> vec4<f32> {
    let q = p - 0.5;
    let i = vec2<i32>(floor(q));
    let f = q - floor(q);
    let a = load_edge(tex, i, edge);
    let b = load_edge(tex, i + vec2<i32>(1, 0), edge);
    let c = load_edge(tex, i + vec2<i32>(0, 1), edge);
    let d = load_edge(tex, i + vec2<i32>(1, 1), edge);
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

fn sample_base(p: vec2<f32>, edge: u32) -> vec4<f32> {
    return sample_tex(base_tex, p, edge);
}

// Channel of a (premultiplied) layer pixel used as a map: 0 luminance,
// 1 red, 2 green, 3 blue, 4 alpha.
fn map_value(c: vec4<f32>, source: u32) -> f32 {
    switch source {
        case 1u: { return clamp(c.r, 0.0, 1.0); }
        case 2u: { return clamp(c.g, 0.0, 1.0); }
        case 3u: { return clamp(c.b, 0.0, 1.0); }
        case 4u: { return clamp(c.a, 0.0, 1.0); }
        default: { return luma(c); }
    }
}

fn choice(v: f32) -> u32 {
    return u32(max(round(v), 0.0));
}

fn direction(degrees: f32) -> vec2<f32> {
    let a = radians(degrees);
    return vec2<f32>(cos(a), sin(a));
}

// 0 below `edge`, 1 above, with a smooth ramp `width` wide around it.
fn soft_step(edge: f32, width: f32, x: f32) -> f32 {
    if width <= 0.0 {
        return select(0.0, 1.0, x >= edge);
    }
    return smoothstep(edge - width * 0.5, edge + width * 0.5, x);
}

// Liquid Displace: the layer is a height field whose slope bends the image
// below like liquid glass; red, green and blue refract by different amounts
// for a prismatic split. Opacity scales the strength.
// values: strength, scale, prism, smoothness | angle, flow, map, edges.
fn liquid_displace(position: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(base_tex));
    let pixel = vec2<i32>(position.xy);
    if params.opacity <= 0.0 {
        return textureLoad(base_tex, pixel, 0);
    }
    let strength = params.values[0].x;
    let scale = params.values[0].y;
    let prism = params.values[0].z;
    let smoothness = params.values[0].w;
    let angle = params.values[1].x;
    let flow = params.values[1].y;
    let source = choice(params.values[1].z);
    let edge = choice(params.values[1].w);
    // Flow ripples where the map is read, animated over time.
    var at = pixel;
    if flow > 0.0 {
        let t = params.time;
        let wave = vec2<f32>(
            sin(position.y / dims.y * 12.0 + t * 1.7),
            cos(position.x / dims.y * 12.0 + t * 1.3),
        );
        at = pixel + vec2<i32>(round(wave * flow * 0.02 * dims.y));
    }
    // Slope from neighbours a resolution-relative distance away.
    let r = i32(max(1.0, round(dims.y / 540.0 * scale)));
    let dx = map_value(load_clamped(layer_tex, at + vec2<i32>(r, 0)), source)
        - map_value(load_clamped(layer_tex, at - vec2<i32>(r, 0)), source);
    let dy = map_value(load_clamped(layer_tex, at + vec2<i32>(0, r)), source)
        - map_value(load_clamped(layer_tex, at - vec2<i32>(0, r)), source);
    // Map per screen height, softly saturated so hard edges stay bounded.
    let slope = vec2<f32>(dx, dy) / (2.0 * f32(r)) * dims.y;
    let normal = slope / (1.0 + length(slope) * smoothness);
    // Full strength shifts by up to ~6% of the height.
    var offset = -normal * 0.015 * dims.y * params.opacity * strength;
    if angle != 0.0 {
        let d = direction(angle);
        offset = vec2<f32>(offset.x * d.x - offset.y * d.y, offset.x * d.y + offset.y * d.x);
    }
    let spread = 0.3 * prism;
    let red = sample_base(position.xy + offset * (1.0 - spread), edge);
    let green = sample_base(position.xy + offset, edge);
    let blue = sample_base(position.xy + offset * (1.0 + spread), edge);
    let a = max(max(red.a, green.a), blue.a);
    return vec4<f32>(red.r, green.g, blue.b, a);
}

// Chroma Shift: the layer's map pushes red and blue of the image below
// apart along a direction; the layer itself is not drawn.
// values: amount, angle, bias, map | edges.
fn chroma_shift(position: vec2<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(base_tex));
    let pixel = vec2<i32>(position.xy);
    let amount = params.values[0].x;
    let bias = params.values[0].z;
    let source = choice(params.values[0].w);
    let edge = choice(params.values[1].x);
    let m = map_value(textureLoad(layer_tex, pixel, 0), source);
    let k = mix(m, 1.0, bias) * amount * dims.y * params.opacity;
    if k == 0.0 {
        return textureLoad(base_tex, pixel, 0);
    }
    let offset = direction(params.values[0].y) * k;
    let red = sample_base(position.xy + offset, edge);
    let green = textureLoad(base_tex, pixel, 0);
    let blue = sample_base(position.xy - offset, edge);
    let a = max(max(red.a, green.a), blue.a);
    return vec4<f32>(red.r, green.g, blue.b, a);
}

// The bright part of a layer pixel above the threshold knee.
fn bright(c: vec4<f32>, threshold: f32, softness: f32) -> vec4<f32> {
    return c * soft_step(threshold, softness, luma(c));
}

// Glow Add: adds the layer's bright parts, spread by a small blur.
// values: threshold, softness, radius, intensity.
fn glow_add(position: vec2<f32>, b: vec4<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(base_tex));
    let threshold = params.values[0].x;
    let softness = params.values[0].y;
    let radius = params.values[0].z * dims.y / 1080.0;
    let intensity = params.values[0].w;
    let pixel = vec2<i32>(position.xy);
    var glow = bright(textureLoad(layer_tex, pixel, 0), threshold, softness);
    if radius >= 0.5 {
        // Center and two rings of eight taps, weights summing to 1.
        glow *= 0.2;
        for (var i = 0; i < 8; i++) {
            let a = f32(i) * 0.785398;
            let d = vec2<f32>(cos(a), sin(a)) * radius;
            let inner = sample_tex(layer_tex, position.xy + d * 0.5, 0u);
            let outer = sample_tex(layer_tex, position.xy + d, 0u);
            glow += bright(inner, threshold, softness) * 0.06;
            glow += bright(outer, threshold, softness) * 0.04;
        }
    }
    let s = glow * intensity * params.opacity;
    let a = min(s.a, 1.0);
    return vec4<f32>(b.rgb + s.rgb, a + b.a * (1.0 - a));
}

fn pcg(v: u32) -> u32 {
    let state = v * 747796405u + 2891336453u;
    let word = ((state >> ((state >> 28u) + 4u)) ^ state) * 277803737u;
    return (word >> 22u) ^ word;
}

// Dissolve Noise: the layer shows over the image below where a noise value
// is under the opacity, so raising opacity dissolves it in.
// values: grain, softness, speed, seed.
fn dissolve_noise(position: vec2<f32>, b: vec4<f32>, layer: vec4<f32>) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(base_tex));
    let grain = max(1.0, params.values[0].x * dims.y / 1080.0);
    let softness = params.values[0].y;
    let speed = params.values[0].z;
    let seed = u32(max(params.values[0].w, 0.0) * 1000.0);
    let frame = u32(max(floor(params.time * speed), 0.0));
    let cell = vec2<u32>(floor(position.xy / grain));
    let h = pcg(cell.x ^ pcg(cell.y ^ pcg(seed ^ pcg(frame))));
    let n = f32(h >> 8u) / 16777216.0;
    let v = params.opacity * (1.0 + softness);
    var mask: f32;
    if softness > 0.0 {
        mask = smoothstep(n, n + softness, v);
    } else {
        mask = select(0.0, 1.0, v > n);
    }
    let s = layer * mask;
    return s + b * (1.0 - s.a);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(position.xy);
    let b = textureLoad(base_tex, pixel, 0);
    let layer = textureLoad(layer_tex, pixel, 0);
    let s = layer * params.opacity;
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
        // Mask modes: the layer is not drawn; opacity fades the mask in.
        // Alpha Mask: below shows where the layer is opaque.
        case 7u: {
            return b * mix(1.0, layer.a, params.opacity);
        }
        // Luma Mask: below shows where the layer is bright.
        case 8u: {
            return b * mix(1.0, luma(layer), params.opacity);
        }
        // Inverted Luma Mask: below shows where the layer is dark.
        case 9u: {
            return b * mix(1.0, 1.0 - luma(layer), params.opacity);
        }
        // Inverted Alpha Mask: below is cut out where the layer is.
        case 10u: {
            return b * mix(1.0, 1.0 - layer.a, params.opacity);
        }
        // Subtract: remove the light, keep the coverage.
        case 12u: {
            return vec4<f32>(max(b.rgb - s.rgb, vec3<f32>(0.0)), b.a);
        }
        case 13u: {
            return liquid_displace(position.xy);
        }
        case 14u: {
            return chroma_shift(position.xy);
        }
        case 15u: {
            return glow_add(position.xy, b);
        }
        case 16u: {
            return dissolve_noise(position.xy, b, layer);
        }
        default: {
            let mixed = blend_fn(params.mode, unpremultiply(b), unpremultiply(s));
            let rgb = (1.0 - s.a) * b.rgb + (1.0 - b.a) * s.rgb + s.a * b.a * mixed;
            return vec4<f32>(rgb, a);
        }
    }
}
