//! SMPTE timecode from MIDI (MTC) or audio (LTC).
//!
//! - [`MtcDecoder`] reads MIDI Time Code: quarter-frame messages (`F1 nd`)
//!   and full-frame SysEx (`F0 7F 7F 01 01 hh mm ss ff F7`).
//! - [`LtcDecoder`] reads Linear Timecode from audio samples: biphase-mark
//!   coded 80-bit frames, any speed around 24–30 fps.

use std::time::{Duration, Instant};

/// SMPTE frame rates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TcRate {
    Fps24,
    Fps25,
    /// 29.97 drop-frame.
    Fps2997Drop,
    #[default]
    Fps30,
}

impl TcRate {
    pub fn fps(self) -> f64 {
        match self {
            TcRate::Fps24 => 24.0,
            TcRate::Fps25 => 25.0,
            TcRate::Fps2997Drop => 30000.0 / 1001.0,
            TcRate::Fps30 => 30.0,
        }
    }

    fn nominal(self) -> u32 {
        match self {
            TcRate::Fps24 => 24,
            TcRate::Fps25 => 25,
            TcRate::Fps2997Drop | TcRate::Fps30 => 30,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            TcRate::Fps24 => "24",
            TcRate::Fps25 => "25",
            TcRate::Fps2997Drop => "29.97 DF",
            TcRate::Fps30 => "30",
        }
    }
}

/// A timecode position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timecode {
    pub hours: u8,
    pub minutes: u8,
    pub seconds: u8,
    pub frames: u8,
    pub rate: TcRate,
}

impl Timecode {
    /// Frames since 00:00:00:00, dropped frame numbers excluded.
    pub fn frame_count(&self) -> u64 {
        let nominal = self.rate.nominal() as u64;
        let total_minutes = self.hours as u64 * 60 + self.minutes as u64;
        let mut frames = (total_minutes * 60 + self.seconds as u64) * nominal + self.frames as u64;
        if self.rate == TcRate::Fps2997Drop {
            // Two frame numbers are skipped each minute except every tenth.
            frames -= 2 * (total_minutes - total_minutes / 10);
        }
        frames
    }

    /// Position in seconds.
    pub fn seconds(&self) -> f64 {
        self.frame_count() as f64 / self.rate.fps()
    }

    fn valid(&self) -> bool {
        self.hours < 24
            && self.minutes < 60
            && self.seconds < 60
            && (self.frames as u32) < self.rate.nominal()
    }
}

impl std::fmt::Display for Timecode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let sep = if self.rate == TcRate::Fps2997Drop {
            ';'
        } else {
            ':'
        };
        write!(
            f,
            "{:02}:{:02}:{:02}{sep}{:02}",
            self.hours, self.minutes, self.seconds, self.frames
        )
    }
}

/// Timecode received from a source, with when it arrived, so it can be read
/// between frames and timed out when the source stops.
#[derive(Debug, Clone, Copy, Default)]
pub struct TimecodeClock {
    last: Option<(Timecode, Instant)>,
}

impl TimecodeClock {
    pub fn set(&mut self, tc: Timecode) {
        self.last = Some((tc, Instant::now()));
    }

    /// The newest timecode, or `None` when nothing arrived for half a second.
    pub fn current(&self) -> Option<Timecode> {
        self.last
            .filter(|(_, at)| at.elapsed() < Duration::from_millis(500))
            .map(|(tc, _)| tc)
    }

    /// Position in seconds, extrapolated since the last frame for smooth
    /// motion, or `None` when stopped.
    pub fn seconds(&self) -> Option<f64> {
        let (tc, at) = self.last?;
        let elapsed = at.elapsed().as_secs_f64();
        (elapsed < 0.5).then(|| tc.seconds() + elapsed.min(2.0 / tc.rate.fps()))
    }
}

