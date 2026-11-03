//! Real-time video playback from an `ffmpeg` child process.

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
    fn spawn(path: &Path, size: (u32, u32)) -> Result<Self> {
        let program = tool_path("ffmpeg");
        let mut cmd = command(&program);
        cmd.args(["-v", "error", "-hwaccel", "auto"]);
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

/// Plays one video file. Call [`VideoPlayer::advance`] once per rendered frame.
pub struct VideoPlayer {
    path: PathBuf,
    info: VideoInfo,
    size: (u32, u32),
    decoder: Decoder,
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
            decoder: Decoder::spawn(path, size)?,
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

    /// Restarts from the first frame.
    pub fn restart(&mut self) -> Result<()> {
        self.decoder = Decoder::spawn(&self.path, self.size)?;
        self.position = 0.0;
        self.received = 0;
        self.ended = false;
        Ok(())
    }

    /// Moves playback forward by `dt` seconds at `speed`, and returns the frame
    /// to show when it changed since the last call.
    ///
    /// When decoding is slower than playback, the newest available frame is
    /// shown; playback does not skip ahead of the decoder.
    pub fn advance(&mut self, dt: f64, speed: f64, looping: bool) -> Result<Option<&Frame>> {
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
        let dir = std::env::temp_dir().join("verso-media-test");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        let status = command(&tool_path("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg(format!(
                "testsrc=size=320x180:rate=25:duration={}",
                frames as f64 / 25.0
            ))
            .args(["-pix_fmt", "yuv420p"])
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
