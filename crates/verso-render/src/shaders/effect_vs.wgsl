// The one vertex stage of every full-screen pass (effects, generators, media,
// blend, transition, output, MilkDrop blur and input): one triangle covering
// the target, with UVs from the top-left corner at location 0. It is put in
// front of the shaders that have only a fragment stage, see
// `with_fullscreen_vs`.

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VsOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: VsOut;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}