fn rate_from_bits(bits: u8) -> TcRate {
    match bits & 3 {
        0 => TcRate::Fps24,
        1 => TcRate::Fps25,
        2 => TcRate::Fps2997Drop,
        _ => TcRate::Fps30,
    }
}

/// Decodes MIDI Time Code from raw MIDI messages.
#[derive(Debug, Default)]
pub struct MtcDecoder {
    pieces: [u8; 8],
    /// Pieces received since the last piece 0, as a bit set.
    seen: u8,
}

impl MtcDecoder {
    /// Feeds one MIDI message; returns a timecode when one is complete.
    pub fn push(&mut self, bytes: &[u8]) -> Option<Timecode> {
        match bytes {
            [0xF1, data, ..] => {
                let piece = (data >> 4) as usize & 7;
                if piece == 0 {
                    self.seen = 0;
                }
                self.pieces[piece] = data & 0x0F;
                self.seen |= 1 << piece;
                if piece == 7 && self.seen == 0xFF {
                    let p = &self.pieces;
                    let mut tc = Timecode {
                        frames: p[0] | (p[1] & 1) << 4,
                        seconds: p[2] | (p[3] & 3) << 4,
                        minutes: p[4] | (p[5] & 3) << 4,
                        hours: p[6] | (p[7] & 1) << 4,
                        rate: rate_from_bits(p[7] >> 1),
                    };
                    // A full message spans two frames: the position is now
                    // two frames further on.
                    advance(&mut tc, 2);
                    return tc.valid().then_some(tc);
                }
                None
            }
            [0xF0, 0x7F, _, 0x01, 0x01, hr, mn, sc, fr, ..] => {
                let tc = Timecode {
                    hours: hr & 0x1F,
                    minutes: mn & 0x3F,
                    seconds: sc & 0x3F,
                    frames: fr & 0x1F,
                    rate: rate_from_bits(hr >> 5),
                };
                tc.valid().then_some(tc)
            }
            _ => None,
        }
    }
}

/// Moves `tc` forward by `frames`, wrapping at 24 hours.
fn advance(tc: &mut Timecode, frames: u32) {
    let nominal = tc.rate.nominal();
    let mut f = tc.frames as u32 + frames;
    let mut s = tc.seconds as u32;
    let mut m = tc.minutes as u32;
    let mut h = tc.hours as u32;
    while f >= nominal {
        f -= nominal;
        s += 1;
        if s == 60 {
            s = 0;
            m += 1;
            if tc.rate == TcRate::Fps2997Drop && !m.is_multiple_of(10) {
                f += 2;
            }
            if m == 60 {
                m = 0;
                h = (h + 1) % 24;
            }
        }
    }
    *tc = Timecode {
        hours: h as u8,
        minutes: m as u8,
        seconds: s as u8,
        frames: f as u8,
        rate: tc.rate,
    };
}

/// Bits 64–79 of an LTC frame, first transmitted bit lowest.
const LTC_SYNC: u16 = 0xBFFC;

/// Decodes Linear Timecode from mono audio.
pub struct LtcDecoder {
    sample_rate: f32,
    /// Last sign of the signal (with hysteresis) and samples since its change.
    high: bool,
    since_edge: f32,
    /// Estimated length of one bit in samples.
    bit: f32,
    /// A short (half-bit) interval was seen: the next short completes a 1.
    half: bool,
    /// The last 80 bits, newest in bit 79.
    bits: u128,
    /// Bit count since the decoder was (re)synchronised.
    count: u32,
    /// Peak level, decaying, for the hysteresis threshold.
    peak: f32,
}

impl LtcDecoder {
    pub fn new(sample_rate: u32) -> Self {
        let sample_rate = sample_rate.max(8000) as f32;
        Self {
            sample_rate,
            high: false,
            since_edge: 0.0,
            // 30 fps × 80 bits.
            bit: sample_rate / 2400.0,
            half: false,
            bits: 0,
            count: 0,
            peak: 0.0,
        }
    }

