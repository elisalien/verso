// name: RGB Split
// category: Distort
// param: amount float 0 1 0.2
// param: angle float 0 360 0

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let a = radians(param_angle());
    let offset = vec2<f32>(cos(a), sin(a)) * param_amount() * 0.03;
    let r = sample(uv + offset);
    let g = sample(uv);
    let b = sample(uv - offset);
    return vec4<f32>(r.r, g.g, b.b, max(max(r.a, g.a), b.a));
}
