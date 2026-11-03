// Test patterns drawn on a screen in place of its slices, to set up
// projectors. Procedural: no texture, numbers in 7-segment digits.

struct VertexIn {
    @location(0) position: vec2<f32>,
    // Mode, number, center x, center y (pixels).
    @location(1) info: vec4<f32>,
    // Screen width, height (pixels), hue (0–1), unused.
    @location(2) extra: vec4<f32>,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) info: vec4<f32>,
    @location(1) @interpolate(flat) extra: vec4<f32>,
}

const GRID: u32 = 0u;
const COLORS: u32 = 1u;
const GRADIENT: u32 = 2u;
const SLICES: u32 = 3u;
const IDENTIFY: u32 = 4u;

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var out: VertexOut;
    out.position = vec4<f32>(in.position, 0.0, 1.0);
    out.info = in.info;
    out.extra = in.extra;
    return out;
}

fn hue(h: f32) -> vec3<f32> {
    let k = vec3<f32>(0.0, 2.0 / 3.0, 1.0 / 3.0);
    return clamp(abs(fract(h + k) * 6.0 - 3.0) - 1.0, vec3<f32>(0.0), vec3<f32>(1.0));
}

// Segments a–g of each digit, bit 0 = a.
fn segments(d: u32) -> u32 {
    var table = array<u32, 10>(0x3fu, 0x06u, 0x5bu, 0x4fu, 0x66u, 0x6du, 0x7du, 0x07u, 0x7fu, 0x6fu);
    return table[d % 10u];
}

// Whether local point `q` (x 0–1, y 0–2, y down) lies on a lit segment.
fn digit_lit(d: u32, q: vec2<f32>) -> bool {
    let t = 0.2;
    let on = segments(d);
    let x0 = q.x < t;
    let x1 = q.x > 1.0 - t;
    let upper = q.y > t * 0.5 && q.y < 1.0;
    let lower = q.y > 1.0 && q.y < 2.0 - t * 0.5;
    let a = q.y < t;
    let g = abs(q.y - 1.0) < t * 0.5;
    let dd = q.y > 2.0 - t;
    return ((on & 1u) != 0u && a)
        || ((on & 2u) != 0u && x1 && upper)
        || ((on & 4u) != 0u && x1 && lower)
        || ((on & 8u) != 0u && dd)
        || ((on & 16u) != 0u && x0 && lower)
        || ((on & 32u) != 0u && x0 && upper)
        || ((on & 64u) != 0u && g);
}

fn digit_count(value: u32) -> u32 {
    var n = 1u;
    var v = value / 10u;
    while v > 0u {
        n += 1u;
        v /= 10u;
    }
    return n;
}

// 0 outside the number's box, 1 in its backdrop, 2 on a lit segment.
// `center` and `height` are in pixels.
fn number(p: vec2<f32>, center: vec2<f32>, height: f32, value: u32) -> u32 {
    let unit = height * 0.5;
    let n = digit_count(value);
    let width = (f32(n) * 1.4 - 0.4) * unit;
    let origin = center - vec2<f32>(width * 0.5, unit);
    let local = (p - origin) / unit;
    let margin = 0.5;
    if local.x < -margin || local.y < -margin || local.x > width / unit + margin || local.y > 2.0 + margin {
        return 0u;
    }
    if local.x < 0.0 || local.y < 0.0 || local.y > 2.0 {
        return 1u;
    }
    let index = u32(floor(local.x / 1.4));
    let within = local.x - f32(index) * 1.4;
    if index >= n || within > 1.0 {
        return 1u;
    }
    var v = value;
    for (var k = 0u; k < n - 1u - index; k++) {
        v /= 10u;
    }
    if digit_lit(v % 10u, vec2<f32>(within, local.y)) {
        return 2u;
    }
    return 1u;
}

// Draws a number over `color`: white digits on a darkened box.
fn with_number(color: vec3<f32>, p: vec2<f32>, center: vec2<f32>, height: f32, value: u32) -> vec3<f32> {
    switch number(p, center, height, value) {
        case 2u: { return vec3<f32>(1.0); }
        case 1u: { return color * 0.25; }
        default: { return color; }
    }
}

fn grid(p: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    var c = vec3<f32>(0.04);
    let cell = 100.0;
    let m = p % cell;
    if m.x < 1.0 || m.y < 1.0 {
        c = vec3<f32>(0.45);
    }
    let center = size * 0.5;
    // Centre axes and the screen border.
    if abs(p.x - center.x) < 1.5 || abs(p.y - center.y) < 1.5 {
        c = vec3<f32>(1.0);
    }
    if p.x < 3.0 || p.y < 3.0 || p.x > size.x - 3.0 || p.y > size.y - 3.0 {
        c = vec3<f32>(1.0, 0.2, 0.2);
    }
    // Inscribed circle, to see the aspect ratio and keystone.
    let r = min(size.x, size.y) * 0.5 - 4.0;
    if abs(length(p - center) - r) < 1.5 {
        c = vec3<f32>(1.0, 1.0, 0.0);
    }
    // Number of each cell, row by row from 1, in its top left corner.
    let column = floor(p.x / cell);
    let row = floor(p.y / cell);
    let columns = ceil(size.x / cell);
    let value = u32(row * columns + column) + 1u;
    let label = vec2<f32>(column * cell + 22.0, row * cell + 16.0);
    return with_number(c, p, label, 14.0, value);
}

fn colors(p: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    var bars = array<vec3<f32>, 8>(
        vec3<f32>(1.0, 1.0, 1.0), vec3<f32>(1.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 1.0),
        vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0),
        vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 0.0),
    );
    if p.y > size.y * 0.75 {
        // Mid grey, to match the brightness of projectors.
        return vec3<f32>(0.5);
    }
    let i = min(u32(p.x / size.x * 8.0), 7u);
    return bars[i];
}

fn gradient(p: vec2<f32>, size: vec2<f32>) -> vec3<f32> {
    let x = clamp(p.x / size.x, 0.0, 1.0);
    if p.y < size.y * 0.5 {
        return vec3<f32>(x);
    }
    // Steps of 10 %.
    return vec3<f32>(floor(x * 10.0 + 0.5) / 10.0);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    let size = in.extra.xy;
    let mode = u32(in.info.x);
    var c: vec3<f32>;
    switch mode {
        case GRID: { c = grid(p, size); }
        case COLORS: { c = colors(p, size); }
        case GRADIENT: { c = gradient(p, size); }
        case SLICES: {
            c = hue(in.extra.z) * 0.6;
            let height = clamp(min(size.x, size.y) * 0.15, 24.0, 160.0);
            c = with_number(c, p, in.info.zw, height, u32(in.info.y));
        }
        default: {
            // Identify: the screen's color, a border, its number and size.
            let color = hue(in.extra.z);
            c = color * 0.3;
            let border = 20.0;
            if p.x < border || p.y < border || p.x > size.x - border || p.y > size.y - border {
                c = color;
            }
            let center = size * 0.5;
            let height = min(size.x, size.y) * 0.4;
            c = with_number(c, p, center, height, u32(in.info.y));
            let small = max(height * 0.12, 16.0);
            // Under the big number's box, which reaches 0.75 × height down.
            let below = center.y + height * 0.75 + small * 1.5;
            c = with_number(c, p, vec2<f32>(center.x - small * 2.5, below), small, u32(size.x));
            c = with_number(c, p, vec2<f32>(center.x + small * 2.5, below), small, u32(size.y));
        }
    }
    // Output targets are sRGB: the pattern values are what the eye sees.
    return vec4<f32>(pow(c, vec3<f32>(2.2)), 1.0);
}
