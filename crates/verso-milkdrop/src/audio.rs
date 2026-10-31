//! Audio data as MilkDrop sees it: an undersampled waveform, an equalized
//! spectrum and bass/mid/treble levels relative to their long-term average.
//! Port of Butterchurn's `audioProcessor`, `fft` and `audioLevels`.

/// Samples of the FFT input window.
pub const FFT_SIZE: usize = 1024;
/// Waveform and spectrum length handed to presets.
pub const NUM_SAMPS: usize = 512;

/// Per-frame audio for one MilkDrop instance.
#[derive(Debug, Clone)]
pub struct AudioState {
    /// Waveform in MilkDrop units (about -128..127), left and right.
    pub time_l: Vec<f32>,
    pub time_r: Vec<f32>,
    /// Equalized magnitude spectrum, left and right.
    pub freq_l: Vec<f32>,
    pub freq_r: Vec<f32>,
    pub bass: f32,
    pub bass_att: f32,
    pub mid: f32,
    pub mid_att: f32,
    pub treb: f32,
    pub treb_att: f32,
    fft: Fft,
    starts: [usize; 3],
    stops: [usize; 3],
    avg: [f32; 3],
    long_avg: [f32; 3],
}

impl AudioState {
    pub fn new(sample_rate: f32) -> Self {
        let bucket_hz = sample_rate / FFT_SIZE as f32;
        let bin = |hz: f32| ((hz / bucket_hz).round() as i64 - 1).clamp(0, NUM_SAMPS as i64 - 1) as usize;
        let (bass_low, bass_high, mid_high, treb_high) = (bin(20.0), bin(320.0), bin(2800.0), bin(11025.0));
        AudioState {
            time_l: vec![0.0; NUM_SAMPS],
            time_r: vec![0.0; NUM_SAMPS],
            freq_l: vec![0.0; NUM_SAMPS],
            freq_r: vec![0.0; NUM_SAMPS],
            bass: 1.0,
            bass_att: 1.0,
            mid: 1.0,
            mid_att: 1.0,
            treb: 1.0,
            treb_att: 1.0,
            fft: Fft::new(FFT_SIZE, NUM_SAMPS, true),
            starts: [bass_low, bass_high, mid_high],
            stops: [bass_high, mid_high, treb_high],
            avg: [1.0; 3],
            long_avg: [1.0; 3],
        }
    }

    /// Updates from the latest mono samples in -1..1 (the last
    /// [`FFT_SIZE`] are used; fewer are zero padded at the start).
    pub fn update(&mut self, samples: &[f32], fps: f32, frame: u64) {
        let mut time = [0.0f32; FFT_SIZE];
        let tail = &samples[samples.len().saturating_sub(FFT_SIZE)..];
        let offset = FFT_SIZE - tail.len();
        for (dst, s) in time[offset..].iter_mut().zip(tail) {
            // Butterchurn works on signed bytes.
            *dst = (s.clamp(-1.0, 1.0) * 127.0).round();
        }
        let mut last = 0;
        let mut j = 0;
        for i in 0..FFT_SIZE {
            let smoothed = 0.5 * (time[i] + time[last]);
            if i % 2 == 0 {
                self.time_l[j] = smoothed.trunc();
                j += 1;
            }
            last = i;
        }
        self.time_r.copy_from_slice(&self.time_l);
        self.freq_l = self.fft.time_to_frequency(&time);
        self.freq_r.copy_from_slice(&self.freq_l);
        self.update_levels(fps, frame);
    }

    fn update_levels(&mut self, fps: f32, frame: u64) {
        let fps = if fps.is_finite() { fps.clamp(15.0, 144.0) } else { 15.0 };
        let adjust = |rate: f32| rate.powf(30.0 / fps);
        let mut val = [0.0; 3];
        let mut att = [0.0; 3];
        for i in 0..3 {
            let imm: f32 = self.freq_l[self.starts[i]..self.stops[i]].iter().sum();
            let rate = adjust(if imm > self.avg[i] { 0.2 } else { 0.5 });
            self.avg[i] = self.avg[i] * rate + imm * (1.0 - rate);
            let rate = adjust(if frame < 50 { 0.9 } else { 0.992 });
            self.long_avg[i] = self.long_avg[i] * rate + imm * (1.0 - rate);
            if self.long_avg[i] < 0.001 {
                val[i] = 1.0;
                att[i] = 1.0;
            } else {
                val[i] = imm / self.long_avg[i];
                att[i] = self.avg[i] / self.long_avg[i];
            }
        }
        [self.bass, self.mid, self.treb] = val;
        [self.bass_att, self.mid_att, self.treb_att] = att;
    }
}

