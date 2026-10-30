// name: Brightness Contrast
// category: Color
// param: brightness float -1 1 0
// param: contrast float 0 3 1

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    let rgb = (c.rgb - 0.5) * param_contrast() + 0.5 + param_brightness();
    return premultiply(vec4<f32>(max(rgb, vec3<f32>(0.0)), c.a));
}