    /// Feeds samples; returns the frames completed in them.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Timecode> {
        let mut out = Vec::new();
        // Bit lengths of LTC between 23 and 31 fps.
        let (min_bit, max_bit) = (self.sample_rate / 2480.0, self.sample_rate / 1840.0);
        for &s in samples {
            self.peak = (self.peak * 0.9995).max(s.abs());
            let threshold = self.peak * 0.1;
            self.since_edge += 1.0;
            let edge = if self.high && s < -threshold {
                self.high = false;
                true
            } else if !self.high && s > threshold {
                self.high = true;
                true
            } else {
                false
            };
            if !edge {
                continue;
            }
            let interval = std::mem::take(&mut self.since_edge);
            if interval < min_bit * 0.35 || interval > max_bit * 1.4 {
                // Noise or a gap: start over.
                self.half = false;
                self.count = 0;
                continue;
            }
            if interval > self.bit * 0.75 {
                // A full-bit interval: 0.
                self.bit = (self.bit * 0.9 + interval * 0.1).clamp(min_bit, max_bit);
                if self.half {
                    // A lone half bit: lost sync.
                    self.half = false;
                    self.count = 0;
                }
                self.bit_in(false, &mut out);
            } else if self.half {
                // Second half of a 1.
                self.half = false;
                self.bit = (self.bit * 0.9 + interval * 2.0 * 0.1).clamp(min_bit, max_bit);
                self.bit_in(true, &mut out);
            } else {
                self.half = true;
            }
        }
        out
    }

    fn bit_in(&mut self, one: bool, out: &mut Vec<Timecode>) {
        self.bits = (self.bits >> 1) | ((one as u128) << 79);
        self.count += 1;
        if self.count >= 80
            && (self.bits >> 64) as u16 == LTC_SYNC
            && let Some(tc) = ltc_frame(self.bits)
        {
            out.push(tc);
        }
    }
}

