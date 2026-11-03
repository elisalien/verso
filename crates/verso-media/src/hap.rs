//! HAP frames to GPU-ready block-compressed texture data.
//!
//! A HAP frame is a section header (24-bit size and a type byte, or a zero
//! size followed by a 32-bit size) and a payload that is the texture itself,
//! either raw, Snappy-compressed, or split into chunks ("complex") each raw
//! or Snappy-compressed. The texture data goes to the GPU as is: no CPU
//! decoding of pixels.
//!
//! Supported: Hap (BC1), Hap Alpha (BC3), Hap Q (YCoCg in BC3, converted in
//! the shader) and Hap R (BC7). Hap Q Alpha and the alpha-only variant fall
//! back to FFmpeg.

/// Block-compressed format of a frame's texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HapFormat {
    /// RGB in BC1 (DXT1).
    Bc1,
    /// RGBA in BC3 (DXT5).
    Bc3,
    /// Scaled YCoCg in BC3, converted to RGB when drawn.
    YCoCgBc3,
    /// RGBA in BC7.
    Bc7,
}

impl HapFormat {
    /// Bytes per 4×4 block.
    pub fn block_bytes(self) -> usize {
        match self {
            HapFormat::Bc1 => 8,
            HapFormat::Bc3 | HapFormat::YCoCgBc3 | HapFormat::Bc7 => 16,
        }
    }

    /// Texture data size of a `width`×`height` frame.
    pub fn data_len(self, width: u32, height: u32) -> usize {
        width.div_ceil(4) as usize * height.div_ceil(4) as usize * self.block_bytes()
    }

