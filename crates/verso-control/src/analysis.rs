//! Audio analysis: band levels, beat detection and tempo estimate.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};
use serde::{Deserialize, Serialize};

const FRAME: usize = 1024;
const HOP: usize = 512;
/// Latest raw samples kept for waveform consumers (MilkDrop).
pub const WAVEFORM_LEN: usize = 1024;
/// Seconds of onset history used for the tempo estimate.
const TEMPO_WINDOW: f32 = 8.0;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;
/// Bands of the spectrum display, log-spaced from 20 Hz to 20 kHz.
pub const SPECTRUM_BANDS: usize = 64;
pub const SPECTRUM_LOW_HZ: f32 = 20.0;
pub const SPECTRUM_HIGH_HZ: f32 = 20_000.0;
/// Range of the spectrum display: this many dB below full level is 0.
const SPECTRUM_RANGE_DB: f32 = 60.0;
/// Without auto gain, a sine of half full scale fills a band at 0 dB gain.
const BAND_REFERENCE: f32 = FRAME as f32 / 8.0;
/// Without auto gain, the overall level of that sine at 0 dB.
const LEVEL_REFERENCE: f32 = 0.5 * std::f32::consts::FRAC_1_SQRT_2;

/// How the band levels are scaled, as in Resolume's audio analysis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisSettings {
    /// Each band against its own recent peak, so quiet and loud inputs both
    /// use the whole range. Off, levels follow the input level.
    pub auto_gain: bool,
    /// Gain in dB, applied before levels are clipped to 1.
    pub gain_db: f32,
    /// Upper edge of the bass band, in Hz.
    pub bass_mid: f32,
    /// Lower edge of the high band, in Hz.
    pub mid_high: f32,
    /// Seconds a level takes to fall by half once the sound stops.
    pub falloff: f32,
}

impl Default for AnalysisSettings {
    fn default() -> Self {
        Self {
            auto_gain: true,
            gain_db: 0.0,
            bass_mid: 150.0,
            mid_high: 2000.0,
            falloff: 0.15,
        }
    }
}

impl AnalysisSettings {
    pub const GAIN_DB: std::ops::RangeInclusive<f32> = -24.0..=24.0;
    pub const FALLOFF: std::ops::RangeInclusive<f32> = 0.02..=2.0;
    /// Range of both crossovers.
    pub const CROSSOVER: std::ops::RangeInclusive<f32> = 40.0..=16_000.0;

    /// The settings with every value in range and the bass band below the
    /// high band.
    pub fn sanitized(self) -> Self {
        let default = Self::default();
        let clamp = |v: f32, range: &std::ops::RangeInclusive<f32>, d: f32| {
            if v.is_finite() {
                v.clamp(*range.start(), *range.end())
            } else {
                d
            }
        };
        let bass_mid = clamp(self.bass_mid, &Self::CROSSOVER, default.bass_mid)
            .min(Self::CROSSOVER.end() / 1.25);
        let mid_high =
            clamp(self.mid_high, &Self::CROSSOVER, default.mid_high).max(bass_mid * 1.25);
        Self {
            auto_gain: self.auto_gain,
            gain_db: clamp(self.gain_db, &Self::GAIN_DB, 0.0),
            bass_mid,
            mid_high,
            falloff: clamp(self.falloff, &Self::FALLOFF, default.falloff),
        }
    }

