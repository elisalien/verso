//! Plays HAP QuickTime files without FFmpeg: frames are read straight from
//! the file and handed to the GPU block-compressed. Every frame is a key
//! frame, so seeking is instant and playback can run backwards.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use crate::hap::{self, HapFormat};
use crate::mov::{self, VideoTrack};
use crate::{Error, Result, VideoInfo};

/// Frames decoded ahead in the playing direction.
const AHEAD: usize = 3;

/// One frame of texture data, ready for upload.
pub struct HapFrame {
    pub index: usize,
    pub width: u32,
    pub height: u32,
    pub format: HapFormat,
    pub data: Vec<u8>,
}

/// Whether `path` is a HAP file this player reads, and its track.
pub fn probe_hap(path: &Path) -> Option<(VideoTrack, HapFormat)> {
    let track = mov::video_track(path).ok()?;
    let format = HapFormat::from_codec(&track.codec)?;
    Some((track, format))
}

pub struct HapPlayer {
    path: PathBuf,
    track: Arc<VideoTrack>,
    format: HapFormat,
    /// Playback position in frames (fractional).
    position: f64,
    requests: Sender<usize>,
    decoded: Receiver<(usize, std::result::Result<HapFrame, String>)>,
    ready: HashMap<usize, HapFrame>,
    pending: HashSet<usize>,
    current: Option<HapFrame>,
    ended: bool,
}

impl HapPlayer {
    pub fn open(path: &Path) -> Result<Self> {
        let (track, format) =
            probe_hap(path).ok_or_else(|| Error::NoVideo(path.display().to_string()))?;
        let track = Arc::new(track);
        let (requests, incoming) = channel::<usize>();
        let (results, decoded) = channel();
        let worker_track = track.clone();
        let mut file = File::open(path).map_err(|source| Error::Spawn {
            program: "HAP reader".into(),
            source,
        })?;
        thread::Builder::new()
            .name("verso-hap".into())
            .spawn(move || {
                let mut sample = Vec::new();
                for index in incoming {
                    let mut data = Vec::new();
                    let result = worker_track
                        .read(&mut file, index, &mut sample)
                        .map_err(|e| e.to_string())
                        .and_then(|()| {
                            hap::decode(&sample, worker_track.width, worker_track.height, &mut data)
                                .map_err(|e| e.to_string())
                        })
                        .map(|format| HapFrame {
                            index,
                            width: worker_track.width,
                            height: worker_track.height,
                            format,
                            data,
                        });
                    if results.send((index, result)).is_err() {
                        return;
                    }
                }
            })
            .map_err(|source| Error::Spawn {
                program: "HAP reader".into(),
                source,
            })?;
        let mut player = Self {
            path: path.to_owned(),
            track,
            format,
            position: 0.0,
            requests,
            decoded,
            ready: HashMap::new(),
            pending: HashSet::new(),
            current: None,
            ended: false,
        };
        player.request(0);
        Ok(player)
    }

    pub fn format(&self) -> HapFormat {
        self.format
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn info(&self) -> VideoInfo {
        VideoInfo {
            width: self.track.width,
            height: self.track.height,
            fps: self.track.fps,
            duration: Some(self.frames() as f64 / self.track.fps),
        }
    }

    fn frames(&self) -> usize {
        self.track.samples.len()
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    /// Position of the shown frame, in seconds.
    pub fn position(&self) -> f64 {
        self.current
            .as_ref()
            .map(|f| f.index as f64 / self.track.fps)
            .unwrap_or(0.0)
    }

    fn request(&mut self, index: usize) {
        if index < self.frames()
            && !self.pending.contains(&index)
            && !self.ready.contains_key(&index)
            && self.current.as_ref().is_none_or(|f| f.index != index)
            && self.requests.send(index).is_ok()
        {
            self.pending.insert(index);
        }
    }

    pub fn restart(&mut self) {
        self.seek(0.0);
    }

    /// Jumps to `seconds`; the frame there shows as soon as it is read.
    pub fn seek(&mut self, seconds: f64) {
        let last = self.frames().saturating_sub(1) as f64;
        let frame = if seconds.is_finite() {
            (seconds * self.track.fps).clamp(0.0, last)
        } else {
            0.0
        };
        self.position = frame;
        self.ended = false;
        self.request(frame as usize);
    }

    /// Moves playback by `dt` seconds at `speed` (negative plays backwards)
    /// and returns the frame to show when it changed.
    pub fn advance(&mut self, dt: f64, speed: f64, looping: bool) -> Result<Option<&HapFrame>> {
        for (index, result) in self.decoded.try_iter().collect::<Vec<_>>() {
            self.pending.remove(&index);
            match result {
                Ok(frame) => {
                    self.ready.insert(index, frame);
                }
                Err(e) => {
                    return Err(Error::Tool {
                        program: "HAP".into(),
                        message: e,
                    });
                }
            }
        }
        let count = self.frames() as f64;
        if !self.ended {
            self.position += dt * speed * self.track.fps;
            if self.position >= count || self.position < 0.0 {
                if looping {
                    self.position = self.position.rem_euclid(count);
                } else {
                    self.position = self.position.clamp(0.0, count - 1.0);
                    self.ended = true;
                }
            }
        }
        let target = self.position.floor() as usize;
        // Prefetch in the playing direction.
        self.request(target);
        for step in 1..=AHEAD {
            let next = if speed < 0.0 {
                (target + self.frames() - step) % self.frames()
            } else {
                (target + step) % self.frames()
            };
            self.request(next);
        }
        // Drop frames that will not be shown.
        let frames = self.frames();
        self.ready.retain(|i, _| {
            let distance = (*i as isize - target as isize).unsigned_abs();
            distance <= AHEAD || distance + AHEAD >= frames
        });
        let changed = match self.ready.remove(&target) {
            Some(frame) => {
                self.current = Some(frame);
                true
            }
            None => false,
        };
        Ok(if changed { self.current.as_ref() } else { None })
    }

    pub fn frame(&self) -> Option<&HapFrame> {
        self.current.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// Plays until frame `index` shows, or a timeout.
    fn wait_for(player: &mut HapPlayer, dt: f64, speed: f64, index: usize) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if let Ok(Some(f)) = player.advance(dt, speed, true)
                && f.index == index
            {
                return true;
            }
            if player.frame().is_some_and(|f| f.index == index) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    #[test]
    fn plays_seeks_and_reverses() {
        let Some(path) = crate::hap::ffmpeg_tests::encode("hap", 1, "player.mov") else {
            eprintln!("ffmpeg with HAP not available, skipping");
            return;
        };
        let mut player = HapPlayer::open(&path).unwrap();
        assert_eq!(player.format(), HapFormat::Bc1);
        assert_eq!(player.info().duration, Some(0.4));
        assert!(wait_for(&mut player, 0.0, 1.0, 0), "first frame");
        // One frame per call at 25 fps.
        assert!(wait_for(&mut player, 0.04, 1.0, 3));
        player.seek(0.32);
        assert!(wait_for(&mut player, 0.0, 1.0, 8), "seek to frame 8");
        assert!(wait_for(&mut player, 0.04, -1.0, 5), "backwards");
        // Backwards past the start wraps when looping.
        player.seek(0.0);
        assert!(wait_for(&mut player, 0.04, -1.0, 9), "reverse loop");
    }
}