    /// The format a QuickTime codec name announces, when supported.
    pub fn from_codec(codec: &[u8; 4]) -> Option<HapFormat> {
        match codec {
            b"Hap1" => Some(HapFormat::Bc1),
            b"Hap5" => Some(HapFormat::Bc3),
            b"HapY" => Some(HapFormat::YCoCgBc3),
            b"Hap7" => Some(HapFormat::Bc7),
            _ => None,
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum HapError {
    #[error("HAP frame is truncated")]
    Truncated,
    #[error("unsupported HAP section type {0:#04x}")]
    Unsupported(u8),
    #[error("HAP frame decompression failed")]
    Snappy,
    #[error("HAP frame has {got} bytes of texture, {want} expected")]
    Size { got: usize, want: usize },
}

/// Reads a section header at the start of `data`: (type, payload).
fn section(data: &[u8]) -> Result<(u8, &[u8]), HapError> {
    let header = data.get(..4).ok_or(HapError::Truncated)?;
    let small = u32::from_le_bytes([header[0], header[1], header[2], 0]) as usize;
    let kind = header[3];
    let (start, size): (usize, usize) = if small == 0 {
        let size = data.get(4..8).ok_or(HapError::Truncated)?;
        (
            8,
            u32::from_le_bytes(size.try_into().map_err(|_| HapError::Truncated)?) as usize,
        )
    } else {
        (4, small)
    };
    let end = start.checked_add(size).ok_or(HapError::Truncated)?;
    Ok((kind, data.get(start..end).ok_or(HapError::Truncated)?))
}

fn format_of(kind: u8) -> Result<HapFormat, HapError> {
    match kind & 0x0F {
        0x0B => Ok(HapFormat::Bc1),
        0x0E => Ok(HapFormat::Bc3),
        0x0F => Ok(HapFormat::YCoCgBc3),
        0x0C => Ok(HapFormat::Bc7),
        _ => Err(HapError::Unsupported(kind)),
    }
}

fn snappy(data: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), HapError> {
    let len = snap::raw::decompress_len(data).map_err(|_| HapError::Snappy)?;
    if out.len() + len > limit {
        return Err(HapError::Snappy);
    }
    let start = out.len();
    out.resize(start + len, 0);
    snap::raw::Decoder::new()
        .decompress(data, &mut out[start..])
        .map_err(|_| HapError::Snappy)?;
    Ok(())
}

/// Decodes the chunked ("complex") payload: decode instructions, then the
/// chunks.
fn complex(payload: &[u8], out: &mut Vec<u8>, limit: usize) -> Result<(), HapError> {
    let (kind, instructions) = section(payload)?;
    if kind != 0x01 {
        return Err(HapError::Unsupported(kind));
    }
    // The instructions section's own header length.
    let header = if payload[..3] == [0, 0, 0] { 8 } else { 4 };
    let chunks_start = header + instructions.len();
    let mut compressors: &[u8] = &[];
    let mut sizes: Vec<usize> = Vec::new();
    let mut offsets: Option<Vec<usize>> = None;
    let mut rest = instructions;
    while !rest.is_empty() {
        let (kind, body) = section(rest)?;
        let header = if rest[..3] == [0, 0, 0] { 8 } else { 4 };
        let words = || {
            body.chunks_exact(4)
                .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) as usize)
                .collect::<Vec<_>>()
        };
        match kind {
            0x02 => compressors = body,
            0x03 => sizes = words(),
            0x04 => offsets = Some(words()),
            _ => {}
        }
        rest = &rest[header + body.len()..];
    }
    if compressors.len() != sizes.len() {
        return Err(HapError::Truncated);
    }
    let chunks = payload.get(chunks_start..).ok_or(HapError::Truncated)?;
    let mut at = 0usize;
    for (i, (&compressor, &size)) in compressors.iter().zip(&sizes).enumerate() {
        let start = offsets
            .as_ref()
            .and_then(|o| o.get(i).copied())
            .unwrap_or(at);
        let end = start.checked_add(size).ok_or(HapError::Truncated)?;
        let chunk = chunks.get(start..end).ok_or(HapError::Truncated)?;
        match compressor {
            0x0A => {
                if out.len() + chunk.len() > limit {
                    return Err(HapError::Truncated);
                }
                out.extend_from_slice(chunk);
            }
            0x0B => snappy(chunk, out, limit)?,
            other => return Err(HapError::Unsupported(other)),
        }
        at = end;
    }
    Ok(())
}

/// Decodes a frame of `width`×`height` into `out` (texture data, rows of
/// 4×4 blocks) and returns its format.
pub fn decode(
    frame: &[u8],
    width: u32,
    height: u32,
    out: &mut Vec<u8>,
) -> Result<HapFormat, HapError> {
    let (kind, payload) = section(frame)?;
    let format = format_of(kind)?;
    let want = format.data_len(width, height);
    out.clear();
    match kind >> 4 {
        0x0A => out.extend_from_slice(payload),
        0x0B => snappy(payload, out, want)?,
        0x0C => complex(payload, out, want)?,
        _ => return Err(HapError::Unsupported(kind)),
    }
    if out.len() != want {
        return Err(HapError::Size {
            got: out.len(),
            want,
        });
    }
    Ok(format)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(kind: u8, len: usize) -> Vec<u8> {
        let mut h = (len as u32).to_le_bytes()[..3].to_vec();
        h.push(kind);
        h
    }

    #[test]
    fn raw_and_snappy_frames() {
        let texture: Vec<u8> = (0..HapFormat::Bc1.data_len(8, 4))
            .map(|i| i as u8)
            .collect();
        let mut raw = header(0xAB, texture.len());
        raw.extend_from_slice(&texture);
        let mut out = Vec::new();
        assert_eq!(decode(&raw, 8, 4, &mut out), Ok(HapFormat::Bc1));
        assert_eq!(out, texture);

        let packed = snap::raw::Encoder::new().compress_vec(&texture).unwrap();
        let mut frame = header(0xBE, packed.len());
        frame.extend_from_slice(&packed);
        let texture = &texture[..];
        // Same bytes read as BC3 need twice the data: size mismatch.
        assert!(matches!(
            decode(&frame, 8, 4, &mut out),
            Err(HapError::Size { .. })
        ));
        assert_eq!(decode(&frame, 4, 4, &mut out), Ok(HapFormat::Bc3));
        assert_eq!(out, texture);
    }

    #[test]
    fn complex_frames_join_their_chunks() {
        let texture: Vec<u8> = (0..HapFormat::Bc7.data_len(8, 8))
            .map(|i| (i * 3) as u8)
            .collect();
        let (a, b) = texture.split_at(24);
        let b_packed = snap::raw::Encoder::new().compress_vec(b).unwrap();
        let mut instructions = header(0x02, 2);
        instructions.extend_from_slice(&[0x0A, 0x0B]);
        instructions.extend(header(0x03, 8));
        instructions.extend((a.len() as u32).to_le_bytes());
        instructions.extend((b_packed.len() as u32).to_le_bytes());
        let mut payload = header(0x01, instructions.len());
        payload.extend(instructions);
        payload.extend_from_slice(a);
        payload.extend_from_slice(&b_packed);
        let mut frame = header(0xCC, payload.len());
        frame.extend(payload);
        let mut out = Vec::new();
        assert_eq!(decode(&frame, 8, 8, &mut out), Ok(HapFormat::Bc7));
        assert_eq!(out, texture);
    }

    #[test]
    fn truncated_frames_are_errors() {
        let mut out = Vec::new();
        assert_eq!(decode(&[1, 0], 4, 4, &mut out), Err(HapError::Truncated));
        let mut frame = header(0xBB, 100);
        frame.extend_from_slice(&[0; 10]);
        assert_eq!(decode(&frame, 4, 4, &mut out), Err(HapError::Truncated));
    }
}

#[cfg(test)]
pub(crate) mod ffmpeg_tests {
    use super::*;
    use crate::mov;

