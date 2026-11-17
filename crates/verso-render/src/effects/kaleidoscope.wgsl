// name: Kaleidoscope
// category: Distort
// param: segments float 2 16 6
// param: rotation float -1 1 0
// param: spin float -1 1 0.1

const TAU: f32 = 6.28318530718;

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let p = (uv - 0.5) * vec2<f32>(verso.aspect, 1.0);
    let segment = TAU / max(floor(param_segments()), 2.0);
    var angle = atan2(p.y, p.x) + (param_rotation() + verso.time * param_spin()) * TAU;
    angle = abs((angle % segment + segment) % segment - segment / 2.0);
    let q = vec2<f32>(cos(angle), sin(angle)) * length(p);
    return sample(q / vec2<f32>(verso.aspect, 1.0) + 0.5);
}