/// Radix-2 FFT with MilkDrop's equalization curve.
#[derive(Debug, Clone)]
struct Fft {
    samples_in: usize,
    samples_out: usize,
    nfreq: usize,
    equalize: Option<Vec<f32>>,
    bitrev: Vec<usize>,
    cossin: Vec<(f32, f32)>,
}

impl Fft {
    fn new(samples_in: usize, samples_out: usize, equalize: bool) -> Self {
        let nfreq = samples_out * 2;
        let equalize = equalize.then(|| {
            let inv = 1.0 / samples_out as f32;
            (0..samples_out)
                .map(|i| -0.02 * ((samples_out - i) as f32 * inv).ln())
                .collect()
        });
        let mut bitrev: Vec<usize> = (0..nfreq).collect();
        let mut j = 0;
        for i in 0..nfreq {
            if j > i {
                bitrev.swap(i, j);
            }
            let mut m = nfreq >> 1;
            while m >= 1 && j >= m {
                j -= m;
                m >>= 1;
            }
            j += m;
        }
        let mut cossin = Vec::new();
        let mut dftsize = 2;
        while dftsize <= nfreq {
            let theta = -2.0 * std::f32::consts::PI / dftsize as f32;
            cossin.push((theta.cos(), theta.sin()));
            dftsize <<= 1;
        }
        Fft {
            samples_in,
            samples_out,
            nfreq,
            equalize,
            bitrev,
            cossin,
        }
    }

    fn time_to_frequency(&self, input: &[f32]) -> Vec<f32> {
        let n = self.nfreq;
        let mut real = vec![0.0f32; n];
        let mut imag = vec![0.0f32; n];
        for (r, &idx) in real.iter_mut().zip(&self.bitrev) {
            *r = if idx < self.samples_in { input[idx] } else { 0.0 };
        }
        let mut dftsize = 2;
        let mut t = 0;
        while dftsize <= n {
            let (wpr, wpi) = self.cossin[t];
            let (mut wr, mut wi) = (1.0f32, 0.0f32);
            let half = dftsize >> 1;
            for m in 0..half {
                let mut i = m;
                while i < n {
                    let j = i + half;
                    let tr = wr * real[j] - wi * imag[j];
                    let ti = wr * imag[j] + wi * real[j];
                    real[j] = real[i] - tr;
                    imag[j] = imag[i] - ti;
                    real[i] += tr;
                    imag[i] += ti;
                    i += dftsize;
                }
                let wtemp = wr;
                wr = wtemp * wpr - wi * wpi;
                wi = wi * wpr + wtemp * wpi;
            }
            dftsize <<= 1;
            t += 1;
        }
        (0..self.samples_out)
            .map(|i| {
                let mag = (real[i] * real[i] + imag[i] * imag[i]).sqrt();
                self.equalize.as_ref().map_or(mag, |eq| eq[i] * mag)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_silence_settles_levels_at_one() {
        let mut audio = AudioState::new(44100.0);
        for frame in 0..3000 {
            audio.update(&[0.0; FFT_SIZE], 60.0, frame);
        }
        assert_eq!(audio.bass, 1.0);
        assert!(audio.freq_l.iter().all(|&f| f == 0.0));
    }

    #[test]
    fn bass_tone_raises_bass() {
        let mut audio = AudioState::new(44100.0);
        let quiet: Vec<f32> = (0..FFT_SIZE).map(|i| 0.01 * (i as f32 * 0.3).sin()).collect();
        for frame in 0..120 {
            audio.update(&quiet, 60.0, frame);
        }
        let loud: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.8 * (i as f32 * 2.0 * std::f32::consts::PI * 80.0 / 44100.0).sin())
            .collect();
        audio.update(&loud, 60.0, 121);
        assert!(audio.bass > 2.0, "bass {}", audio.bass);
        assert!(audio.time_l.iter().any(|&v| v.abs() > 50.0));
    }
}
