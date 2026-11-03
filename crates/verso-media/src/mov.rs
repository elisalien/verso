//! Minimal QuickTime / MP4 reader: the sample table of the first video
//! track, enough to read each frame of an intra-only codec (HAP) directly.
//!
//! Files may come from anywhere (a composition shared by someone else), so
//! sizes are checked before anything is allocated.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Largest `moov` box read into memory.
const MAX_MOOV: u64 = 256 * 1024 * 1024;
/// Most samples a track may list.
const MAX_SAMPLES: u64 = 20_000_000;
/// Largest single frame.
pub const MAX_SAMPLE: u32 = 256 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum MovError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a QuickTime/MP4 file: {0}")]
    Invalid(&'static str),
}

type Result<T> = std::result::Result<T, MovError>;

/// The video track of a file.
#[derive(Debug, Clone, PartialEq)]
pub struct VideoTrack {
    /// Sample description format, e.g. `Hap1`.
    pub codec: [u8; 4],
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    /// File offset and size of each frame, in display order.
    pub samples: Vec<(u64, u32)>,
}

impl VideoTrack {
    pub fn codec_name(&self) -> String {
        String::from_utf8_lossy(&self.codec).into_owned()
    }

    /// Reads frame `index` into `buffer`.
    pub fn read(&self, file: &mut File, index: usize, buffer: &mut Vec<u8>) -> Result<()> {
        let &(offset, size) = self
            .samples
            .get(index)
            .ok_or(MovError::Invalid("frame out of range"))?;
        buffer.resize(size as usize, 0);
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(buffer)?;
        Ok(())
    }
}

