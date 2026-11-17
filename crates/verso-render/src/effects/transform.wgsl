// name: Transform
// category: Transform
// param: scale float 0.1 4 1
// param: rotation float -180 180 0
// param: position point -1 1 0 0

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let a = radians(param_rotation());
    // Square pixels while rotating.
    var p = (uv - 0.5 - param_position() * vec2<f32>(0.5, -0.5)) * vec2<f32>(verso.aspect, 1.0);
    p = mat2x2<f32>(cos(a), -sin(a), sin(a), cos(a)) * p / param_scale();
    let q = p / vec2<f32>(verso.aspect, 1.0) + 0.5;
    let inside = all(q >= vec2<f32>(0.0)) && all(q <= vec2<f32>(1.0));
    return select(vec4<f32>(0.0), sample(q), inside);
}
