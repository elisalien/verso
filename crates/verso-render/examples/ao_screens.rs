//! Renders the screens of a Resolume Advanced Output preset from the demo
//! composition, to check slices and warps against Arena:
//!
//! `cargo run -p verso-render --example ao_screens -- <preset.xml> [out dir] [pattern]`
//!
//! Writes `<out>/<screen name>.png` per rendered screen (default out dir
//! `ao-out`), and the composition itself as `<out>/composition.png`.

use std::path::{Path, PathBuf};

use verso_core::{Command, Composition};
use verso_render::{Pattern, Renderer, ScreenRenderer};

fn read_back(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    (w, h): (u32, u32),
) -> Vec<u8> {
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
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
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    data.chunks(row as usize)
        .flat_map(|r| &r[..(w * 4) as usize])
        .copied()
        .collect()
}

fn save(path: &Path, (w, h): (u32, u32), pixels: Vec<u8>) {
    image::RgbaImage::from_raw(w, h, pixels)
        .expect("size matches")
        .save(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    println!("{}", path.display());
}

fn main() {
    let mut args = std::env::args().skip(1);
    let preset = PathBuf::from(
        args.next()
            .expect("usage: ao_screens <preset.xml> [out dir]"),
    );
    let out = PathBuf::from(args.next().unwrap_or_else(|| "ao-out".into()));
    std::fs::create_dir_all(&out).unwrap();
    // Optional test pattern on every screen: grid, colors, gradient,
    // slices or identify.
    let pattern = args.next().map(|name| match name.as_str() {
        "grid" => Pattern::Grid,
        "colors" => Pattern::Colors,
        "gradient" => Pattern::Gradient,
        "slices" => Pattern::Slices,
        "identify" => Pattern::Identify { number: 0 },
        other => panic!("unknown pattern {other}"),
    });

    let text = std::fs::read_to_string(&preset).unwrap();
    let import = verso_io::resolume_ao::import(&text).unwrap();
    for warning in &import.warnings {
        eprintln!("warning: {warning}");
    }
    let setup = import.setup;

    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&Default::default())).expect("no GPU adapter");
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let mut comp = Composition::demo();
    comp.width = setup.canvas.0;
    comp.height = setup.canvas.1;
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let mut renderer = Renderer::new(&device, setup.canvas);
    let mut screens = ScreenRenderer::new(&device);
    if let Some(pattern) = pattern {
        let patterns = (setup.screens.iter().enumerate())
            .map(|(i, s)| match pattern {
                Pattern::Identify { .. } => (s.id, Pattern::Identify { number: i as u32 + 1 }),
                _ => (s.id, pattern),
            })
            .collect();
        screens.set_patterns(patterns);
    }

    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(&device, &queue, &mut encoder, &comp, &[], 1.0);
    screens.render(&device, &queue, &mut encoder, &setup, &renderer, &comp);
    queue.submit([encoder.finish()]);

    let pixels = read_back(&device, &queue, renderer.output_texture(), setup.canvas);
    save(&out.join("composition.png"), setup.canvas, pixels);
    for screen in &setup.screens {
        let (Some(texture), Some(size)) = (screens.texture(screen.id), screens.size(screen.id))
        else {
            continue;
        };
        let pixels = read_back(&device, &queue, texture, size);
        let name: String = screen
            .name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '_' })
            .collect();
        save(&out.join(format!("{name}.png")), size, pixels);
    }
}
