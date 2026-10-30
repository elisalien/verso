// name: Mirror
// category: Distort
// param: horizontal bool true
// param: vertical bool false

fn effect(uv: vec2<f32>) -> vec4<f32> {
    var p = uv;
    if param_horizontal() {
        p.x = 0.5 - abs(p.x - 0.5);
    }
    if param_vertical() {
        p.y = 0.5 - abs(p.y - 0.5);
    }
    return sample(p);
}
