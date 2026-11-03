//! Audio analysis: band levels, beat detection and tempo estimate.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

const FRAME: usize = 1024;
const HOP: usize = 512;
/// Latest raw samples kept for waveform consumers (MilkDrop).
pub const WAVEFORM_LEN: usize = 1024;
/// Seconds of onset history used for the tempo estimate.
const TEMPO_WINDOW: f32 = 8.0;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;

/// Analysis results, each band in 0..1.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Levels {
    pub level: f32,
    pub bass: f32,
    pub mid: f32,
    pub high: f32,
    /// 1 on a detected beat, then decaying.
    pub beat: f32,
    /// Beats detected so far.
    pub beats: u64,
    /// Tempo estimated from the last seconds of audio, when confident.
    pub bpm: Option<f32>,
}

impl Levels {
    /// In `verso_core::modulation::AudioBand` order.
    pub fn bands(&self) -> [f32; 5] {
        [self.level, self.bass, self.mid, self.high, self.beat]
    }
}

pub struct Analyzer {
    sample_rate: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    pending: Vec<f32>,
    /// Ring of the last [`WAVEFORM_LEN`] samples, oldest at `recent_pos`.
    recent: Vec<f32>,
    recent_pos: usize,
    /// Decaying peaks for auto gain: level, bass, mid, high.
    peaks: [f32; 4],
    smoothed: [f32; 4],
    /// Bass magnitude of recent hops, for the beat threshold.
    bass_history: Vec<f32>,
    previous_bass: f32,
    hops_since_beat: usize,
    /// Onset strength per hop, for the tempo estimate.
    onsets: Vec<f32>,
    hops_since_tempo: usize,
    levels: Levels,
}

