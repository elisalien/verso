//! Renders MilkDrop presets offline with synthetic music, for checking and
//! for preview images:
//!
//! `cargo run --release -p verso-render --example milk_render -- [options] <files or dirs>`
//!
//! Options: `--out <dir>` (default `milk-out`), `--size <w>x<h>` (default
//! 320x180), `--seconds <s>` (default 8, at 30 fps). Each variant of each
//! family is written as `<out>/<file stem>-<variant>.png`, and a line per
//! render goes to stdout: `ok|error <png> <lit fraction> <change> <ms/frame>`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use verso_core::{BlendMode, Clip, Command, Composition, Source};
use verso_render::Renderer;

const FPS: f32 = 30.0;

/// Kick, snare, hats and a bass line at 124 BPM.
fn music(t0: f32) -> Vec<f32> {
    let rate = 44100.0;
    let beat = 60.0 / 124.0;
    (0..1024)
        .map(|i| {
            let t = t0 - (1024 - i) as f32 / rate;
            let b = t.rem_euclid(beat);
            let bar_beat = (t / beat).floor() as i64 % 4;
            let kick = (b * 2.0 * std::f32::consts::PI * (50.0 + 80.0 * (-b * 30.0).exp())).sin() * (-b * 8.0).exp();
            let snare_t = if bar_beat % 2 == 1 { b } else { 10.0 };
            let noise = ((t * 12345.678).sin() * 43758.545).fract() * 2.0 - 1.0;
            let snare = noise * (-snare_t * 18.0).exp() * 0.5;
            let hat_t = (t + beat / 2.0).rem_euclid(beat / 2.0);
            let hat = noise * (-hat_t * 60.0).exp() * 0.2;
            let note = [55.0, 55.0, 65.4, 49.0][(t / (beat * 4.0)).floor() as usize % 4];
            let bass = (t * 2.0 * std::f32::consts::PI * note).sin() * 0.25;
            ((kick * 0.8 + snare + hat + bass) * 0.8).clamp(-1.0, 1.0)
        })
        .collect()
}

fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &Renderer) -> Vec<u8> {
    let (w, h) = renderer.size();
    let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (row * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
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
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data = buffer.get_mapped_range(..).unwrap();
    data.chunks(row as usize)
        .flat_map(|r| &r[..(w * 4) as usize])
        .copied()
        .collect()
}

fn collect(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(path).unwrap().flatten().map(|e| e.path()).collect();
        entries.sort();
        for p in entries {
            collect(&p, out);
        }
    } else if path.extension().is_some_and(|e| e == "json") {
        out.push(path.to_owned());
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out_dir = PathBuf::from("milk-out");
    let mut size = (320u32, 180u32);
    let mut seconds = 8.0f32;
    let mut inputs = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out_dir = args.next().unwrap().into(),
            "--size" => {
                let s = args.next().unwrap();
                let (w, h) = s.split_once('x').unwrap();
                size = (w.parse().unwrap(), h.parse().unwrap());
            }
            "--seconds" => seconds = args.next().unwrap().parse().unwrap(),
            _ => collect(Path::new(&arg), &mut inputs),
        }
    }
    std::fs::create_dir_all(&out_dir).unwrap();

    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    for path in inputs {
        let mut renderer = Renderer::new(&device, size);
        let info = match renderer.milkdrop_mut().library_mut().load_file(&path) {
            Ok(info) => info.clone(),
            Err(e) => {
                println!("error {} 0 0 0 load: {e}", path.display());
                continue;
            }
        };
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        for variant in 0..info.variants.len() {
            let png = out_dir.join(format!("{stem}-{variant}.png"));
            let mut comp = Composition::new("milk", 1, 1);
            comp.width = size.0;
            comp.height = size.1;
            let source = Source::milkdrop(&info.id, variant);
            comp.apply(Command::SetClip {
                layer: 0,
                column: 0,
                clip: Some(Clip::new("milk", source)),
            })
            .unwrap();
            comp.apply(Command::SetLayerBlend {
                layer: 0,
                blend: BlendMode::Alpha,
            })
            .unwrap();
            comp.apply(Command::TriggerColumn(0)).unwrap();

            let frames = (seconds * FPS) as usize;
            let start = Instant::now();
            let mut previous = Vec::new();
            let mut change = 0.0;
            let mut pixels = Vec::new();
            for f in 0..frames {
                let t = f as f32 / FPS;
                renderer.milkdrop_mut().set_audio(music(t), 44100.0);
                let mut encoder = device.create_command_encoder(&Default::default());
                renderer.render(&device, &queue, &mut encoder, &comp, &[], t);
                queue.submit([encoder.finish()]);
                // Compare the last two half-second steps for motion.
                if f + 1 == frames - (FPS as usize / 2) || f + 1 == frames {
                    pixels = read_back(&device, &queue, &renderer);
                    if !previous.is_empty() {
                        let diff: u64 = pixels.iter().zip(&previous).map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64).sum();
                        change = diff as f64 / pixels.len() as f64 / 255.0;
                    }
                    previous.clone_from(&pixels);
                }
            }
            let ms = start.elapsed().as_secs_f64() * 1000.0 / frames as f64;
            if let Some(e) = renderer.milkdrop().error(&info.id, variant) {
                let first = e.lines().find(|l| l.contains("error")).unwrap_or(e.lines().next().unwrap_or(""));
                println!("error {} 0 0 0 {first}", png.display());
                continue;
            }
            let lit = pixels
                .chunks(4)
                .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 45)
                .count() as f64
                / (pixels.len() / 4) as f64;
            image::save_buffer(&png, &pixels, size.0, size.1, image::ExtendedColorType::Rgba8).unwrap();
            println!("ok {} {lit:.3} {change:.4} {ms:.1}", png.display());
        }
    }
}
