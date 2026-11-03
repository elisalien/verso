//! Performance bench: compositing, video decoding and the full video path.
//!
//! ```bash
//! cargo run --release -p verso-app --example bench
//! ```
//!
//! 1. Compositing: 1, 4 and 8 generator layers with a blur each, 1080p.
//! 2. Decoding: frames per second of 1080p H.264 and HAP clips, one and four
//!    players at once (FFmpeg on `PATH`).
//! 3. Full path: four HAP 1080p layers decoded, uploaded and composited —
//!    the v0.2 exit criterion is 60 fps.
//!
//! Numbers depend on the machine: run it on the show computer.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::wgpu;
use verso_core::{
    BlendMode, Clip, Command, Composition, Effect, EffectEdit, EffectTarget, Generator, Source,
};
use verso_media::VideoPlayer;
use verso_render::{MediaTexture, Renderer};

const SIZE: (u32, u32) = (1920, 1080);

fn gpu() -> Option<(wgpu::Device, wgpu::Queue, String)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let name = adapter.get_info().name;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).ok()?;
    Some((device, queue, name))
}

fn generators(layers: usize) -> Composition {
    let mut comp = Composition::new("bench", layers, 1);
    (comp.width, comp.height) = SIZE;
    for l in 0..layers {
        comp.layers[l].blend = BlendMode::Screen;
        comp.apply(Command::SetClip {
            layer: l,
            column: 0,
            clip: Some(Clip::new(
                "gen",
                Source::Generator {
                    kind: Generator::ALL[l % Generator::ALL.len()],
                    color: [1.0; 4],
                    speed: 1.0,
                },
            )),
        })
        .ok();
        comp.apply(Command::EditEffects {
            target: EffectTarget::Layer(l),
            edit: EffectEdit::Add(Effect::new("builtin:blur", "Blur")),
        })
        .ok();
    }
    comp.apply(Command::TriggerColumn(0)).ok();
    comp
}

/// Average milliseconds per composited frame, waiting for the GPU each time.
fn composite(device: &wgpu::Device, queue: &wgpu::Queue, layers: usize) -> f64 {
    let comp = generators(layers);
    let mut renderer = Renderer::new(device, SIZE);
    let frames = 60;
    let mut total = Duration::ZERO;
    for i in 0..frames + 5 {
        let start = Instant::now();
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(device, queue, &mut encoder, &comp, &[], i as f32 / 60.0);
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        // The first frames compile pipelines.
        if i >= 5 {
            total += start.elapsed();
        }
    }
    total.as_secs_f64() * 1000.0 / frames as f64
}

/// Encodes a 1080p test clip with `codec`, or `None` without FFmpeg.
fn clip(dir: &Path, codec: &str) -> Option<PathBuf> {
    let path = dir.join(format!("bench-{codec}.mov"));
    if path.exists() {
        return Some(path);
    }
    let ffmpeg = verso_media::tool_path("ffmpeg");
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-v", "error", "-y", "-f", "lavfi", "-i"])
        .arg("testsrc2=size=1920x1080:rate=60:duration=6")
        .args(["-c:v", codec]);
    if codec == "libx264" {
        cmd.args(["-pix_fmt", "yuv420p"]);
    }
    let ok = cmd.arg(&path).status().is_ok_and(|s| s.success());
    ok.then_some(path)
}