    fn gain(&self) -> f32 {
        10f32.powf(self.gain_db / 20.0)
    }
}

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
    settings: AnalysisSettings,
    /// Spectrum display, each band in 0..1 on a dB scale.
    spectrum: [f32; SPECTRUM_BANDS],
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
            settings: AnalysisSettings::default(),
            spectrum: [0.0; SPECTRUM_BANDS],
        }
    }

    pub fn set_settings(&mut self, settings: AnalysisSettings) {
        self.settings = settings.sanitized();
    }

    /// Spectrum display: [`SPECTRUM_BANDS`] log-spaced bands, each in 0..1
    /// over the last 60 dB, after the gain.
    pub fn spectrum(&self) -> [f32; SPECTRUM_BANDS] {
        self.spectrum
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
        // Bins from `low` to `high` Hz, at least one so narrow bands at the
        // bottom of the spectrum still show something.
        let band = |low: f32, high: f32| -> f32 {
            let a = ((low / bin_hz) as usize).clamp(1, spectrum.len() - 1);
            let b = ((high / bin_hz) as usize).clamp(a + 1, spectrum.len());
            spectrum[a..b]
                .iter()
                .map(|c| c.norm_sqr())
                .sum::<f32>()
                .sqrt()
        };
        let settings = self.settings;
        let raw = [
            rms,
            band(20.0, settings.bass_mid),
            band(settings.bass_mid, settings.mid_high),
            band(settings.mid_high, 16000.0),
        ];

        // Auto gain: each band against its own decaying peak, but never against
        // less than a quarter of the loudest band, so a band holding only
        // spectral leakage stays low. Without it, against a fixed level.
        let peak_decay = self.decay(10.0);
        for (peak, value) in self.peaks.iter_mut().zip(raw) {
            *peak = (*peak * peak_decay).max(value).max(1e-6);
        }
        let loudest = self.peaks[1..].iter().cloned().fold(0.0, f32::max);
        let gain = settings.gain();
        let release = self.decay(settings.falloff);
        for (i, (smoothed, value)) in self.smoothed.iter_mut().zip(raw).enumerate() {
            let reference = match (settings.auto_gain, i) {
                (true, 0) => self.peaks[0],
                (true, _) => self.peaks[i].max(loudest * 0.25),
                (false, 0) => LEVEL_REFERENCE,
                (false, _) => BAND_REFERENCE,
            };
            let value = (value / reference * gain).clamp(0.0, 1.0);
            *smoothed = value.max(*smoothed * release);
        }

        let ratio = (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(1.0 / SPECTRUM_BANDS as f32);
        for (i, shown) in self.spectrum.iter_mut().enumerate() {
            let low = SPECTRUM_LOW_HZ * ratio.powi(i as i32);
            let level = band(low, low * ratio) / BAND_REFERENCE * gain;
            let db = 20.0 * level.max(1e-9).log10();
            let value = (1.0 + db / SPECTRUM_RANGE_DB).clamp(0.0, 1.0);
            *shown = value.max(*shown * release);
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

    fn tone(analyzer: &mut Analyzer, hz: f32, amplitude: f32) {
        feed(analyzer, 1.0, |t| {
            amplitude * (std::f32::consts::TAU * hz * t).sin()
        });
    }

    #[test]
    fn manual_gain_follows_the_input_level() {
        let manual = AnalysisSettings {
            auto_gain: false,
            ..Default::default()
        };
        let mut analyzer = Analyzer::new(RATE);
        analyzer.set_settings(manual);
        tone(&mut analyzer, 80.0, 0.5);
        let full = analyzer.levels().bass;
        assert!(full > 0.85, "{full}");

        // A quarter of the amplitude reads a quarter, and 12 dB of gain
        // brings it back.
        let mut analyzer = Analyzer::new(RATE);
        analyzer.set_settings(manual);
        tone(&mut analyzer, 80.0, 0.125);
        let quiet = analyzer.levels().bass;
        assert!((quiet / full - 0.25).abs() < 0.05, "{quiet} vs {full}");
        analyzer.set_settings(AnalysisSettings {
            gain_db: 12.04,
            ..manual
        });
        tone(&mut analyzer, 80.0, 0.125);
        let boosted = analyzer.levels().bass;
        assert!((boosted - full).abs() < 0.05, "{boosted} vs {full}");
    }

    #[test]
    fn crossovers_move_the_bands() {
        let mut analyzer = Analyzer::new(RATE);
        tone(&mut analyzer, 400.0, 0.5);
        assert!(analyzer.levels().mid > 0.8, "{:?}", analyzer.levels());
        assert!(analyzer.levels().bass < 0.3, "{:?}", analyzer.levels());

        let mut analyzer = Analyzer::new(RATE);
        analyzer.set_settings(AnalysisSettings {
            bass_mid: 600.0,
            ..Default::default()
        });
        tone(&mut analyzer, 400.0, 0.5);
        assert!(analyzer.levels().bass > 0.8, "{:?}", analyzer.levels());
        assert!(analyzer.levels().mid < 0.3, "{:?}", analyzer.levels());
    }

    #[test]
    fn spectrum_peaks_at_the_tone() {
        let mut analyzer = Analyzer::new(RATE);
        tone(&mut analyzer, 1000.0, 0.5);
        let spectrum = analyzer.spectrum();
        let peak = (0..SPECTRUM_BANDS)
            .max_by(|&a, &b| spectrum[a].total_cmp(&spectrum[b]))
            .unwrap();
        let ratio = (SPECTRUM_HIGH_HZ / SPECTRUM_LOW_HZ).powf(1.0 / SPECTRUM_BANDS as f32);
        let low = SPECTRUM_LOW_HZ * ratio.powi(peak as i32);
        assert!(
            (low * 0.8..=low * ratio * 1.25).contains(&1000.0),
            "band {peak}: {low} Hz"
        );
        assert!(spectrum[peak] > 0.9, "{}", spectrum[peak]);
        assert!(spectrum[0] < 0.5, "{}", spectrum[0]);
    }

    #[test]
    fn settings_are_kept_in_range() {
        let s = AnalysisSettings {
            gain_db: f32::NAN,
            bass_mid: 5000.0,
            mid_high: 100.0,
            falloff: 99.0,
            ..Default::default()
        }
        .sanitized();
        assert_eq!(s.gain_db, 0.0);
        assert!(s.bass_mid < s.mid_high);
        assert_eq!(s.falloff, 2.0);
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