/// Decodes the 80 bits of an LTC frame, bit 0 first transmitted.
fn ltc_frame(bits: u128) -> Option<Timecode> {
    let field = |start: u32, len: u32| ((bits >> start) & ((1 << len) - 1)) as u8;
    let drop = field(10, 1) == 1;
    let tc = Timecode {
        frames: field(0, 4) + 10 * field(8, 2),
        seconds: field(16, 4) + 10 * field(24, 3),
        minutes: field(32, 4) + 10 * field(40, 3),
        hours: field(48, 4) + 10 * field(56, 2),
        rate: if drop {
            TcRate::Fps2997Drop
        } else {
            // The frame rate is not in the frame; 30 is assumed until a
            // frame number shows otherwise. Seconds stay right either way.
            TcRate::Fps30
        },
    };
    tc.valid().then_some(tc)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes `tc` as the 80 bits of an LTC frame.
    fn ltc_bits(tc: &Timecode) -> u128 {
        let mut bits: u128 = 0;
        let mut put = |start: u32, value: u8, len: u32| {
            bits |= ((value as u128) & ((1 << len) - 1)) << start;
        };
        put(0, tc.frames % 10, 4);
        put(8, tc.frames / 10, 2);
        put(10, (tc.rate == TcRate::Fps2997Drop) as u8, 1);
        put(16, tc.seconds % 10, 4);
        put(24, tc.seconds / 10, 3);
        put(32, tc.minutes % 10, 4);
        put(40, tc.minutes / 10, 3);
        put(48, tc.hours % 10, 4);
        put(56, tc.hours / 10, 2);
        bits | (LTC_SYNC as u128) << 64
    }

    /// Biphase-mark audio for consecutive frames starting at `start`.
    fn ltc_audio(start: Timecode, frames: u32, fps: f32, sample_rate: f32) -> Vec<f32> {
        let bit_len = sample_rate / (fps * 80.0);
        let mut out = Vec::new();
        let mut level = 0.8f32;
        let mut t = 0.0f32;
        let mut tc = start;
        for _ in 0..frames {
            let bits = ltc_bits(&tc);
            for i in 0..80 {
                let one = (bits >> i) & 1 == 1;
                // Transition at every bit start; ones also in the middle.
                level = -level;
                let half = bit_len / 2.0;
                let end_half = t + half;
                while (out.len() as f32) < end_half {
                    out.push(level);
                }
                if one {
                    level = -level;
                }
                t += bit_len;
                while (out.len() as f32) < t {
                    out.push(level);
                }
            }
            advance(&mut tc, 1);
        }
        out
    }

    #[test]
    fn ltc_round_trip_at_25_and_30_fps() {
        for (fps, rate) in [(25.0, TcRate::Fps25), (30.0, TcRate::Fps30)] {
            let start = Timecode {
                hours: 1,
                minutes: 59,
                seconds: 59,
                frames: 20,
                rate,
            };
            let audio = ltc_audio(start, 12, fps, 48000.0);
            let mut decoder = LtcDecoder::new(48000);
            // Feed in uneven blocks like an audio callback would.
            let decoded: Vec<Timecode> = audio.chunks(333).flat_map(|c| decoder.push(c)).collect();
            assert!(decoded.len() >= 10, "{fps} fps: {decoded:?}");
            let last = decoded.last().unwrap();
            // The last frame ends without the next bit's transition, so it
            // is only decoded once more signal arrives: the 11th is the last.
            let mut expected = start;
            advance(&mut expected, 10);
            assert_eq!(
                (last.hours, last.minutes, last.seconds, last.frames),
                (
                    expected.hours,
                    expected.minutes,
                    expected.seconds,
                    expected.frames
                ),
                "{fps} fps"
            );
            assert_eq!(last.hours, 2, "crossed the hour");
        }
    }

    #[test]
    fn ltc_ignores_noise() {
        let mut decoder = LtcDecoder::new(48000);
        let noise: Vec<f32> = (0..48000)
            .map(|i| ((i as f32 * 12.9898).sin() * 43758.545).fract() - 0.5)
            .collect();
        assert!(decoder.push(&noise).is_empty());
    }

    #[test]
    fn mtc_quarter_frames_and_full_frame() {
        let mut decoder = MtcDecoder::default();
        // 10:20:30:12 at 25 fps.
        let (h, m, s, f) = (10u8, 20u8, 30u8, 12u8);
        let rate_bits = 1u8;
        let pieces = [
            f & 0xF,
            f >> 4,
            s & 0xF,
            s >> 4,
            m & 0xF,
            m >> 4,
            h & 0xF,
            (h >> 4) | rate_bits << 1,
        ];
        let mut result = None;
        for (i, p) in pieces.iter().enumerate() {
            result = decoder.push(&[0xF1, (i as u8) << 4 | p]);
            if i < 7 {
                assert!(result.is_none());
            }
        }
        let tc = result.unwrap();
        assert_eq!(tc.rate, TcRate::Fps25);
        assert_eq!(tc.to_string(), "10:20:30:14", "two frames later");

        let full = decoder
            .push(&[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x60 | 1, 2, 3, 4, 0xF7])
            .unwrap();
        assert_eq!(full.rate, TcRate::Fps30);
        assert_eq!(full.to_string(), "01:02:03:04");
    }

    #[test]
    fn drop_frame_counts() {
        let tc = Timecode {
            hours: 0,
            minutes: 10,
            seconds: 0,
            frames: 0,
            rate: TcRate::Fps2997Drop,
        };
        // 10 minutes of 29.97 drop-frame is 17982 frames.
        assert_eq!(tc.frame_count(), 17982);
        assert!((tc.seconds() - 600.0).abs() < 0.01);
        let mut t = Timecode {
            minutes: 0,
            seconds: 59,
            frames: 29,
            ..tc
        };
        advance(&mut t, 1);
        assert_eq!(t.to_string(), "00:01:00;02");
    }
}
