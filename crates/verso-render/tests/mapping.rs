//! Mapping slices on the GPU: input rectangle, warp, soft edges and masks.
//! Skips without a GPU adapter.

use verso_core::mapping::{EdgeBlend, Mask, Rect, Screen, Slice, Warp};
use verso_render::MediaTexture;
use verso_render::mapping::ScreenRenderer;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// A 32×16 source: left half red, right half green.
fn source(device: &wgpu::Device, queue: &wgpu::Queue) -> MediaTexture {
    let mut pixels = Vec::new();
    for _y in 0..16 {
        for x in 0..32 {
            pixels.extend_from_slice(if x < 16 {
                &[255, 0, 0, 255]
            } else {
                &[0, 255, 0, 255]
            });
        }
    }
    let mut texture = MediaTexture::new(device, (32, 16));
    texture.upload(device, queue, (32, 16), &pixels);
    texture
}

/// Renders `slice` alone on a 64×32 screen and reads it back.
fn render(device: &wgpu::Device, queue: &wgpu::Queue, slice: Slice) -> Vec<[u8; 4]> {
    let source = source(device, queue);
    let mut screen = Screen::new("test", (64, 32));
    screen.slices = vec![slice];
    let mut renderer = ScreenRenderer::new(device, queue, screen.size);
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(device, queue, &mut encoder, &screen, source.view());
    let row = (64 * 4u32).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * 32) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        renderer.texture().as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(32),
            },
        },
        wgpu::Extent3d {
            width: 64,
            height: 32,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    data.chunks(row as usize)
        .flat_map(|r| r[..64 * 4].chunks(4).map(|p| [p[0], p[1], p[2], p[3]]))
        .collect()
}

fn at(pixels: &[[u8; 4]], x: usize, y: usize) -> [u8; 4] {
    pixels[y * 64 + x]
}

#[test]
fn slices_pick_warp_fade_and_mask() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU adapter, skipping");
        return;
    };

    // Input: the right half only, on the whole screen.
    let mut slice = Slice::full("right");
    slice.input = Rect {
        x: 0.5,
        y: 0.0,
        w: 0.5,
        h: 1.0,
    };
    let pixels = render(&device, &queue, slice);
    assert_eq!(&at(&pixels, 2, 16)[..3], &[0, 255, 0]);
    assert_eq!(&at(&pixels, 61, 16)[..3], &[0, 255, 0]);

    // Warp: the whole source into the top-left quarter.
    let mut slice = Slice::full("corner");
    slice.warp = Warp::grid(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 0.5,
            h: 0.5,
        },
        2,
        2,
    );
    let pixels = render(&device, &queue, slice);
    assert_eq!(&at(&pixels, 4, 4)[..3], &[255, 0, 0]);
    assert_eq!(&at(&pixels, 28, 4)[..3], &[0, 255, 0]);
    assert_eq!(&at(&pixels, 48, 24)[..3], &[0, 0, 0], "outside the warp");

    // Soft right edge over half the width: full on the left, darker and
    // darker towards the right edge.
    let mut slice = Slice::full("blend");
    slice.blend = EdgeBlend {
        right: 0.5,
        gamma: 1.0,
        ..Default::default()
    };
    let pixels = render(&device, &queue, slice);
    assert_eq!(at(&pixels, 20, 16)[0], 255);
    let ramp: Vec<u8> = (38..64).step_by(5).map(|x| at(&pixels, x, 16)[1]).collect();
    assert!(ramp.windows(2).all(|w| w[0] > w[1]), "{ramp:?}");
    assert!(at(&pixels, 63, 16)[1] < 20);

    // Mask: only the left half of the screen shows; inverted, the right.
    let left = Mask {
        points: vec![[0.0, 0.0], [0.5, 0.0], [0.5, 1.0], [0.0, 1.0]],
        invert: false,
    };
    let mut slice = Slice::full("mask");
    slice.mask = Some(left.clone());
    let pixels = render(&device, &queue, slice);
    assert_eq!(&at(&pixels, 8, 16)[..3], &[255, 0, 0]);
    assert_eq!(&at(&pixels, 56, 16)[..3], &[0, 0, 0]);
    let mut slice = Slice::full("inverted");
    slice.mask = Some(Mask {
        invert: true,
        ..left
    });
    let pixels = render(&device, &queue, slice);
    assert_eq!(&at(&pixels, 8, 16)[..3], &[0, 0, 0]);
    assert_eq!(&at(&pixels, 56, 16)[..3], &[0, 255, 0]);
}
