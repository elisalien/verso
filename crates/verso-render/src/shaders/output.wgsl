// Applies the master and flattens the composition over black.

struct Params {
    opacity: f32,
    // Scalars, not vec3: a vec3 would be 16-byte aligned and grow the struct.
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var comp_tex: texture_2d<f32>;

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let c = textureLoad(comp_tex, vec2<i32>(position.xy), 0);
    return vec4<f32>(clamp(c.rgb * params.opacity, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
