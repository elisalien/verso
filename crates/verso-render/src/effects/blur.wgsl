// name: Blur
// category: Blur
// param: radius float 0 1 0.2

fn effect(uv: vec2<f32>) -> vec4<f32> {
    // 7×7 Gaussian-weighted taps, spread up to 40 pixels.
    let step = param_radius() * 40.0 / 3.0 / resolution();
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var y = -3; y <= 3; y++) {
        for (var x = -3; x <= 3; x++) {
            let w = exp(-f32(x * x + y * y) / 8.0);
            sum += sample(uv + vec2<f32>(f32(x), f32(y)) * step) * w;
            total += w;
        }
    }
    return sum / total;
}
