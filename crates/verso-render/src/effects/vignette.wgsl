// name: Vignette
// category: Lens
// param: amount float 0 1 0.6
// param: size float 0 1.5 0.75
// param: softness float 0.01 1 0.45
// param: round bool true
// param: color color 0 0 0 1

// Darkens the edges toward a color, round or following the frame shape.

fn effect(uv: vec2<f32>) -> vec4<f32> {
    var p = uv - 0.5;
    if param_round() {
        p.x *= verso.aspect;
        p /= max(verso.aspect, 1.0);
    }
    let d = length(p) * 2.0;
    let edge = smoothstep(param_size(), param_size() + param_softness(), d) * param_amount();
    let s = sample(uv);
    let c = param_color();
    // Mixed in premultiplied space, within the coverage of the image.
    return vec4<f32>(mix(s.rgb, c.rgb * s.a, edge), s.a);
}
