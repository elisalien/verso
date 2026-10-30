//! Parameter modulation: LFOs synced to the tempo and audio-reactive signals.
//!
//! Modulators never change the stored composition. Each frame, [`modulate`]
//! returns a copy with modulated values, and only that copy is rendered.

use serde::{Deserialize, Serialize};

use crate::Composition;
use crate::control::{Param, Range};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Waveform {
    Sine,
    Triangle,
    SawUp,
    Square,
    /// A new random value each cycle.
    Random,
}

impl Waveform {
    pub const ALL: [Waveform; 5] = [
        Waveform::Sine,
        Waveform::Triangle,
        Waveform::SawUp,
        Waveform::Square,
        Waveform::Random,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Waveform::Sine => "Sine",
            Waveform::Triangle => "Triangle",
            Waveform::SawUp => "Saw",
            Waveform::Square => "Square",
            Waveform::Random => "Random",
        }
    }

    /// Value in 0..1 at `phase` cycles.
    pub fn at(self, phase: f64) -> f32 {
        let t = phase.rem_euclid(1.0) as f32;
        match self {
            Waveform::Sine => 0.5 - 0.5 * (t * std::f32::consts::TAU).cos(),
            Waveform::Triangle => 1.0 - (2.0 * t - 1.0).abs(),
            Waveform::SawUp => t,
            Waveform::Square => {
                if t < 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Waveform::Random => {
                // Hash of the cycle number: stable within a cycle.
                let cycle = phase.floor() as i64 as u64;
                let mut x = cycle.wrapping_mul(0x9E37_79B9_7F4A_7C15);
                x ^= x >> 31;
                x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                x ^= x >> 29;
                (x >> 40) as f32 / (1u64 << 24) as f32
            }
        }
    }
}

/// Audio analysis signals, each in 0..1.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AudioBand {
    Level,
    Bass,
    Mid,
    High,
    /// Pulse on detected beats, decaying quickly.
    Beat,
}

impl AudioBand {
    pub const ALL: [AudioBand; 5] = [
        AudioBand::Level,
        AudioBand::Bass,
        AudioBand::Mid,
        AudioBand::High,
        AudioBand::Beat,
    ];

    pub fn name(self) -> &'static str {
        match self {
            AudioBand::Level => "Level",
            AudioBand::Bass => "Bass",
            AudioBand::Mid => "Mid",
            AudioBand::High => "High",
            AudioBand::Beat => "Beat",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ModSource {
    /// One cycle every `beats` beats of the composition tempo.
    Lfo {
        waveform: Waveform,
        beats: f32,
    },
    Audio {
        band: AudioBand,
    },
}

/// Moves a parameter by `depth` of its range following `source`. With a
/// positive depth the parameter rises from its set value; negative falls.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modulator {
    pub param: Param,
    pub source: ModSource,
    /// -1..1, as a fraction of the parameter range.
    pub depth: f32,
    #[serde(default)]
    pub bypass: bool,
}

/// Inputs of modulation for one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Signals {
    /// Musical time in beats since start.
    pub beats: f64,
    /// Indexed by [`AudioBand`].
    pub audio: [f32; 5],
}

impl Signals {
    pub fn audio(&self, band: AudioBand) -> f32 {
        self.audio[band.index()]
    }
}

impl ModSource {
    /// Signal value in 0..1.
    pub fn value(&self, signals: &Signals) -> f32 {
        match *self {
            ModSource::Lfo { waveform, beats } => {
                waveform.at(signals.beats / beats.max(1e-3) as f64)
            }
            ModSource::Audio { band } => signals.audio(band).clamp(0.0, 1.0),
        }
    }
}

