// name: Hue Saturation
// category: Color
// param: hue float -180 180 0
// param: saturation float 0 2 1
// param: lightness float -1 1 0

fn rotate_hue(rgb: vec3<f32>, degrees: f32) -> vec3<f32> {
    // Rotation around the grey axis in YIQ space.
    let yiq = mat3x3<f32>(
        vec3<f32>(0.299, 0.596, 0.211),
        vec3<f32>(0.587, -0.274, -0.523),
        vec3<f32>(0.114, -0.322, 0.312),
    ) * rgb;
    let a = radians(degrees);
    let iq = vec2<f32>(yiq.y * cos(a) - yiq.z * sin(a), yiq.y * sin(a) + yiq.z * cos(a));
    return mat3x3<f32>(
        vec3<f32>(1.0, 1.0, 1.0),
        vec3<f32>(0.956, -0.272, -1.106),
        vec3<f32>(0.621, -0.647, 1.703),
    ) * vec3<f32>(yiq.x, iq);
}

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    var rgb = rotate_hue(c.rgb, param_hue());
    let grey = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3<f32>(grey), rgb, param_saturation());
    rgb = select(rgb * (1.0 + param_lightness()), mix(rgb, vec3<f32>(1.0), param_lightness()), param_lightness() > 0.0);
    return premultiply(vec4<f32>(max(rgb, vec3<f32>(0.0)), c.a));
}
