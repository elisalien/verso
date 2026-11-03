//! Renders compositions on the GPU and checks output pixels.
//! Tests pass without running when no adapter is available (e.g. headless CI).

use verso_core::{
    BlendMode, Clip, Command, Composition, Effect, EffectEdit, EffectTarget, ParamValue, Source,
};
use verso_render::{FrameGrabber, MediaTexture, MediaView, Renderer};

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
}

fn gpu() -> Option<Gpu> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some(Gpu { device, queue })
}

/// Renders `comp` and returns its RGBA8 sRGB pixels.
fn render(gpu: &Gpu, comp: &Composition, time: f32) -> Vec<u8> {
    render_with_media(gpu, comp, &[], time)
}

fn render_with_media(
    gpu: &Gpu,
    comp: &Composition,
    media: &[Option<MediaView<'_>>],
    time: f32,
) -> Vec<u8> {
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    render_frame(gpu, &mut renderer, comp, media, time)
}

/// Renders one frame with a long-lived renderer and reads its pixels back.
fn render_frame(
    gpu: &Gpu,
    renderer: &mut Renderer,
    comp: &Composition,
    media: &[Option<MediaView<'_>>],
    time: f32,
) -> Vec<u8> {
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, comp, media, time);
    read_back(gpu, encoder, renderer.output_texture(), renderer.size())
}

/// Submits `encoder` with a copy of `texture` (RGBA8) and returns its pixels.
fn read_back(
    gpu: &Gpu,
    mut encoder: wgpu::CommandEncoder,
    texture: &wgpu::Texture,
    (w, h): (u32, u32),
) -> Vec<u8> {
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();

    let data = buffer.get_mapped_range(..).unwrap();
    data.chunks(row as usize)
        .flat_map(|r| &r[..(w * 4) as usize])
        .copied()
        .collect()
}

fn solid(color: [f32; 4]) -> Option<Clip> {
    Some(Clip::new("Solid", Source::Solid { color }))
}

/// A tiny composition with one solid clip per layer, all playing.
fn solids(layers: &[([f32; 4], BlendMode, f32)]) -> Composition {
    let mut comp = Composition::new("test", layers.len(), 1);
    comp.width = 16;
    comp.height = 8;
    for (i, (color, blend, opacity)) in layers.iter().enumerate() {
        comp.apply(Command::SetClip {
            layer: i,
            column: 0,
            clip: solid(*color),
        })
        .unwrap();
        comp.apply(Command::SetLayerBlend {
            layer: i,
            blend: *blend,
        })
        .unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: i,
            opacity: *opacity,
        })
        .unwrap();
    }
    comp.apply(Command::TriggerColumn(0)).unwrap();
    comp
}

fn first_pixel(pixels: &[u8]) -> [u8; 3] {
    [pixels[0], pixels[1], pixels[2]]
}

const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
const GREEN: [f32; 4] = [0.0, 1.0, 0.0, 1.0];
const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

#[test]
fn blend_modes_produce_expected_colors() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let cases: &[(&str, Composition, [u8; 3])] = &[
        ("empty is black", Composition::new("t", 1, 1), [0, 0, 0]),
        (
            "alpha",
            solids(&[(RED, BlendMode::Alpha, 1.0)]),
            [255, 0, 0],
        ),
        (
            "add",
            solids(&[(RED, BlendMode::Alpha, 1.0), (GREEN, BlendMode::Add, 1.0)]),
            [255, 255, 0],
        ),
        (
            "multiply",
            solids(&[
                (WHITE, BlendMode::Alpha, 1.0),
                (RED, BlendMode::Multiply, 1.0),
            ]),
            [255, 0, 0],
        ),
        (
            "difference",
            solids(&[
                (WHITE, BlendMode::Alpha, 1.0),
                (RED, BlendMode::Difference, 1.0),
            ]),
            [0, 255, 255],
        ),
        (
            "alpha over at half opacity",
            solids(&[(RED, BlendMode::Alpha, 1.0), (GREEN, BlendMode::Alpha, 0.5)]),
            // Linear 0.5 encoded as sRGB.
            [188, 188, 0],
        ),
    ];
    for (name, comp, expected) in cases {
        let got = first_pixel(&render(&gpu, comp, 0.0));
        for c in 0..3 {
            assert!(
                got[c].abs_diff(expected[c]) <= 2,
                "{name}: got {got:?}, expected {expected:?}"
            );
        }
    }
}

#[test]
fn bypass_and_master_opacity() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    comp.apply(Command::SetLayerBypass {
        layer: 0,
        bypass: true,
    })
    .unwrap();
    assert_eq!(first_pixel(&render(&gpu, &comp, 0.0)), [0, 0, 0]);

    comp.apply(Command::SetLayerBypass {
        layer: 0,
        bypass: false,
    })
    .unwrap();
    comp.apply(Command::SetMasterOpacity(0.0)).unwrap();
    assert_eq!(first_pixel(&render(&gpu, &comp, 0.0)), [0, 0, 0]);
}