    /// Encodes a short HAP clip; `None` without FFmpeg or its HAP encoder.
    pub(crate) fn encode(format: &str, chunks: u32, name: &str) -> Option<std::path::PathBuf> {
        let dir = std::env::temp_dir().join("verso-hap-test");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(name);
        // HAP needs sizes in multiples of 4.
        let output = std::process::Command::new(crate::tool_path("ffmpeg"))
            .args(["-v", "error", "-y", "-f", "lavfi", "-i"])
            .arg("testsrc2=size=128x72:rate=25:duration=0.4")
            .args(["-c:v", "hap", "-format", format, "-chunks"])
            .arg(chunks.to_string())
            .arg(&path)
            .output()
            .ok()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Unknown encoder") {
            return None;
        }
        assert!(
            output.status.success(),
            "FFmpeg HAP encoding failed: {stderr}"
        );
        Some(path)
    }

    #[test]
    fn ffmpeg_hap_files_decode() {
        let cases = [
            ("hap", 1, b"Hap1", HapFormat::Bc1),
            ("hap_alpha", 1, b"Hap5", HapFormat::Bc3),
            ("hap_q", 1, b"HapY", HapFormat::YCoCgBc3),
            ("hap", 4, b"Hap1", HapFormat::Bc1),
            ("hap_q", 3, b"HapY", HapFormat::YCoCgBc3),
        ];
        for (format, chunks, codec, expected) in cases {
            let name = format!("{format}-{chunks}.mov");
            let Some(path) = encode(format, chunks, &name) else {
                eprintln!("ffmpeg with HAP not available, skipping");
                return;
            };
            let track = mov::video_track(&path).unwrap();
            assert_eq!(&track.codec, codec, "{name}");
            assert_eq!((track.width, track.height), (128, 72), "{name}");
            assert_eq!(track.samples.len(), 10, "{name}");
            assert!((track.fps - 25.0).abs() < 0.01, "{name}: {}", track.fps);
            let mut file = std::fs::File::open(&path).unwrap();
            let (mut frame, mut texture) = (Vec::new(), Vec::new());
            for i in 0..track.samples.len() {
                track.read(&mut file, i, &mut frame).unwrap();
                let got = decode(&frame, track.width, track.height, &mut texture)
                    .unwrap_or_else(|e| panic!("{name} frame {i}: {e}"));
                assert_eq!(got, expected, "{name}");
            }
        }
    }
}
