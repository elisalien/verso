// name: Prism
// category: Lens
// param: amount float 0 1 0.3
// param: center point 0 1 0.5 0.5
// param: spectrum bool true

// Chromatic aberration growing from the center: each color is scaled by
// its own amount, as a lens refracts wavelengths differently. With
// spectrum on, seven tints spread the colors into a rainbow fringe.

fn tint(t: f32) -> vec3<f32> {
    // Red, green and blue weights across the spectrum, t from 0 to 1.
    let x = t * 3.0;
    return clamp(
        vec3<f32>(1.5 - abs(x), 1.5 - abs(x - 1.5), 1.5 - abs(x - 3.0)),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let center = param_center();
    let d = uv - center;
    let spread = param_amount() * 0.06;
    if !param_spectrum() {
        let r = sample(center + d * (1.0 + spread));
        let g = sample(uv);
        let b = sample(center + d * (1.0 - spread));
        return vec4<f32>(r.r, g.g, b.b, max(max(r.a, g.a), b.a));
    }
    var sum = vec3<f32>(0.0);
    var weight = vec3<f32>(0.0);
    var alpha = 0.0;
    for (var i = 0; i < 7; i++) {
        let t = f32(i) / 6.0;
        let s = sample(center + d * (1.0 + spread * (1.0 - 2.0 * t)));
        let w = tint(t);
        sum += s.rgb * w;
        weight += w;
        alpha = max(alpha, s.a);
    }
    return vec4<f32>(sum / weight, alpha);
}
