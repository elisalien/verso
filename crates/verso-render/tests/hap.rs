//! HAP frames uploaded as block-compressed textures render like FFmpeg's
//! own decoding of the same file. Skips without FFmpeg (with its HAP
//! encoder) or a GPU with BC texture support.

use verso_core::{Clip, Command, Composition, Source};
use verso_media::{hap, mov};
use verso_render::{MediaFormat, MediaTexture, Renderer};

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    if !adapter
        .features()
        .contains(wgpu::Features::TEXTURE_COMPRESSION_BC)
    {
        return None;
    }
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::TEXTURE_COMPRESSION_BC,
        ..Default::default()
    }))
    .ok()
}

fn ffmpeg() -> std::process::Command {
    let mut cmd = std::process::Command::new(verso_media::tool_path("ffmpeg"));
    cmd.args(["-v", "error", "-y"]);
    cmd
}

/// A 64×32 HAP clip with four flat color quadrants (flat areas survive
/// block compression), and FFmpeg's RGBA decoding of its first frame.
fn clip(format: &str) -> Option<(std::path::PathBuf, Vec<u8>)> {
    let dir = std::env::temp_dir().join("verso-hap-render");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{format}.mov"));
    let source = "color=c=0xE03020:s=32x16,format=rgba[a];color=c=0x20C040:s=32x16,format=rgba[b];\
                  color=c=0x3050E0:s=32x16,format=rgba[c];color=c=0xF0F0F0:s=32x16,format=rgba[d];\
                  [a][b]hstack[top];[c][d]hstack[bottom];[top][bottom]vstack,trim=end_frame=1";
    let ok = ffmpeg()
        .args(["-f", "lavfi", "-i", "nullsrc", "-filter_complex", source])
        .args(["-frames:v", "1", "-c:v", "hap", "-format", format])
        .arg(&path)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return None;
    }
    let out = ffmpeg()
        .arg("-i")
        .arg(&path)
        .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgba", "-"])
        .output()
        .ok()?;
    (out.status.success() && out.stdout.len() == 64 * 32 * 4).then_some((path, out.stdout))
}

fn render(device: &wgpu::Device, queue: &wgpu::Queue, texture: &MediaTexture) -> Vec<u8> {
    let mut comp = Composition::new("hap", 1, 1);
    (comp.width, comp.height) = (64, 32);
    comp.apply(Command::SetClip {
        layer: 0,
        column: 0,
        clip: Some(Clip::new(
            "hap",
            Source::Video {
                path: "hap.mov".into(),
                speed: 1.0,
                looping: true,
            },
        )),
    })
    .unwrap();
    comp.apply(Command::TriggerColumn(0)).unwrap();
    let mut renderer = Renderer::new(device, (64, 32));
    let mut encoder = device.create_command_encoder(&Default::default());
    renderer.render(
        device,
        queue,
        &mut encoder,
        &comp,
        &[Some(texture.media_view())],
        0.0,
    );
    let mut grabber = verso_render::FrameGrabber::new(device, (64, 32));
    grabber.capture(&mut encoder, renderer.output_texture());
    queue.submit([encoder.finish()]);
    grabber.after_submit();
    for _ in 0..100 {
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        if let Some(frame) = grabber.poll(device) {
            return frame.data;
        }
    }
    panic!("no frame read back");
}

#[test]
fn hap_textures_match_ffmpeg() {
    let Some((device, queue)) = gpu() else {
        eprintln!("no GPU with BC textures, skipping");
        return;
    };
    for (format, expected) in [
        ("hap", MediaFormat::Bc1),
        ("hap_alpha", MediaFormat::Bc3),
        ("hap_q", MediaFormat::YCoCgBc3),
    ] {
        let Some((path, reference)) = clip(format) else {
            eprintln!("ffmpeg with HAP not available, skipping");
            return;
        };
        let track = mov::video_track(&path).unwrap();
        let mut file = std::fs::File::open(&path).unwrap();
        let (mut frame, mut data) = (Vec::new(), Vec::new());
        track.read(&mut file, 0, &mut frame).unwrap();
        let decoded = hap::decode(&frame, track.width, track.height, &mut data).unwrap();
        let media_format = match decoded {
            hap::HapFormat::Bc1 => MediaFormat::Bc1,
            hap::HapFormat::Bc3 => MediaFormat::Bc3,
            hap::HapFormat::YCoCgBc3 => MediaFormat::YCoCgBc3,
            hap::HapFormat::Bc7 => MediaFormat::Bc7,
        };
        assert_eq!(media_format, expected);
        let mut texture = MediaTexture::with_format(&device, (64, 32), media_format);
        texture.upload_as(&device, &queue, (64, 32), media_format, &data);
        let pixels = render(&device, &queue, &texture);
        // Compare the middle of each quadrant.
        for (x, y) in [(16, 8), (48, 8), (16, 24), (48, 24)] {
            let i = (y * 64 + x) * 4;
            for c in 0..3 {
                let (got, want) = (pixels[i + c], reference[i + c]);
                assert!(
                    got.abs_diff(want) <= 6,
                    "{format} at ({x},{y}) channel {c}: {got} vs FFmpeg {want}"
                );
            }
        }
    }
}