/// Writes the demo composition to `target/verso-demo.png` for a visual check.
#[test]
fn demo_renders_non_black_image() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = Composition::demo();
    comp.width = 640;
    comp.height = 360;
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let pixels = render(&gpu, &comp, 1.5);
    assert!(pixels.chunks(4).any(|p| p[..3] != [0, 0, 0]));

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/verso-demo.png");
    image::save_buffer(
        &path,
        &pixels,
        comp.width,
        comp.height,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

#[test]
fn image_source_is_fitted_and_drawn() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = Composition::new("t", 1, 1);
    // 4:1 target with a 2:1 image: pillarboxed, image in the middle half.
    comp.width = 32;
    comp.height = 8;
    let clip = Clip::new(
        "Image",
        Source::Image {
            path: "unused.png".into(),
        },
    );
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(clip),
    })
    .unwrap();
    comp.apply(Command::SetLayerBlend {
        layer: 0,
        blend: BlendMode::Alpha,
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();

    // Without a decoded frame the layer is skipped.
    assert_eq!(first_pixel(&render(&gpu, &comp, 0.0)), [0, 0, 0]);

    let mut texture = MediaTexture::new(&gpu.device, (1, 1));
    // Left pixel red, right pixel green.
    texture.upload(
        &gpu.device,
        &gpu.queue,
        (2, 1),
        &[255, 0, 0, 255, 0, 255, 0, 255],
    );
    let pixels = render_with_media(&gpu, &comp, &[Some(texture.media_view())], 0.0);
    let at = |x: usize, y: usize| {
        let i = (y * 32 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    assert_eq!(at(2, 4), [0, 0, 0], "left bar is empty");
    assert_eq!(at(29, 4), [0, 0, 0], "right bar is empty");
    assert!(
        at(9, 4)[0] > 200 && at(9, 4)[1] < 60,
        "left of image is red: {:?}",
        at(9, 4)
    );
    assert!(
        at(22, 4)[1] > 200 && at(22, 4)[0] < 60,
        "right of image is green: {:?}",
        at(22, 4)
    );
}

fn add_effect(comp: &mut Composition, target: EffectTarget, id: &str) {
    comp.apply(Command::EditEffects {
        target,
        edit: EffectEdit::Add(Effect::new(id, id)),
    })
    .unwrap();
}

fn close(got: [u8; 3], expected: [u8; 3]) -> bool {
    (0..3).all(|c| got[c].abs_diff(expected[c]) <= 3)
}

#[test]
fn effects_apply_at_every_level() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    for target in [
        EffectTarget::Clip {
            layer: 0,
            column: 0,
        },
        EffectTarget::Layer(0),
        EffectTarget::Composition,
    ] {
        let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
        add_effect(&mut comp, target, "builtin:invert");
        let got = first_pixel(&render(&gpu, &comp, 0.0));
        assert!(close(got, [0, 255, 255]), "{target:?}: {got:?}");
    }
}

#[test]
fn bypassed_and_unknown_effects_pass_through() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    add_effect(&mut comp, EffectTarget::Layer(0), "builtin:invert");
    add_effect(&mut comp, EffectTarget::Layer(0), "missing:effect");
    comp.apply(Command::EditEffects {
        target: EffectTarget::Layer(0),
        edit: EffectEdit::SetBypass {
            index: 0,
            bypass: true,
        },
    })
    .unwrap();
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [255, 0, 0]));
}

#[test]
fn effect_parameters_reach_the_shader() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = solids(&[(WHITE, BlendMode::Alpha, 1.0)]);
    add_effect(&mut comp, EffectTarget::Layer(0), "builtin:invert");
    comp.apply(Command::EditEffects {
        target: EffectTarget::Layer(0),
        edit: EffectEdit::SetParam {
            index: 0,
            name: "amount".into(),
            value: ParamValue::Float(0.5),
        },
    })
    .unwrap();
    // Half inverted white: linear 0.5, sRGB 188.
    assert!(close(
        first_pixel(&render(&gpu, &comp, 0.0)),
        [188, 188, 188]
    ));
}

#[test]
fn isf_effect_runs_on_gpu() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("swap.fs");
    std::fs::write(
        &path,
        r#"/*{
    "INPUTS": [
        {"NAME": "inputImage", "TYPE": "image"},
        {"NAME": "mixAmount", "TYPE": "float", "DEFAULT": 1.0}
    ]
}*/
void main() {
    vec4 c = IMG_THIS_PIXEL(inputImage);
    gl_FragColor = mix(c, vec4(c.b, c.g, c.r, c.a), mixAmount);
}
"#,
    )
    .unwrap();

    let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let id = path.display().to_string();
    add_effect(&mut comp, EffectTarget::Layer(0), &id);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer.library_mut().load_file(&path).unwrap();
    let got = first_pixel(&render_frame(&gpu, &mut renderer, &comp, &[], 0.0));
    assert!(close(got, [0, 0, 255]), "{got:?}");
}

