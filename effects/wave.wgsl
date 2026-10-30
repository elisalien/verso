// Example Verso effect. Copy this file to start your own; Verso loads every
// .wgsl and .fs file in this folder at startup.
//
// name: Wave
// category: Distort
// param: amplitude float 0 0.1 0.02
// param: frequency float 1 40 10
// param: speed float -4 4 1

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let offset = sin(uv.y * param_frequency() + verso.time * param_speed()) * param_amplitude();
    return sample(uv + vec2<f32>(offset, 0.0));
}
