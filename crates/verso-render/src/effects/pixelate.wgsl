// name: Pixelate
// category: Distort
// param: size float 0 1 0.2

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let cells = mix(320.0, 8.0, pow(param_size(), 0.5));
    let grid = vec2<f32>(cells * verso.aspect, cells);
    return sample((floor(uv * grid) + 0.5) / grid);
}
