//! Renders compositions on the GPU and checks output pixels.
//! Tests pass without running when no adapter is available (e.g. headless CI).

use verso_core::{BlendMode, Clip, Command, Composition, Source};
use verso_render::{MediaTexture, MediaView, Renderer};

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
    Some(Clip {
        name: "Solid".into(),
        source: Source::Solid { color },
    })
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
    let clip = Clip {
        name: "Image".into(),
        source: Source::Image {
            path: "unused.png".into(),
        },
    };
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
