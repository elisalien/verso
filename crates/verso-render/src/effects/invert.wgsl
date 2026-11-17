// name: Invert
// category: Color
// param: amount float 0 1 1

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    let inverted = vec3<f32>(1.0) - c.rgb;
    return premultiply(vec4<f32>(mix(c.rgb, inverted, param_amount()), c.a));
}