impl Analyzer {
    pub fn new(sample_rate: u32) -> Self {
        let window = (0..FRAME)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FRAME as f32).cos())
            .collect();
        Self {
            sample_rate: sample_rate as f32,
            fft: RealFftPlanner::new().plan_fft_forward(FRAME),
            window,
            pending: Vec::with_capacity(FRAME * 2),
            recent: vec![0.0; WAVEFORM_LEN],
            recent_pos: 0,
            peaks: [1e-6; 4],
            smoothed: [0.0; 4],
            bass_history: Vec::new(),
            previous_bass: 0.0,
            hops_since_beat: usize::MAX / 2,
            onsets: Vec::new(),
            hops_since_tempo: 0,
            levels: Levels::default(),
        }
    }

    fn hops_per_second(&self) -> f32 {
        self.sample_rate / HOP as f32
    }

    /// Per-hop factor that halves a value in `seconds`.
    fn decay(&self, seconds: f32) -> f32 {
        0.5f32.powf(1.0 / (seconds * self.hops_per_second()))
    }

    /// Feeds mono samples in -1..1.
    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            self.recent[self.recent_pos] = s;
            self.recent_pos = (self.recent_pos + 1) % WAVEFORM_LEN;
        }
        self.pending.extend_from_slice(samples);
        while self.pending.len() >= FRAME {
            self.analyze_frame();
            self.pending.drain(..HOP);
        }
    }

    pub fn levels(&self) -> Levels {
        self.levels
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    /// The last [`WAVEFORM_LEN`] mono samples, oldest first.
    pub fn waveform(&self) -> Vec<f32> {
        let (newer, older) = self.recent.split_at(self.recent_pos);
        older.iter().chain(newer).copied().collect()
    }

    fn analyze_frame(&mut self) {
        let frame = &self.pending[..FRAME];
        let rms = (frame.iter().map(|s| s * s).sum::<f32>() / FRAME as f32).sqrt();
        let mut input: Vec<f32> = frame.iter().zip(&self.window).map(|(s, w)| s * w).collect();
        let mut spectrum = self.fft.make_output_vec();
        if self.fft.process(&mut input, &mut spectrum).is_err() {
            return;
        }
        let bin_hz = self.sample_rate / FRAME as f32;
        let band = |low: f32, high: f32| -> f32 {
            let (a, b) = (
                (low / bin_hz) as usize,
                ((high / bin_hz) as usize).min(spectrum.len()),
            );
            spectrum[a.max(1)..b]
                .iter()
                .map(|c| c.norm_sqr())
                .sum::<f32>()
                .sqrt()
        };
        let raw = [
            rms,
            band(20.0, 150.0),
            band(150.0, 2000.0),
            band(2000.0, 16000.0),
        ];

        // Auto gain: each band against its own decaying peak, but never against
        // less than a quarter of the loudest band, so a band holding only
        // spectral leakage stays low.
        let peak_decay = self.decay(10.0);
        for (peak, value) in self.peaks.iter_mut().zip(raw) {
            *peak = (*peak * peak_decay).max(value).max(1e-6);
        }
        let loudest = self.peaks[1..].iter().cloned().fold(0.0, f32::max);
        let release = self.decay(0.15);
        for (i, (smoothed, value)) in self.smoothed.iter_mut().zip(raw).enumerate() {
            let reference = if i == 0 {
                self.peaks[0]
            } else {
                self.peaks[i].max(loudest * 0.25)
            };
            let value = (value / reference).clamp(0.0, 1.0);
            *smoothed = value.max(*smoothed * release);
        }

        self.detect_beat(raw[1]);
        self.estimate_tempo();

        let [level, bass, mid, high] = self.smoothed;
        self.levels = Levels {
            level,
            bass,
            mid,
            high,
            ..self.levels
        };
    }

    fn detect_beat(&mut self, bass: f32) {
        let onset = (bass - self.previous_bass).max(0.0);
        self.previous_bass = bass;
        self.onsets.push(onset);
        let keep = (TEMPO_WINDOW * self.hops_per_second()) as usize;
        if self.onsets.len() > keep {
            self.onsets.drain(..self.onsets.len() - keep);
        }

        let second = self.hops_per_second() as usize;
        let average = if self.bass_history.is_empty() {
            0.0
        } else {
            self.bass_history.iter().sum::<f32>() / self.bass_history.len() as f32
        };
        self.bass_history.push(bass);
        if self.bass_history.len() > second {
            self.bass_history.remove(0);
        }

        self.hops_since_beat = self.hops_since_beat.saturating_add(1);
        let min_gap = (0.3 * self.hops_per_second()) as usize;
        let is_beat = onset > 0.0
            && bass > average * 1.3
            && bass > self.peaks[1] * 0.3
            && self.hops_since_beat >= min_gap;
        if is_beat {
            self.hops_since_beat = 0;
            self.levels.beats += 1;
            self.levels.beat = 1.0;
        } else {
            self.levels.beat *= self.decay(0.08);
        }
    }

    /// Autocorrelation of the onset envelope over the plausible tempo range,
    /// recomputed about once a second.
    fn estimate_tempo(&mut self) {
        self.hops_since_tempo += 1;
        let hps = self.hops_per_second();
        if (self.hops_since_tempo as f32) < hps {
            return;
        }
        self.hops_since_tempo = 0;
        let onsets = &self.onsets;
        if (onsets.len() as f32) < 4.0 * hps {
            return;
        }
        let mean = onsets.iter().sum::<f32>() / onsets.len() as f32;
        let centered: Vec<f32> = onsets.iter().map(|o| o - mean).collect();
        let correlation = |lag: usize| -> f32 {
            centered[lag..]
                .iter()
                .zip(&centered)
                .map(|(a, b)| a * b)
                .sum::<f32>()
                / (centered.len() - lag) as f32
        };
        let energy = correlation(0);
        if energy <= 0.0 {
            return;
        }
        let min_lag = (60.0 / MAX_BPM * hps).floor() as usize;
        let max_lag = (60.0 / MIN_BPM * hps).ceil() as usize;
        let scores: Vec<f32> = (min_lag..=max_lag + 1).map(correlation).collect();
        let Some((best, &score)) = scores[..scores.len() - 1]
            .iter()
            .enumerate()
            .skip(1)
            .max_by(|a, b| a.1.total_cmp(b.1))
        else {
            return;
        };
        if score < energy * 0.2 {
            self.levels.bpm = None;
            return;
        }
        // Parabolic interpolation around the best lag.
        let (a, b, c) = (scores[best - 1], score, scores[best + 1]);
        let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
        let lag = (min_lag + best) as f32
            + if shift.is_finite() {
                shift.clamp(-0.5, 0.5)
            } else {
                0.0
            };
        self.levels.bpm = Some(60.0 * hps / lag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn feed(analyzer: &mut Analyzer, seconds: f32, sample: impl Fn(f32) -> f32) {
        let n = (seconds * RATE as f32) as usize;
        let samples: Vec<f32> = (0..n).map(|i| sample(i as f32 / RATE as f32)).collect();
        for chunk in samples.chunks(480) {
            analyzer.push(chunk);
        }
    }

    #[test]
    fn bass_tone_lights_bass_only() {
        let mut analyzer = Analyzer::new(RATE);
        feed(&mut analyzer, 2.0, |t| {
            0.5 * (std::f32::consts::TAU * 80.0 * t).sin()
        });
        let levels = analyzer.levels();
        assert!(levels.bass > 0.8, "{levels:?}");
        assert!(levels.high < 0.2, "{levels:?}");
        assert!(levels.level > 0.8, "{levels:?}");
    }

    #[test]
    fn treble_tone_lights_high_only() {
        let mut analyzer = Analyzer::new(RATE);
        feed(&mut analyzer, 2.0, |t| {
            0.5 * (std::f32::consts::TAU * 5000.0 * t).sin()
        });
        let levels = analyzer.levels();
        assert!(levels.high > 0.8, "{levels:?}");
        assert!(levels.bass < 0.2, "{levels:?}");
    }

    #[test]
    fn kick_pattern_gives_beats_and_tempo() {
        let mut analyzer = Analyzer::new(RATE);
        let period = 60.0 / 120.0;
        // Decaying 60 Hz thump every half second.
        feed(&mut analyzer, 12.0, |t| {
            let since = t % period;
            (std::f32::consts::TAU * 60.0 * t).sin() * (-since * 25.0).exp() * 0.8
        });
        let levels = analyzer.levels();
        assert!((22..=25).contains(&levels.beats), "beats {}", levels.beats);
        let bpm = levels.bpm.expect("tempo found");
        assert!((bpm - 120.0).abs() < 2.0, "bpm {bpm}");
    }

    #[test]
    fn silence_is_quiet() {
        let mut analyzer = Analyzer::new(RATE);
        feed(&mut analyzer, 1.0, |_| 0.0);
        let levels = analyzer.levels();
        assert_eq!(levels.beats, 0);
        assert_eq!(levels.bpm, None);
        assert!(levels.bass < 0.01);
    }
}