/// Copy of `comp` with every active modulator applied. `range` gives the range
/// of parameters that have no built-in one (effect parameters) together with
/// their current value, since defaults live in the effect library.
pub fn modulate(
    comp: &Composition,
    signals: &Signals,
    range: impl Fn(&Param) -> Option<(Range, f32)>,
) -> Composition {
    let mut out = comp.clone();
    for modulator in comp.modulators.iter().filter(|m| !m.bypass) {
        let param = &modulator.param;
        let resolved = match param.builtin_range() {
            Some(r) => param.get(comp).map(|v| (r, v)),
            None => range(param),
        };
        let Some((range, base)) = resolved else {
            continue;
        };
        // Several modulators on one parameter add up.
        let current = param.get(&out).unwrap_or(base);
        let offset = modulator.depth * modulator.source.value(signals) * (range.max - range.min);
        let value = (current + offset).clamp(range.min.min(range.max), range.max.max(range.min));
        param.write(&mut out, value, range);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, EffectEdit, EffectTarget};

    #[test]
    fn waveforms_span_unit_range() {
        for waveform in Waveform::ALL {
            for i in 0..=40 {
                let v = waveform.at(i as f64 / 10.0);
                assert!((0.0..=1.0).contains(&v), "{waveform:?} {v}");
            }
        }
        assert_eq!(Waveform::Sine.at(0.0), 0.0);
        assert!((Waveform::Sine.at(0.5) - 1.0).abs() < 1e-6);
        assert_eq!(Waveform::Square.at(0.25), 1.0);
        assert_eq!(Waveform::Random.at(3.1), Waveform::Random.at(3.9));
    }

    #[test]
    fn lfo_follows_beats() {
        let mut comp = Composition::demo();
        comp.layers[0].opacity = 0.0;
        comp.modulators.push(Modulator {
            param: Param::LayerOpacity(0),
            source: ModSource::Lfo {
                waveform: Waveform::SawUp,
                beats: 4.0,
            },
            depth: 1.0,
            bypass: false,
        });
        let at = |beats| {
            let signals = Signals {
                beats,
                ..Default::default()
            };
            modulate(&comp, &signals, |_| None).layers[0].opacity
        };
        assert_eq!(at(0.0), 0.0);
        assert_eq!(at(2.0), 0.5);
        assert_eq!(at(3.0), 0.75);
        // The stored composition is untouched.
        assert_eq!(comp.layers[0].opacity, 0.0);
    }

    #[test]
    fn audio_modulation_is_clamped_and_bypassable() {
        let mut comp = Composition::demo();
        comp.master.opacity = 0.8;
        comp.modulators.push(Modulator {
            param: Param::Master,
            source: ModSource::Audio {
                band: AudioBand::Bass,
            },
            depth: 0.5,
            bypass: false,
        });
        let mut signals = Signals::default();
        signals.audio[AudioBand::Bass as usize] = 1.0;
        assert_eq!(modulate(&comp, &signals, |_| None).master.opacity, 1.0);
        comp.modulators[0].depth = -0.5;
        assert!((modulate(&comp, &signals, |_| None).master.opacity - 0.3).abs() < 1e-6);
        comp.modulators[0].bypass = true;
        assert_eq!(modulate(&comp, &signals, |_| None).master.opacity, 0.8);
    }

    #[test]
    fn effect_params_use_supplied_range() {
        let mut comp = Composition::demo();
        let target = EffectTarget::Layer(0);
        comp.apply(Command::EditEffects {
            target,
            edit: EffectEdit::Add(crate::Effect::new("builtin:blur", "Blur")),
        })
        .unwrap();
        let param = Param::Effect {
            target,
            index: 0,
            name: "radius".into(),
        };
        comp.modulators.push(Modulator {
            param: param.clone(),
            source: ModSource::Audio {
                band: AudioBand::Level,
            },
            depth: 1.0,
            bypass: false,
        });
        let mut signals = Signals::default();
        signals.audio[0] = 0.5;
        let range = Range {
            min: 0.0,
            max: 2.0,
            toggle: false,
        };
        let out = modulate(&comp, &signals, |_| Some((range, 0.2)));
        // Default 0.2 plus half of a range of 2.
        assert_eq!(param.get(&out), Some(1.2));
    }
}
