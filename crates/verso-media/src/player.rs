//! Real-time video playback from an `ffmpeg` child process.
//!
//! Restarting a clip (a retrigger, or a loop reaching its end) must not show a
//! frozen frame while a new `ffmpeg` starts, so a *standby* decoder is kept
//! ready on the first frame. Restarting swaps it in and spawns the next
//! standby on a worker thread. The cost is one extra `ffmpeg` process per
//! playing clip, blocked on a full pipe.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, sync_channel};
use std::thread;

use crate::{Error, Frame, Result, VideoInfo, command, fit_size, input, probe, tool_path};

/// Decoded frames buffered ahead of playback.
const QUEUE: usize = 6;

enum Message {
    Frame(Vec<u8>),
    End,
}

struct Decoder {
    child: Child,
    frames: Receiver<Message>,
}

impl Decoder {
    /// Starts decoding `path` from `start` seconds.
    fn spawn(path: &Path, size: (u32, u32), start: f64) -> Result<Self> {
        let program = tool_path("ffmpeg");
        let mut cmd = command(&program);
        cmd.args(["-v", "error", "-hwaccel", "auto"]);
        if start > 0.0 {
            // Before the input: fast seek to the nearest key frame, then
            // decode up to `start`.
            cmd.arg("-ss").arg(format!("{start:.3}"));
        }
        input(&mut cmd, path)?;
        let mut child = cmd
            .args(["-an", "-vf"])
            .arg(format!("scale={}:{}", size.0, size.1))
            .args(["-f", "rawvideo", "-pix_fmt", "rgba", "-"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|source| Error::Spawn {
                program: program.clone(),
                source,
            })?;
        let stdout = child.stdout.take().expect("stdout is piped");
        let (sender, frames) = sync_channel(QUEUE);
        let frame_len = size.0 as usize * size.1 as usize * 4;
        thread::Builder::new()
            .name("verso-decoder".into())
            .spawn(move || read_frames(stdout, frame_len, sender))
            .map_err(|source| Error::Spawn { program, source })?;
        Ok(Self { child, frames })
    }

    /// Spawns on a worker thread; the result arrives on the returned channel.
    fn spawn_in_background(path: PathBuf, size: (u32, u32)) -> Receiver<Result<Decoder>> {
        let (sender, receiver) = sync_channel(1);
        thread::spawn(move || {
            let _ = sender.send(Decoder::spawn(&path, size, 0.0));
        });
        receiver
    }
}

fn read_frames(mut stdout: impl Read, frame_len: usize, sender: SyncSender<Message>) {
    loop {
        let mut data = vec![0; frame_len];
        if stdout.read_exact(&mut data).is_err() {
            let _ = sender.send(Message::End);
            return;
        }
        // The player dropped the receiver: stop reading.
        if sender.send(Message::Frame(data)).is_err() {
            return;
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The next decoder, ready to take over on restart.
enum Standby {
    Spawning(Receiver<Result<Decoder>>),
    Ready(Decoder),
    /// Spawning failed; the next restart spawns synchronously and reports.
    Failed,
}

/// Where a clip is in its playback, for the interface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transport {
    /// Seconds since the first frame of this run.
    pub position: f64,
    /// Seconds, when the container reports it.
    pub duration: Option<f64>,
    /// Source frames shown since the first frame of this run.
    pub frame: u64,
    pub ended: bool,
}

impl Transport {
    /// 0 to 1 through the clip, when the duration is known.
    pub fn progress(&self) -> Option<f32> {
        let duration = self.duration.filter(|d| *d > 0.0)?;
        Some((self.position / duration).clamp(0.0, 1.0) as f32)
    }
}

/// Plays one video file. Call [`VideoPlayer::advance`] once per rendered frame.
pub struct VideoPlayer {
    path: PathBuf,
    info: VideoInfo,
    size: (u32, u32),
    decoder: Decoder,
    standby: Standby,
    /// Playback position in source frames since the decoder started.
    position: f64,
    /// Frames received from the current decoder.
    received: u64,
    current: Option<Frame>,
    ended: bool,
}

impl VideoPlayer {
    /// Opens `path`, scaled to fit in `max` pixels.
    pub fn open(path: &Path, max: (u32, u32)) -> Result<Self> {
        let info = probe(path)?;
        let size = fit_size((info.width, info.height), max);
        Ok(Self {
            path: path.to_owned(),
            info,
            size,
            decoder: Decoder::spawn(path, size, 0.0)?,
            standby: Standby::Spawning(Decoder::spawn_in_background(path.to_owned(), size)),
            position: 0.0,
            received: 0,
            current: None,
            ended: false,
        })
    }

    pub fn info(&self) -> VideoInfo {
        self.info
    }

    /// Decoded frame size.
    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Whether a non-looping clip reached its end.
    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Position and duration for the interface.
    pub fn transport(&self) -> Transport {
        // The frame on screen is the last one received.
        let shown = self.received.saturating_sub(1);
        Transport {
            position: shown as f64 / self.info.fps,
            duration: self.info.duration,
            frame: shown,
            ended: self.ended,
        }
    }

    /// Restarts from the first frame, taking the standby decoder so no frame
    /// is lost waiting for a new process. When the standby is still starting,
    /// the restart waits for it rather than launching a third process.
    pub fn restart(&mut self) -> Result<()> {
        let standby = std::mem::replace(&mut self.standby, Standby::Failed);
        let next = match standby {
            Standby::Ready(decoder) => Ok(decoder),
            Standby::Spawning(receiver) => match receiver.recv() {
                Ok(result) => result,
                Err(_) => Decoder::spawn(&self.path, self.size, 0.0),
            },
            Standby::Failed => Decoder::spawn(&self.path, self.size, 0.0),
        };
        self.decoder = next?;
        self.standby =
            Standby::Spawning(Decoder::spawn_in_background(self.path.clone(), self.size));
        self.position = 0.0;
        self.received = 0;
        self.ended = false;
        Ok(())
    }

    /// Jumps to `seconds`. FFmpeg restarts there, so this takes a moment with
    /// long-GOP codecs; HAP files use [`crate::HapPlayer`] instead. Jumping to
    /// the start takes the standby decoder.
    pub fn seek(&mut self, seconds: f64) -> Result<()> {
        let end = self.info.duration.unwrap_or(f64::INFINITY);
        let seconds = if seconds.is_finite() {
            seconds.clamp(0.0, (end - 1.0 / self.info.fps).max(0.0))
        } else {
            0.0
        };
        if seconds == 0.0 {
            return self.restart();
        }
        self.decoder = Decoder::spawn(&self.path, self.size, seconds)?;
        self.position = seconds * self.info.fps;
        self.received = self.position.floor() as u64;
        self.ended = false;
        Ok(())
    }

    /// Moves the standby from "spawning" to "ready" once its process is up,
    /// without blocking.
    fn poll_standby(&mut self) {
        if let Standby::Spawning(receiver) = &self.standby {
            match receiver.try_recv() {
                Ok(Ok(decoder)) => self.standby = Standby::Ready(decoder),
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => self.standby = Standby::Failed,
                Err(TryRecvError::Empty) => {}
            }
        }
    }

    /// Position of the shown frame, in seconds.
    pub fn position(&self) -> f64 {
        self.received.saturating_sub(1) as f64 / self.info.fps
    }

    /// Moves playback forward by `dt` seconds at `speed`, and returns the frame
    /// to show when it changed since the last call.
    ///
    /// When decoding is slower than playback, the newest available frame is
    /// shown; playback does not skip ahead of the decoder.
    pub fn advance(&mut self, dt: f64, speed: f64, looping: bool) -> Result<Option<&Frame>> {
        self.poll_standby();
        if !self.ended {
            self.position += dt * speed.max(0.0) * self.info.fps;
        }
        let mut changed = false;
        // Frame `n` is due once the position passes `n`.
        while (self.received as f64) <= self.position && !self.ended {
            match self.decoder.frames.try_recv() {
                Ok(Message::Frame(data)) => {
                    self.received += 1;
                    self.current = Some(Frame {
                        width: self.size.0,
                        height: self.size.1,
                        data,
                    });
                    changed = true;
                }
                Ok(Message::End) | Err(TryRecvError::Disconnected) => {
                    if self.received == 0 {
                        // Nothing decoded at all: not a playable video.
                        self.ended = true;
                        return Err(Error::NoVideo(self.path.display().to_string()));
                    }
                    if looping {
                        self.restart()?;
                    } else {
                        self.ended = true;
                    }
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        // Do not let the clock run far ahead of a slow decoder.
        self.position = self.position.min(self.received as f64 + 1.0);
        Ok(if changed { self.current.as_ref() } else { None })
    }

    /// Latest decoded frame.
    pub fn frame(&self) -> Option<&Frame> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Encodes a short test clip with FFmpeg; `None` when FFmpeg is missing.
    fn test_clip(name: &str, frames: u32) -> Option<PathBuf> {
        encode_clip(name, frames, &["-pix_fmt", "yuv420p"])
    }

    /// Same as [`test_clip`], with the encoder options in `codec`.
    fn encode_clip(name: &str, frames: u32, codec: &[&str]) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join("verso-media-test");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let status = command(&tool_path("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc=size=320x180:rate=25:duration={}",
                frames as f64 / 25.0
            ))
            .args(codec)
            .arg(&path)
            .status();
        match status {
            Ok(status) if status.success() => Some(path),
            other => {
                eprintln!("test clip encoding failed: {other:?}");
                None
            }
        }
    }

    /// Advances in real time until `done` or a timeout.
    fn play_until(
        player: &mut VideoPlayer,
        looping: bool,
        mut done: impl FnMut(&VideoPlayer) -> bool,
    ) {
        let start = Instant::now();
        let mut last = Instant::now();
        while start.elapsed() < Duration::from_secs(10) && !done(player) {
            std::thread::sleep(Duration::from_millis(5));
            let now = Instant::now();
            player
                .advance((now - last).as_secs_f64(), 1.0, looping)
                .unwrap();
            last = now;
        }
    }

    #[test]
    fn probes_and_plays_to_the_end() {
        let Some(path) = test_clip("ten.mp4", 10) else {
            eprintln!("ffmpeg not available, skipping");
            return;
        };
        let info = probe(&path).unwrap();
        assert_eq!((info.width, info.height), (320, 180));
        assert!((info.fps - 25.0).abs() < 0.01);

        let mut player = VideoPlayer::open(&path, (160, 90)).unwrap();
        assert_eq!(player.size(), (160, 90));
        play_until(&mut player, false, |p| p.ended());
        assert!(player.ended());
        assert_eq!(player.received, 10);
        let frame = player.frame().unwrap();
        assert_eq!(frame.data.len(), 160 * 90 * 4);
    }

    /// Resolume DXV3 in a QuickTime file, as Alley and Arena write it.
    #[test]
    fn plays_dxv3() {
        let Some(path) = encode_clip("dxv3.mov", 5, &["-c:v", "dxv", "-format", "dxt1"]) else {
            eprintln!("ffmpeg with the DXV encoder not available, skipping");
            return;
        };
        assert_eq!(crate::MediaKind::of(&path), Some(crate::MediaKind::Video));
        let info = probe(&path).unwrap();
        assert_eq!((info.width, info.height), (320, 180));

        let mut player = VideoPlayer::open(&path, (320, 180)).unwrap();
        play_until(&mut player, false, |p| p.ended());
        assert_eq!(player.received, 5);
        let frame = player.frame().unwrap();
        assert!(frame.data.chunks(4).any(|px| px[0] > 64), "black frame");
    }

    #[test]
    fn restart_shows_a_frame_at_once() {
        let Some(path) = test_clip("restart.mp4", 25) else {
            eprintln!("ffmpeg not available, skipping");
            return;
        };
        let mut player = VideoPlayer::open(&path, (320, 180)).unwrap();
        // Play a little: the standby decoder has time to start.
        play_until(&mut player, false, |p| p.received >= 5);
        let before = player.transport();
        assert!(before.position > 0.1, "{before:?}");
        assert_eq!(before.duration.map(f64::round), Some(1.0));

        player.restart().unwrap();
        // The standby already holds the first frames: no wait for a process.
        let frame = player.advance(0.0, 1.0, false).unwrap();
        assert!(frame.is_some(), "no frame right after restart");
        assert_eq!(player.transport().frame, 0);
    }

    #[test]
    fn looping_restarts() {
        let Some(path) = test_clip("five.mp4", 5) else {
            eprintln!("ffmpeg not available, skipping");
            return;
        };
        let mut player = VideoPlayer::open(&path, (320, 180)).unwrap();
        let mut restarts = 0;
        let mut previous = 0;
        play_until(&mut player, true, |p| {
            if p.received < previous {
                restarts += 1;
            }
            previous = p.received;
            restarts >= 2
        });
        assert!(restarts >= 2, "looping clip did not restart");
        assert!(!player.ended());
    }

    #[test]
    fn poster_is_first_frame_size() {
        let Some(path) = test_clip("poster.mp4", 3) else {
            eprintln!("ffmpeg not available, skipping");
            return;
        };
        let frame = crate::poster(&path, (64, 64)).unwrap();
        assert_eq!((frame.width, frame.height), (64, 36));
    }
}

/// Throughput measurement, not a pass/fail test. Run with
/// `cargo test -p verso-media --release -- --ignored --nocapture bench`.
/// Encodes a 1080p HAP and a 1080p H.264 clip, plays four of each at once for
/// 5 seconds and prints the frame rate reached per player.
#[cfg(test)]
mod bench {
    use super::*;
    use std::time::{Duration, Instant};

    fn clip(name: &str, codec: &[&str]) -> Option<PathBuf> {
        let dir = std::env::temp_dir().join("verso-media-bench");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
        let status = command(&tool_path("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=1920x1080:rate=60:duration=12")
            .args(codec)
            .arg(&path)
            .status()
            .ok()?;
        status.success().then_some(path)
    }

    fn measure(label: &str, path: &Path, players: usize) {
        let mut all: Vec<VideoPlayer> = (0..players)
            .map(|_| VideoPlayer::open(path, (1920, 1080)).unwrap())
            .collect();
        // Let the decoders start before the clock runs.
        std::thread::sleep(Duration::from_millis(500));
        let start = Instant::now();
        let mut last = start;
        while start.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(4));
            let now = Instant::now();
            let dt = (now - last).as_secs_f64();
            last = now;
            for p in &mut all {
                p.advance(dt, 1.0, true).unwrap();
            }
        }
        let seconds = start.elapsed().as_secs_f64();
        let rates: Vec<String> = all
            .iter()
            .map(|p| format!("{:.1}", p.transport().frame as f64 / seconds))
            .collect();
        println!(
            "{label}: {players} × 1080p60, fps per player: {}",
            rates.join(" ")
        );
    }

    #[test]
    #[ignore]
    fn bench_four_1080p() {
        let hap = clip("hap.mov", &["-c:v", "hap"]);
        let dxv = clip("dxv.mov", &["-c:v", "dxv", "-format", "dxt1"]);
        let h264 = clip(
            "h264.mp4",
            &["-c:v", "libx264", "-preset", "fast", "-pix_fmt", "yuv420p"],
        );
        let (Some(hap), Some(h264)) = (hap, h264) else {
            eprintln!("ffmpeg with hap and libx264 needed, skipping");
            return;
        };
        for n in [1, 4] {
            measure("HAP", &hap, n);
            measure("H.264", &h264, n);
            if let Some(dxv) = &dxv {
                measure("DXV3", dxv, n);
            }
        }
    }
}