#[test]
fn groups_mix_their_layers_then_blend() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    // Red base, then a group of green (Add) at half group opacity.
    let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0), (GREEN, BlendMode::Alpha, 1.0)]);
    comp.apply(Command::AddGroup { layers: vec![1] }).unwrap();
    comp.apply(Command::SetGroupBlend {
        group: 0,
        blend: BlendMode::Add,
    })
    .unwrap();
    comp.apply(Command::SetGroupOpacity {
        group: 0,
        opacity: 0.5,
    })
    .unwrap();
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [255, 188, 0]));

    // Group effects process the group's image only.
    add_effect(&mut comp, EffectTarget::Group(0), "builtin:invert");
    comp.apply(Command::SetGroupOpacity {
        group: 0,
        opacity: 1.0,
    })
    .unwrap();
    // Inverted green (magenta) added onto red.
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [255, 0, 255]));

    comp.apply(Command::SetGroupBypass {
        group: 0,
        bypass: true,
    })
    .unwrap();
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [255, 0, 0]));
}

#[test]
fn transition_crossfades_between_clips() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = Composition::new("t", 1, 2);
    comp.width = 16;
    comp.height = 8;
    for (column, color) in [(0, RED), (1, GREEN)] {
        comp.apply(Command::SetClip {
            layer: 0,
            column,
            clip: solid(color),
        })
        .unwrap();
    }
    comp.apply(Command::SetLayerBlend {
        layer: 0,
        blend: BlendMode::Alpha,
    })
    .unwrap();
    comp.apply(Command::SetLayerTransition {
        layer: 0,
        seconds: 2.0,
    })
    .unwrap();
    comp.apply(Command::TriggerClip {
        layer: 0,
        column: 0,
    })
    .unwrap();

    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut frame =
        |comp: &Composition, time| first_pixel(&render_frame(&gpu, &mut renderer, comp, &[], time));
    // The first clip of an idle layer appears without a fade.
    assert!(close(frame(&comp, 0.0), [255, 0, 0]));

    comp.apply(Command::TriggerClip {
        layer: 0,
        column: 1,
    })
    .unwrap();
    assert!(
        close(frame(&comp, 10.0), [255, 0, 0]),
        "transition starts at the old clip"
    );
    assert!(
        close(frame(&comp, 11.0), [188, 188, 0]),
        "halfway is an even mix"
    );
    assert!(
        close(frame(&comp, 12.5), [0, 255, 0]),
        "ends on the new clip"
    );

    // Ejecting fades out.
    comp.apply(Command::EjectLayer(0)).unwrap();
    frame(&comp, 20.0);
    assert!(close(frame(&comp, 21.0), [0, 188, 0]));
    assert!(close(frame(&comp, 23.0), [0, 0, 0]));
}

#[test]
fn grabber_reads_frames_back_without_blocking() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut grabber = FrameGrabber::new(&gpu.device, renderer.size());
    let mut frame = None;
    for _ in 0..50 {
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
        grabber.capture(&mut encoder, renderer.output_texture());
        gpu.queue.submit([encoder.finish()]);
        grabber.after_submit();
        if let Some(f) = grabber.poll(&gpu.device) {
            frame = Some(f);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let frame = frame.expect("a frame within 50 renders");
    assert_eq!((frame.width, frame.height), (comp.width, comp.height));
    assert_eq!(frame.data.len(), (comp.width * comp.height * 4) as usize);
    assert!(frame.data.chunks(4).all(|p| p == [255, 0, 0, 255]));
}

#[test]
fn generator_shader_is_a_clip_source() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("solid.glsl");
    std::fs::write(
        &path,
        "// name: Solid Green\nvoid mainImage(out vec4 c, in vec2 p) { c = vec4(0.0, 1.0, 0.0, 1.0); }\n",
    )
    .unwrap();
    let id = path.display().to_string();
    let mut comp = Composition::new("t", 1, 1);
    comp.width = 16;
    comp.height = 8;
    let clip = Clip::new("Shader", Source::Shader(Effect::new(&id, "Solid Green")));
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(clip),
    })
    .unwrap();
    comp.apply(Command::SetLayerBlend {
        layer: 0,
        blend: BlendMode::Alpha,
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();

    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let def = renderer.library_mut().load_file(&path).unwrap();
    assert_eq!(def.kind, verso_render::ShaderKind::Generator);
    let got = first_pixel(&render_frame(&gpu, &mut renderer, &comp, &[], 0.0));
    assert!(close(got, [0, 255, 0]), "{got:?}");
}

#[test]
fn nested_groups_multiply_opacity_and_bypass() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = solids(&[(RED, BlendMode::Alpha, 1.0), (GREEN, BlendMode::Alpha, 1.0)]);
    // Outer group 0 holds inner group 1, which holds the green layer.
    comp.apply(Command::AddGroup { layers: vec![1] }).unwrap();
    comp.apply(Command::AddGroup { layers: vec![1] }).unwrap();
    assert_eq!(comp.groups[1].parent, Some(0));
    for group in [0, 1] {
        comp.apply(Command::SetGroupOpacity {
            group,
            opacity: 0.5,
        })
        .unwrap();
    }
    // Green at 0.25 over red: linear (0.75, 0.25, 0) in sRGB.
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [225, 137, 0]));

    comp.apply(Command::SetGroupBypass {
        group: 1,
        bypass: true,
    })
    .unwrap();
    assert!(close(first_pixel(&render(&gpu, &comp, 0.0)), [255, 0, 0]));
}

