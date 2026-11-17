// name: Bokeh
// category: Lens
// param: radius float 0 1 0.3
// param: highlights float 0 4 1.5
// param: quality float 16 128 64

// Lens blur gathered along a golden-angle spiral, which spreads samples
// evenly over a disc. Bright samples weigh more, so highlights bloom into
// discs as out-of-focus lights do.

const GOLDEN_ANGLE: f32 = 2.39996323;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let radius = param_radius() * 0.04 * vec2<f32>(1.0 / verso.aspect, 1.0);
    let count = i32(clamp(param_quality(), 16.0, 128.0));
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < count; i++) {
        let r = sqrt((f32(i) + 0.5) / f32(count));
        let a = f32(i) * GOLDEN_ANGLE;
        let s = sample(uv + vec2<f32>(cos(a), sin(a)) * r * radius);
        let luma = dot(unpremultiply(s).rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
        let w = 1.0 + param_highlights() * pow(luma, 4.0) * 8.0;
        sum += s * w;
        total += w;
    }
    return sum / total;
}
