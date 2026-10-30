// name: Posterize
// category: Color
// param: levels float 2 16 4

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    let n = max(floor(param_levels()), 2.0) - 1.0;
    return premultiply(vec4<f32>(floor(c.rgb * n + 0.5) / n, c.a));
}
