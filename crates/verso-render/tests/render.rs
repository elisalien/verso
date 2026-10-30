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

    let (w, h) = renderer.size();
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        renderer.output_texture().as_image_copy(),
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