#[test]
fn milkdrop_preset_animates_with_feedback() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("milk.json");
    // Zooming feedback, a spinning additive shape, a wave and a blurred warp.
    let preset = r#"{
        "baseVals": {"decay": 0.97, "zoom": 1.02, "rot": 0.01, "wave_a": 1.0, "wave_mode": 2, "ob_size": 0.0, "ib_size": 0.0},
        "frame_eqs_eel": "wave_r = 0.5 + 0.5*sin(time); q1 = bass;",
        "pixel_eqs_eel": "zoom = zoom + rad*0.01;",
        "init_eqs_eel": "",
        "shapes": [{"baseVals": {"enabled": 1, "sides": 5, "additive": 1, "rad": 0.2, "r": 1, "g": 0.5, "b": 0, "a": 1, "a2": 0.5},
                    "init_eqs_eel": "", "frame_eqs_eel": "ang = time;"}],
        "waves": [],
        "warp": " shader_body { ret = texture(sampler_main, uv).rgb * decay + texture(sampler_blur1, uv).rgb * 0.02; }",
        "comp": ""
    }"#;
    std::fs::write(&path, preset).unwrap();
    let mut comp = Composition::new("t", 1, 1);
    comp.width = 64;
    comp.height = 48;
    let clip = Clip::new("Milk", Source::milkdrop("User/milk", 0));
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(clip),
    })
    .unwrap();
    comp.apply(Command::SetLayerBlend {
        layer: 0,
        blend: BlendMode::Alpha,
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();

    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer
        .milkdrop_mut()
        .library_mut()
        .load_file(&path)
        .unwrap();
    let samples: Vec<f32> = (0..1024).map(|i| (i as f32 * 0.07).sin() * 0.6).collect();
    let mut frames = Vec::new();
    for f in 0..30 {
        renderer.milkdrop_mut().set_audio(samples.clone(), 44100.0);
        frames.push(render_frame(
            &gpu,
            &mut renderer,
            &comp,
            &[],
            f as f32 / 30.0,
        ));
    }
    assert_eq!(renderer.milkdrop().error("User/milk", 0), None);
    let lit = |px: &[u8]| {
        px.chunks(4)
            .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 30)
            .count()
    };
    let last = frames.last().unwrap();
    assert!(lit(last) > 64 * 48 / 20, "only {} lit pixels", lit(last));
    assert_ne!(&frames[28], last, "image is static");
}

/// Writes a two-variant family and returns its path.
fn milk_family(dir: &std::path::Path) -> std::path::PathBuf {
    let path = dir.join("milk-family.json");
    // Variant 0 holds a red image, variant 1 a blue one; both keep the
    // feedback buffer as is (decay 1, no zoom).
    let variant = |r: f32, b: f32| {
        format!(
            r#"{{"name": "v", "preset": {{"baseVals": {{"decay": 1, "zoom": 1, "warp": 0, "wave_a": 0, "ob_size": 0, "ib_size": 0, "gammaadj": 1}},
            "warp": " shader_body {{ ret = vec3({r}, 0.0, {b}); }}", "comp": ""}}}}"#
        )
    };
    let json = format!(
        r#"{{"name": "Test", "category": "Test", "variants": [{}, {}]}}"#,
        variant(1.0, 0.0),
        variant(0.0, 1.0)
    );
    std::fs::write(&path, json).unwrap();
    path
}

fn milk_comp(source: Source, layers: usize) -> Composition {
    let mut comp = Composition::new("t", layers, 1);
    comp.width = 64;
    comp.height = 36;
    let last = layers - 1;
    comp.apply(Command::SetClip {
        layer: last,
        column: 0,
        clip: Some(Clip::new("Milk", source)),
    })
    .unwrap();
    for layer in 0..layers {
        comp.apply(Command::SetLayerBlend {
            layer,
            blend: BlendMode::Alpha,
        })
        .unwrap();
    }
    comp
}

fn mean_rgb(px: &[u8]) -> [f32; 3] {
    let n = (px.len() / 4) as f32;
    let mut sum = [0.0f32; 3];
    for p in px.chunks(4) {
        for c in 0..3 {
            sum[c] += p[c] as f32;
        }
    }
    sum.map(|s| s / n)
}