/// Decoded frames per second for each of `count` players reading `path`
/// together, pulled as fast as they decode.
fn decode(path: &Path, count: usize, seconds: f64) -> Vec<f64> {
    let mut players: Vec<VideoPlayer> = (0..count)
        .filter_map(|_| VideoPlayer::open(path, SIZE).ok())
        .collect();
    let mut frames = vec![0u64; players.len()];
    let start = Instant::now();
    while start.elapsed().as_secs_f64() < seconds {
        let mut any = false;
        for (player, n) in players.iter_mut().zip(&mut frames) {
            let fps = player.info().fps;
            if let Ok(Some(_)) = player.advance(1.0 / fps, 1.0, true) {
                *n += 1;
                any = true;
            }
        }
        if !any {
            std::thread::sleep(Duration::from_micros(200));
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    frames.iter().map(|&n| n as f64 / elapsed).collect()
}

/// Frames per second of decoding, uploading and compositing `count` layers
/// of `path`, running as fast as possible.
fn full_path(device: &wgpu::Device, queue: &wgpu::Queue, path: &Path, count: usize) -> f64 {
    let mut comp = Composition::new("bench", count, 1);
    (comp.width, comp.height) = SIZE;
    for l in 0..count {
        comp.layers[l].blend = BlendMode::Screen;
        comp.apply(Command::SetClip {
            layer: l,
            column: 0,
            clip: Some(Clip::new(
                "video",
                Source::Video {
                    path: path.to_owned(),
                    speed: 1.0,
                    looping: true,
                },
            )),
        })
        .ok();
    }
    comp.apply(Command::TriggerColumn(0)).ok();
    let mut players: Vec<VideoPlayer> = (0..count)
        .filter_map(|_| VideoPlayer::open(path, SIZE).ok())
        .collect();
    let mut textures: Vec<MediaTexture> = (0..count)
        .map(|_| MediaTexture::new(device, SIZE))
        .collect();
    let mut renderer = Renderer::new(device, SIZE);
    let fps = players.first().map(|p| p.info().fps).unwrap_or(60.0);
    let start = Instant::now();
    let mut rendered = 0u64;
    while start.elapsed() < Duration::from_secs(5) {
        // Advance one video frame per rendered frame: the output keeps up
        // only if every layer decoded its next frame in time.
        for (player, texture) in players.iter_mut().zip(&mut textures) {
            loop {
                match player.advance(1.0 / fps, 1.0, true) {
                    Ok(Some(frame)) => {
                        texture.upload(device, queue, (frame.width, frame.height), &frame.data);
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_micros(200)),
                    Err(_) => break,
                }
            }
        }
        let views: Vec<_> = textures.iter().map(|t| Some(t.media_view())).collect();
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(
            device,
            queue,
            &mut encoder,
            &comp,
            &views,
            rendered as f32 / 60.0,
        );
        queue.submit([encoder.finish()]);
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rendered += 1;
    }
    rendered as f64 / start.elapsed().as_secs_f64()
}

fn main() {
    let dir = std::env::temp_dir().join("verso-bench");
    let _ = std::fs::create_dir_all(&dir);
    println!("Verso bench, {}×{}", SIZE.0, SIZE.1);

    let gpu = gpu();
    match &gpu {
        Some((device, queue, name)) => {
            println!("\nCompositing on {name} (generator + blur per layer)");
            for layers in [1, 4, 8] {
                let ms = composite(device, queue, layers);
                println!(
                    "  {layers} layer(s): {ms:6.2} ms/frame ({:5.0} fps)",
                    1000.0 / ms
                );
            }
        }
        None => println!("\nNo GPU adapter: compositing skipped"),
    }

    println!("\nDecoding (FFmpeg → RGBA), frames per second per player");
    let mut hap = None;
    for codec in ["libx264", "hap"] {
        let Some(path) = clip(&dir, codec) else {
            println!("  {codec}: FFmpeg or its encoder is missing, skipped");
            continue;
        };
        for count in [1, 4] {
            let rates = decode(&path, count, 4.0);
            let min = rates.iter().cloned().fold(f64::INFINITY, f64::min);
            println!("  {codec:8} × {count}: {min:6.1} fps (slowest player)");
        }
        if codec == "hap" {
            hap = Some(path);
        }
    }

    if let (Some((device, queue, _)), Some(path)) = (&gpu, hap) {
        let fps = full_path(device, queue, &path, 4);
        let verdict = if fps >= 59.0 { "meets" } else { "misses" };
        println!("\nFull path, 4 × HAP 1080p60: {fps:5.1} fps — {verdict} the 60 fps criterion");
    }
}