fn be16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Child boxes of `data`: (type, body).
fn boxes(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while at + 8 <= data.len() {
        let Some(size32) = be32(data, at) else { break };
        let kind: [u8; 4] = data[at + 4..at + 8].try_into().unwrap_or_default();
        let (header, size) = match size32 {
            0 => (8, (data.len() - at) as u64),
            1 => match be64(data, at + 8) {
                Some(s) => (16, s),
                None => break,
            },
            s => (8, s as u64),
        };
        if size < header as u64 || at as u64 + size > data.len() as u64 {
            break;
        }
        out.push((kind, &data[at + header..at + size as usize]));
        at += size as usize;
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    boxes(data)
        .into_iter()
        .find(|(k, _)| k == kind)
        .map(|(_, b)| b)
}

/// Finds the `moov` box at the top level of the file and reads it.
fn read_moov(file: &mut File) -> Result<Vec<u8>> {
    let len = file.metadata()?.len();
    let mut at = 0u64;
    while at + 8 <= len {
        file.seek(SeekFrom::Start(at))?;
        let mut header = [0u8; 16];
        file.read_exact(&mut header[..8])?;
        let size32 = u32::from_be_bytes(header[..4].try_into().unwrap_or_default());
        let (hlen, size) = match size32 {
            0 => (8, len - at),
            1 => {
                file.read_exact(&mut header[8..16])?;
                (
                    16,
                    u64::from_be_bytes(header[8..16].try_into().unwrap_or_default()),
                )
            }
            s => (8, s as u64),
        };
        if size < hlen {
            return Err(MovError::Invalid("bad box size"));
        }
        if &header[4..8] == b"moov" {
            let body = size - hlen;
            if body > MAX_MOOV {
                return Err(MovError::Invalid("moov too large"));
            }
            let mut moov = vec![0u8; body as usize];
            file.read_exact(&mut moov)?;
            return Ok(moov);
        }
        at = at
            .checked_add(size)
            .ok_or(MovError::Invalid("bad box size"))?;
    }
    Err(MovError::Invalid("no moov box"))
}

/// Reads the first video track of `path`.
pub fn video_track(path: &Path) -> Result<VideoTrack> {
    let mut file = File::open(path)?;
    let moov = read_moov(&mut file)?;
    for (kind, trak) in boxes(&moov) {
        if &kind != b"trak" {
            continue;
        }
        let Some(mdia) = child(trak, b"mdia") else {
            continue;
        };
        let is_video = child(mdia, b"hdlr").and_then(|h| h.get(8..12)) == Some(b"vide");
        if !is_video {
            continue;
        }
        return parse_track(mdia);
    }
    Err(MovError::Invalid("no video track"))
}

fn parse_track(mdia: &[u8]) -> Result<VideoTrack> {
    let bad = MovError::Invalid;
    let mdhd = child(mdia, b"mdhd").ok_or(bad("no mdhd"))?;
    let timescale = match mdhd.first() {
        Some(1) => be32(mdhd, 20),
        _ => be32(mdhd, 12),
    }
    .ok_or(bad("bad mdhd"))?;
    let stbl = child(mdia, b"minf")
        .and_then(|m| child(m, b"stbl"))
        .ok_or(bad("no stbl"))?;

    // First sample description: format and size.
    let stsd = child(stbl, b"stsd").ok_or(bad("no stsd"))?;
    let entry = stsd.get(8..).ok_or(bad("bad stsd"))?;
    let codec: [u8; 4] = entry
        .get(4..8)
        .and_then(|c| c.try_into().ok())
        .ok_or(bad("bad stsd"))?;
    // size, format, 6 reserved, data ref index (2), then the visual entry:
    // version, revision, vendor, temporal and spatial quality, width, height.
    let width = be16(entry, 32).ok_or(bad("bad stsd"))? as u32;
    let height = be16(entry, 34).ok_or(bad("bad stsd"))? as u32;

    // Sample sizes.
    let stsz = child(stbl, b"stsz").ok_or(bad("no stsz"))?;
    let fixed = be32(stsz, 4).ok_or(bad("bad stsz"))?;
    let count = be32(stsz, 8).ok_or(bad("bad stsz"))? as u64;
    if count == 0 || count > MAX_SAMPLES {
        return Err(bad("bad sample count"));
    }
    let sizes: Vec<u32> = if fixed != 0 {
        vec![fixed; count as usize]
    } else {
        (0..count as usize)
            .map(|i| be32(stsz, 12 + i * 4))
            .collect::<Option<_>>()
            .ok_or(bad("bad stsz"))?
    };
    if sizes.iter().any(|&s| s == 0 || s > MAX_SAMPLE) {
        return Err(bad("bad sample size"));
    }

    // Chunk offsets.
    let offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        let n = be32(stco, 4).ok_or(bad("bad stco"))? as usize;
        if n as u64 > count {
            return Err(bad("bad stco"));
        }
        (0..n)
            .map(|i| be32(stco, 8 + i * 4).map(u64::from))
            .collect::<Option<_>>()
            .ok_or(bad("bad stco"))?
    } else {
        let co64 = child(stbl, b"co64").ok_or(bad("no chunk offsets"))?;
        let n = be32(co64, 4).ok_or(bad("bad co64"))? as usize;
        if n as u64 > count {
            return Err(bad("bad co64"));
        }
        (0..n)
            .map(|i| be64(co64, 8 + i * 8))
            .collect::<Option<_>>()
            .ok_or(bad("bad co64"))?
    };

    // Samples per chunk: runs of (first chunk, samples per chunk).
    let stsc = child(stbl, b"stsc").ok_or(bad("no stsc"))?;
    let runs = be32(stsc, 4).ok_or(bad("bad stsc"))? as usize;
    if runs as u64 > count {
        return Err(bad("bad stsc"));
    }
    let runs: Vec<(u32, u32)> = (0..runs)
        .map(|i| Some((be32(stsc, 8 + i * 12)?, be32(stsc, 12 + i * 12)?)))
        .collect::<Option<_>>()
        .ok_or(bad("bad stsc"))?;

    let mut samples = Vec::with_capacity(count as usize);
    for (chunk, &offset) in offsets.iter().enumerate() {
        let number = chunk as u32 + 1;
        let per_chunk = runs
            .iter()
            .take_while(|(first, _)| *first <= number)
            .last()
            .map(|(_, n)| *n)
            .ok_or(bad("bad stsc"))?;
        let mut at = offset;
        for _ in 0..per_chunk {
            let Some(&size) = sizes.get(samples.len()) else {
                break;
            };
            samples.push((at, size));
            at = at.checked_add(size as u64).ok_or(bad("bad offsets"))?;
        }
    }
    if samples.len() != sizes.len() {
        return Err(bad("sample table does not add up"));
    }

    // Frame rate from the time-to-sample table.
    let stts = child(stbl, b"stts").ok_or(bad("no stts"))?;
    let entries = be32(stts, 4).ok_or(bad("bad stts"))? as usize;
    let mut duration = 0u64;
    let mut timed = 0u64;
    for i in 0..entries.min(count as usize) {
        let n = be32(stts, 8 + i * 8).ok_or(bad("bad stts"))? as u64;
        let delta = be32(stts, 12 + i * 8).ok_or(bad("bad stts"))? as u64;
        duration = duration.saturating_add(n.saturating_mul(delta));
        timed += n;
    }
    let fps = if duration > 0 && timescale > 0 {
        timed as f64 * timescale as f64 / duration as f64
    } else {
        30.0
    };
    let fps = if fps.is_finite() && fps > 0.0 && fps <= 1000.0 {
        fps
    } else {
        30.0
    };
    Ok(VideoTrack {
        codec,
        width,
        height,
        fps,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_is_rejected_without_panicking() {
        let dir = std::env::temp_dir().join("verso-mov-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("garbage.mov");
        // A moov box claiming a huge size, then random bytes.
        let mut data = vec![0u8, 0, 0, 1];
        data.extend_from_slice(b"moov");
        data.extend((0..4096).map(|i| (i * 7919 % 251) as u8));
        std::fs::write(&path, &data).unwrap();
        assert!(video_track(&path).is_err());
        assert!(boxes(&data).len() <= 1);
    }
}
