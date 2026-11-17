// name: Deck FX
// category: Color
// param: negative float 0 1 0
// param: brightness float -1 1 0
// param: contrast float 0 3 1
// param: gamma float 0.1 4 1
// param: hue float -180 180 0
// param: saturation float 0 3 1
// param: luma_key_min float 0 1 0
// param: luma_key_max float 0 1 1
// param: gain color 1 1 1 1
// param: alpha float 0 1 1

// An output colour stage, in this order: negative, brightness, contrast,
// gamma, then hue, saturation and luma key in YIQ, then RGB gain and alpha.
//
// A key bound at its end of the range (min 0, max 1) disables that side, so
// pure black and pure white stay opaque by default.

const RGB_TO_YIQ = mat3x3<f32>(
    vec3<f32>(0.299, 0.5959, 0.2115),
    vec3<f32>(0.587, -0.2746, -0.5227),
    vec3<f32>(0.114, -0.3213, 0.3112),
);
const YIQ_TO_RGB = mat3x3<f32>(
    vec3<f32>(1.0, 1.0, 1.0),
    vec3<f32>(0.956, -0.272, -1.106),
    vec3<f32>(0.619, -0.647, 1.703),
);
// Slope of the luma key edge: fully keyed 0.02 past the bound.
const KEY_SLOPE: f32 = 50.0;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let c = unpremultiply(sample(uv));
    var rgb = mix(c.rgb, 1.0 - c.rgb, param_negative());
    rgb += param_brightness();
    rgb = clamp((rgb - 0.5) * param_contrast() + 0.5, vec3<f32>(0.0), vec3<f32>(1.0));
    rgb = pow(rgb, vec3<f32>(param_gamma()));

    var yiq = RGB_TO_YIQ * rgb;
    let a = radians(param_hue());
    let iq = vec2<f32>(yiq.y * cos(a) - yiq.z * sin(a), yiq.y * sin(a) + yiq.z * cos(a));
    yiq = vec3<f32>(yiq.x, iq * param_saturation());

    let lo = param_luma_key_min();
    let hi = param_luma_key_max();
    let key_lo = select(clamp((yiq.x - lo) * KEY_SLOPE, 0.0, 1.0), 1.0, lo <= 0.0);
    let key_hi = select(clamp((hi - yiq.x) * KEY_SLOPE, 0.0, 1.0), 1.0, hi >= 1.0);

    rgb = max(YIQ_TO_RGB * yiq, vec3<f32>(0.0)) * param_gain().rgb;
    let alpha = c.a * key_lo * key_hi * param_alpha() * param_gain().a;
    return premultiply(vec4<f32>(rgb, alpha));
}
