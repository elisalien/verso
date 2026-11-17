// Fixed stages of the MilkDrop passes. Internal passes flip y so that the
// feedback textures keep OpenGL's bottom-up row order: preset shaders then
// sample them with the same coordinates as in MilkDrop and Butterchurn.

struct WarpOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) uv_orig: vec2<f32>,
    @location(2) color: vec4<f32>,
}

@vertex
fn warp_vs(
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<f32>,
) -> WarpOut {
    var out: WarpOut;
    out.position = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    out.uv = uv;
    out.uv_orig = pos * 0.5 + 0.5;
    out.color = color;
    return out;
}

struct CompOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
}

// The composite pass draws the final image the right way up.
@vertex
fn comp_vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> CompOut {
    var out: CompOut;
    out.position = vec4<f32>(pos, 0.0, 1.0);
    out.uv = pos * 0.5 + 0.5;
    out.color = color;
    return out;
}

struct SpriteOut {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
}

@group(0) @binding(0) var sprite_tex: texture_2d<f32>;
@group(0) @binding(1) var sprite_sampler: sampler;

@vertex
fn sprite_vs(
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) uv: vec2<f32>,
) -> SpriteOut {
    var out: SpriteOut;
    out.position = vec4<f32>(pos.x, -pos.y, 0.0, 1.0);
    out.color = color;
    out.uv = uv;
    return out;
}

@fragment
fn sprite_flat(in: SpriteOut) -> @location(0) vec4<f32> {
    return in.color;
}

@fragment
fn sprite_textured(in: SpriteOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(sprite_tex, sprite_sampler, in.uv, 0.0) * in.color;
}
