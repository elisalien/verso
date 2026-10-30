// name: Tint
// category: Color
// param: color color 0.18 0.62 0.75 1
// param: amount float 0 1 1

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    let grey = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let tinted = grey * param_color().rgb;
    return premultiply(vec4<f32>(mix(c.rgb, tinted, param_amount()), c.a));
}
