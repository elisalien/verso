// name: Lens Distortion
// category: Lens
// param: amount float -1 1 0.3
// param: zoom float 0.5 2 1
// param: fill bool true

// Barrel (positive) or pincushion (negative) distortion. With fill on, the
// image is scaled so no empty edge shows.

fn effect(uv: vec2<f32>) -> vec4<f32> {
    let k = param_amount() * 0.5;
    var p = (uv - 0.5) * vec2<f32>(verso.aspect, 1.0);
    // Distance of a frame corner, to scale the result back into the frame.
    let corner = length(vec2<f32>(verso.aspect, 1.0) * 0.5);
    var scale = 1.0 / param_zoom();
    if param_fill() && k > 0.0 {
        scale /= 1.0 + k * corner * corner;
    }
    p *= scale;
    let r2 = dot(p, p);
    let q = p * (1.0 + k * r2) / vec2<f32>(verso.aspect, 1.0) + 0.5;
    if any(q < vec2<f32>(0.0)) || any(q > vec2<f32>(1.0)) {
        return vec4<f32>(0.0);
    }
    return sample(q);
}