#[test]
fn milkdrop_variant_change_blends() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = milk_family(&dir);
    let mut comp = milk_comp(Source::milkdrop("Test/milk-family", 0), 1);
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer
        .milkdrop_mut()
        .library_mut()
        .load_file(&path)
        .unwrap();
    let mut t = 0.0;
    let mut frame = |renderer: &mut Renderer, comp: &Composition| {
        t += 0.1;
        mean_rgb(&render_frame(&gpu, renderer, comp, &[], t))
    };
    for _ in 0..3 {
        frame(&mut renderer, &comp);
    }
    let red = frame(&mut renderer, &comp);
    assert!(red[0] > 200.0 && red[2] < 20.0, "{red:?}");

    // Switch to the blue variant with a 1 s blend.
    if let Some(clip) = comp.layers[0].clips[0].as_mut()
        && let Source::Milkdrop { variant, blend, .. } = &mut clip.source
    {
        *variant = 1;
        *blend = 1.0;
    }
    let mut seen_mix = false;
    for _ in 0..8 {
        let c = frame(&mut renderer, &comp);
        seen_mix |= c[0] > 20.0 && c[2] > 20.0;
    }
    assert!(seen_mix, "no frame mixed both presets");
    for _ in 0..5 {
        frame(&mut renderer, &comp);
    }
    let blue = frame(&mut renderer, &comp);
    assert!(blue[2] > 200.0 && blue[0] < 20.0, "{blue:?}");
}

