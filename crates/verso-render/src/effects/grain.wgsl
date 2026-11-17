// name: Film Grain
// category: Lens
// param: amount float 0 1 0.25
// param: size float 1 4 1.5
// param: color bool false

// Animated film grain, strongest in the mid-tones as on film stock.

fn hash(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yxz + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn noise(cell: vec2<f32>, seed: f32) -> f32 {
    // Smoothed value noise, so large grain is round rather than square.
    let i = floor(cell);
    let f = fract(cell);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(vec3<f32>(i, seed));
    let b = hash(vec3<f32>(i + vec2<f32>(1.0, 0.0), seed));
    let c = hash(vec3<f32>(i + vec2<f32>(0.0, 1.0), seed));
    let d = hash(vec3<f32>(i + vec2<f32>(1.0, 1.0), seed));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let s = unpremultiply(sample(uv));
    let cell = uv * resolution() / param_size();
    let frame = floor(verso.time * 24.0);
    var n = vec3<f32>(noise(cell, frame));
    if param_color() {
        n = vec3<f32>(n.x, noise(cell, frame + 17.0), noise(cell, frame + 41.0));
    }
    let luma = dot(s.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let strength = param_amount() * 0.35 * (1.0 - abs(luma * 2.0 - 1.0) * 0.6);
    let rgb = clamp(s.rgb + (n - 0.5) * strength, vec3<f32>(0.0), vec3<f32>(1.0));
    return premultiply(vec4<f32>(rgb, s.a));
}
