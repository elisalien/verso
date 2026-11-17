// name: Tilt-Shift
// category: Lens
// param: focus float 0 1 0.5
// param: band float 0 1 0.2
// param: radius float 0 1 0.5
// param: angle float -90 90 0
// param: saturation float 0 2 1.3

// Miniature look: sharp along a band, blurred more the farther from it,
// with more saturation as on toy-like photographs.

const GOLDEN_ANGLE: f32 = 2.39996323;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let a = radians(param_angle());
    // Distance from the focus line, across the band.
    let p = (uv - vec2<f32>(0.5, param_focus())) * vec2<f32>(verso.aspect, 1.0);
    let across = abs(dot(p, vec2<f32>(-sin(a), cos(a))));
    let blur = smoothstep(param_band() * 0.5, param_band() * 0.5 + 0.35, across);
    let radius = blur * param_radius() * 0.03 * vec2<f32>(1.0 / verso.aspect, 1.0);
    var sum = vec4<f32>(0.0);
    for (var i = 0; i < 32; i++) {
        let r = sqrt((f32(i) + 0.5) / 32.0);
        let t = f32(i) * GOLDEN_ANGLE;
        sum += sample(uv + vec2<f32>(cos(t), sin(t)) * r * radius);
    }
    let c = unpremultiply(sum / 32.0);
    let luma = dot(c.rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    let rgb = clamp(mix(vec3<f32>(luma), c.rgb, param_saturation()), vec3<f32>(0.0), vec3<f32>(1.0));
    return premultiply(vec4<f32>(rgb, c.a));
}