#[test]
fn milkdrop_reads_input_layer() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    // Default warp keeps the feedback; the input fully replaces it each frame.
    let path = dir.join("milk-input.json");
    std::fs::write(
        &path,
        r#"{"baseVals": {"decay": 1, "zoom": 1, "warp": 0, "wave_a": 0, "ob_size": 0, "ib_size": 0, "gammaadj": 1}, "warp": "", "comp": ""}"#,
    )
    .unwrap();
    let mut source = Source::milkdrop("User/milk-input", 0);
    if let Source::Milkdrop {
        input, input_mix, ..
    } = &mut source
    {
        *input = Some(0);
        *input_mix = 1.0;
    }
    let mut comp = milk_comp(source, 2);
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: solid([0.0, 1.0, 0.0, 1.0]),
    })
    .unwrap();
    // The input layer is hidden: only the preset shows it.
    comp.apply(Command::SetLayerOpacity {
        layer: 0,
        opacity: 0.0,
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer
        .milkdrop_mut()
        .library_mut()
        .load_file(&path)
        .unwrap();
    let mut last = [0.0; 3];
    for f in 0..4 {
        last = mean_rgb(&render_frame(
            &gpu,
            &mut renderer,
            &comp,
            &[],
            f as f32 * 0.1,
        ));
    }
    assert!(last[1] > 200.0 && last[0] < 30.0, "{last:?}");
}

#[test]
fn milkdrop_input_keeps_orientation() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("milk-input.json");
    std::fs::write(
        &path,
        r#"{"baseVals": {"decay": 1, "zoom": 1, "warp": 0, "wave_a": 0, "ob_size": 0, "ib_size": 0, "gammaadj": 1}, "warp": "", "comp": ""}"#,
    )
    .unwrap();
    // Red grows to the right, green upward: any flip changes a corner.
    let shader = dir.join("corners.glsl");
    std::fs::write(
        &shader,
        "// name: Corners
void mainImage(out vec4 c, in vec2 p) { c = vec4(p / iResolution.xy, 0.0, 1.0); }
",
    )
    .unwrap();
    let id = shader.display().to_string();
    let gradient = Source::Shader(Effect::new(&id, "Corners"));
    // Reference: the gradient on its own.
    let mut plain = Composition::new("t", 1, 1);
    plain.width = 64;
    plain.height = 36;
    plain
        .apply(Command::SetClip {
            layer: 0,
            column: 0,
            clip: Some(Clip::new("g", gradient.clone())),
        })
        .unwrap();
    plain.apply(Command::TriggerColumn(0)).unwrap();
    let mut plain_renderer = Renderer::new(&gpu.device, (plain.width, plain.height));
    plain_renderer.library_mut().load_file(&shader).unwrap();
    let reference = render_frame(&gpu, &mut plain_renderer, &plain, &[], 0.0);

    let mut source = Source::milkdrop("User/milk-input", 0);
    if let Source::Milkdrop {
        input, input_mix, ..
    } = &mut source
    {
        *input = Some(0);
        *input_mix = 1.0;
    }
    let mut comp = milk_comp(source, 2);
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(Clip::new("g", gradient)),
    })
    .unwrap();
    comp.apply(Command::SetLayerOpacity {
        layer: 0,
        opacity: 0.0,
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer
        .milkdrop_mut()
        .library_mut()
        .load_file(&path)
        .unwrap();
    renderer.library_mut().load_file(&shader).unwrap();
    let mut got = Vec::new();
    for f in 0..3 {
        got = render_frame(&gpu, &mut renderer, &comp, &[], 0.0 + f as f32 * 0.0);
    }
    let corner = |px: &[u8], x: usize, y: usize| {
        let i = (y * 64 + x) * 4;
        [px[i] as i32, px[i + 1] as i32, px[i + 2] as i32]
    };
    // The gradient tells the corners apart, so a flip would show.
    let corners = [(2, 2), (61, 2), (2, 33), (61, 33)];
    let distinct = corners
        .iter()
        .map(|&(x, y)| corner(&reference, x, y))
        .collect::<std::collections::HashSet<_>>();
    assert_eq!(distinct.len(), 4, "corners must differ: {distinct:?}");
    for (x, y) in corners {
        let (a, b) = (corner(&reference, x, y), corner(&got, x, y));
        assert!(
            (0..3).all(|c| (a[c] - b[c]).abs() <= 12),
            "({x},{y}): {a:?} vs {b:?}"
        );
    }
}

#[test]
fn screens_draw_their_slices() {
    use verso_core::output::{OutputSetup, Slice, rect};
    use verso_render::ScreenRenderer;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut screens = ScreenRenderer::new(&gpu.device);

    let mut setup = OutputSetup {
        canvas: (16, 8),
        ..Default::default()
    };
    // Screen A: the composition on its left half only.
    let a = setup.add_screen("A", (16, 8));
    setup.screens[0].slices = vec![Slice::new(a + 10, "Left", rect(16.0, 8.0), rect(8.0, 8.0))];
    // Screen B: whole, color corrected from red to green.
    let b = setup.add_screen("B", (4, 4));
    setup.screens[1].color.rgb = [-1.0, 1.0, 0.0];
    // A disabled screen is not rendered.
    let c = setup.add_screen("C", (4, 4));
    setup.screens[2].enabled = false;

    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
    let created = screens.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &setup,
        &renderer,
        &comp,
    );
    assert_eq!(created, [a, b]);
    assert!(screens.texture(c).is_none());
    assert_eq!(screens.size(b), Some((4, 4)));

    let pixels = read_back(&gpu, encoder, screens.texture(a).unwrap(), (16, 8));
    let at = |x: usize, y: usize| {
        let i = (y * 16 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    assert_eq!(at(2, 4), [255, 0, 0]);
    assert_eq!(at(12, 4), [0, 0, 0]);

    let encoder = gpu.device.create_command_encoder(&Default::default());
    let pixels = read_back(&gpu, encoder, screens.texture(b).unwrap(), (4, 4));
    assert_eq!(first_pixel(&pixels), [0, 255, 0]);
}

#[test]
fn highlighted_slice_is_tinted() {
    use verso_core::output::{OutputSetup, Slice, rect};
    use verso_render::ScreenRenderer;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut screens = ScreenRenderer::new(&gpu.device);
    let mut setup = OutputSetup {
        canvas: (16, 8),
        ..Default::default()
    };
    let a = setup.add_screen("A", (16, 8));
    setup.screens[0].slices = vec![
        Slice::new(a + 10, "Left", rect(16.0, 8.0), rect(8.0, 8.0)),
        Slice::new(
            a + 11,
            "Right",
            rect(16.0, 8.0),
            [[8.0, 0.0], [16.0, 0.0], [16.0, 8.0], [8.0, 8.0]],
        ),
    ];
    screens.set_highlight(Some(a + 11));

    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
    screens.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &setup,
        &renderer,
        &comp,
    );
    let pixels = read_back(&gpu, encoder, screens.texture(a).unwrap(), (16, 8));
    let at = |x: usize, y: usize| {
        let i = (y * 16 + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };
    // The other slice is untouched; the highlighted one turns toward cyan.
    assert_eq!(at(2, 4), [255, 0, 0]);
    let tinted = at(12, 4);
    assert!(tinted[0] < 255 && tinted[2] > 0, "{tinted:?}");
}

#[test]
fn soft_edges_blend_overlapping_slices() {
    use verso_core::output::{OutputSetup, Slice, rect};
    use verso_render::ScreenRenderer;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut screens = ScreenRenderer::new(&gpu.device);
    // Two 16 px screens of a 28 px composition: inputs 0–16 and 12–28
    // overlap on 12–16.
    let mut setup = OutputSetup {
        canvas: (28, 8),
        ..Default::default()
    };
    let a = setup.add_screen("A", (16, 8));
    let b = setup.add_screen("B", (16, 8));
    let input = |x0: f32| [[x0, 0.0], [x0 + 16.0, 0.0], [x0 + 16.0, 8.0], [x0, 8.0]];
    let mut left = Slice::new(a + 10, "L", input(0.0), rect(16.0, 8.0));
    left.soft_edge = true;
    let mut right = Slice::new(b + 10, "R", input(12.0), rect(16.0, 8.0));
    right.soft_edge = true;
    setup.screens[0].slices = vec![left];
    setup.screens[1].slices = vec![right];

    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
    screens.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &setup,
        &renderer,
        &comp,
    );
    let left = read_back(&gpu, encoder, screens.texture(a).unwrap(), (16, 8));
    let encoder = gpu.device.create_command_encoder(&Default::default());
    let right = read_back(&gpu, encoder, screens.texture(b).unwrap(), (16, 8));
    let red = |pixels: &[u8], x: usize| pixels[(4 * 16 + x) * 4];
    // Outside the overlap, full red.
    assert_eq!(red(&left, 2), 255);
    assert_eq!(red(&right, 13), 255);
    // In the overlap (composition x 12–16), each screen fades out toward
    // its edge, and the light of both adds up to about full red.
    let linear = |v: u8| (v as f32 / 255.0).powf(2.2);
    let (l, r) = (red(&left, 15), red(&right, 3));
    assert!(l < 128 && r > 200, "left {l}, right {r}");
    for x in 12..16 {
        let sum = linear(red(&left, x)) + linear(red(&right, x - 12));
        assert!((sum - 1.0).abs() < 0.08, "x {x}: {sum}");
    }
}

#[test]
fn test_patterns_replace_the_slices() {
    use verso_core::output::{OutputSetup, Slice, rect};
    use verso_render::{Pattern, ScreenRenderer};
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let comp = solids(&[(RED, BlendMode::Alpha, 1.0)]);
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    let mut screens = ScreenRenderer::new(&gpu.device);
    let (w, h) = (240usize, 120usize);
    let mut setup = OutputSetup {
        canvas: (w as u32, h as u32),
        ..Default::default()
    };
    let size = (w as f32, h as f32);
    let ids: Vec<u64> = (0..4)
        .map(|i| {
            let id = setup.add_screen(&format!("S{i}"), (w as u32, h as u32));
            setup.screens[i].slices = vec![Slice::new(
                id + 10,
                "All",
                rect(size.0, size.1),
                rect(size.0, size.1),
            )];
            id
        })
        .collect();
    let mut render = |pattern: Pattern| {
        screens.set_patterns(ids.iter().map(|&id| (id, pattern)).collect());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
        screens.render(
            &gpu.device,
            &gpu.queue,
            &mut encoder,
            &setup,
            &renderer,
            &comp,
        );
        read_back(
            &gpu,
            encoder,
            screens.texture(ids[0]).unwrap(),
            (w as u32, h as u32),
        )
    };
    let at = |pixels: &[u8], x: usize, y: usize| {
        let i = (y * w + x) * 4;
        [pixels[i], pixels[i + 1], pixels[i + 2]]
    };

    // Grid: red border, white centre axes, dark cells; no red composition.
    let grid = render(Pattern::Grid);
    let border = at(&grid, 1, 60);
    assert!(border[0] > 240 && border[1] < 140, "{border:?}");
    assert_eq!(at(&grid, 120, 30), [255, 255, 255]);
    assert!(at(&grid, 60, 80)[0] < 30, "{:?}", at(&grid, 60, 80));

    // Colors: white first bar, black last bar, grey bottom.
    let colors = render(Pattern::Colors);
    assert_eq!(at(&colors, 5, 20), [255, 255, 255]);
    assert_eq!(at(&colors, w - 5, 20), [0, 0, 0]);
    let grey = at(&colors, 120, h - 5);
    assert!(grey.iter().all(|&v| (120..136).contains(&v)), "{grey:?}");

    // Gradient: dark on the left, bright on the right.
    let gradient = render(Pattern::Gradient);
    assert!(at(&gradient, 2, 20)[0] < 20 && at(&gradient, w - 2, 20)[0] > 235);

    // Slices: the slice is filled with its color, not with the composition.
    let slices = render(Pattern::Slices);
    assert_ne!(at(&slices, 10, 10), [0, 0, 0]);
    assert_ne!(at(&slices, 10, 10), [255, 0, 0]);

    // Identify: a colored border and a white digit in the middle.
    let identify = render(Pattern::Identify { number: 1 });
    let border = at(&identify, 5, 60);
    assert!(border.iter().any(|&v| v > 200), "{border:?}");
    // "1" lights only its right segments, so look over the middle third.
    let lit = (w / 3..2 * w / 3).any(|x| (0..h).any(|y| at(&identify, x, y) == [255, 255, 255]));
    assert!(lit, "no digit drawn");

    // Without patterns, the slices are back.
    screens.set_patterns(Vec::new());
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
    screens.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &setup,
        &renderer,
        &comp,
    );
    let plain = read_back(
        &gpu,
        encoder,
        screens.texture(ids[0]).unwrap(),
        (w as u32, h as u32),
    );
    assert_eq!(at(&plain, 10, 10), [255, 0, 0]);
}

#[test]
fn slices_show_layers_and_groups() {
    use verso_core::output::OutputSetup;
    use verso_render::ScreenRenderer;
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    // Red layer 1, green layer 2 at half opacity in a group, blue layer 3.
    let mut comp = solids(&[
        (RED, BlendMode::Alpha, 1.0),
        (GREEN, BlendMode::Alpha, 0.5),
        ([0.0, 0.0, 1.0, 1.0], BlendMode::Alpha, 1.0),
    ]);
    comp.apply(Command::AddGroup { layers: vec![1] }).unwrap();
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer.keep_group_images(true);
    let mut screens = ScreenRenderer::new(&gpu.device);

    let mut setup = OutputSetup {
        canvas: (16, 8),
        ..Default::default()
    };
    let sources = ["0:1", "1:1", "3:1", "1:9"];
    let ids: Vec<u64> = sources
        .iter()
        .map(|source| {
            let id = setup.add_screen(source, (4, 4));
            setup.screens.last_mut().unwrap().slices[0].source = (*source).into();
            id
        })
        .collect();

    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.render(&gpu.device, &gpu.queue, &mut encoder, &comp, &[], 0.0);
    screens.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &setup,
        &renderer,
        &comp,
    );
    gpu.queue.submit([encoder.finish()]);
    let pixel = |id: u64| {
        let encoder = gpu.device.create_command_encoder(&Default::default());
        first_pixel(&read_back(
            &gpu,
            encoder,
            screens.texture(id).unwrap(),
            (4, 4),
        ))
    };
    // The composition is the top layer, blue.
    assert_eq!(pixel(ids[0]), [0, 0, 255]);
    assert_eq!(pixel(ids[1]), [255, 0, 0]);
    // The group holds green at half opacity, over black.
    let group = pixel(ids[2]);
    assert!(close(group, [0, 188, 0]), "{group:?}");
    // A layer that does not exist shows black.
    assert_eq!(pixel(ids[3]), [0, 0, 0]);
}

#[test]
fn milkdrop_clips_on_one_layer_blend() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let dir = std::env::temp_dir().join("verso-render-test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = milk_family(&dir);
    // Red preset in column 1, blue in column 2, on the same layer.
    let mut comp = Composition::new("t", 1, 2);
    comp.width = 64;
    comp.height = 36;
    for (column, variant) in [(0, 0), (1, 1)] {
        let mut source = Source::milkdrop("Test/milk-family", variant);
        if let Source::Milkdrop { blend, .. } = &mut source {
            *blend = 1.0;
        }
        comp.apply(Command::SetClip {
            layer: 0,
            column,
            clip: Some(Clip::new("Milk", source)),
        })
        .unwrap();
    }
    // A layer crossfade is set; between MilkDrop clips the preset blend wins.
    comp.layers[0].transition = 5.0;
    comp.apply(Command::TriggerClip {
        layer: 0,
        column: 0,
    })
    .unwrap();
    let mut renderer = Renderer::new(&gpu.device, (comp.width, comp.height));
    renderer
        .milkdrop_mut()
        .library_mut()
        .load_file(&path)
        .unwrap();
    let mut t = 0.0;
    let mut frame = |renderer: &mut Renderer, comp: &Composition| {
        t += 0.1;
        mean_rgb(&render_frame(&gpu, renderer, comp, &[], t))
    };
    for _ in 0..4 {
        frame(&mut renderer, &comp);
    }
    comp.apply(Command::TriggerClip {
        layer: 0,
        column: 1,
    })
    .unwrap();
    let mut seen_mix = false;
    for _ in 0..8 {
        let c = frame(&mut renderer, &comp);
        seen_mix |= c[0] > 20.0 && c[2] > 20.0;
    }
    assert!(seen_mix, "no frame mixed both presets");
    // Done after the 1 s MilkDrop blend, well before the 5 s crossfade.
    for _ in 0..4 {
        frame(&mut renderer, &comp);
    }
    let blue = frame(&mut renderer, &comp);
    assert!(blue[2] > 200.0 && blue[0] < 20.0, "{blue:?}");
}

#[test]
fn outgoing_clip_keeps_playing_during_transition() {
    let Some(gpu) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };
    let mut comp = Composition::new("fade", 1, 2);
    comp.width = 32;
    comp.height = 16;
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(Clip::new(
            "plasma",
            Source::Generator {
                kind: verso_core::Generator::Plasma,
                color: WHITE,
                speed: 20.0,
            },
        )),
    })
    .unwrap();
    comp.apply(Command::SetClip {
        layer: 0,
        column: 1,
        clip: solid([0.0, 0.0, 0.0, 0.0]),
    })
    .unwrap();
    // A fade so long that its progress barely moves between the frames.
    comp.apply(Command::SetLayerTransition {
        layer: 0,
        seconds: 30.0,
    })
    .unwrap();
    let mut renderer = Renderer::new(&gpu.device, (32, 16));
    comp.apply(Command::TriggerColumn(0)).unwrap();
    render_frame(&gpu, &mut renderer, &comp, &[], 0.0);
    comp.apply(Command::TriggerColumn(1)).unwrap();
    let a = render_frame(&gpu, &mut renderer, &comp, &[], 0.50);
    let b = render_frame(&gpu, &mut renderer, &comp, &[], 0.55);
    let difference: u32 = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y) as u32).sum();
    // A frozen outgoing image would only change by the fade step (0.2 %).
    assert!(
        difference > 32 * 16 * 4,
        "outgoing generator froze: {difference}"
    );
}
