//! Audio capture from an input device, analysed on the audio thread.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};

use crate::timecode::{LtcDecoder, Timecode, TimecodeClock};
use crate::{AnalysisSettings, Analyzer, Error, Levels, Result, SPECTRUM_BANDS};

/// LTC reading on one input channel.
struct Ltc {
    channel: usize,
    decoder: LtcDecoder,
    clock: TimecodeClock,
}

/// Names of the input devices.
pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.input_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// Prefix of a device name that captures what an output device plays
/// (loopback), followed by the output device name.
pub const LOOPBACK_PREFIX: &str = "System sound: ";

/// Loopback choice following whatever output Windows plays to. The device
/// is chosen when capture starts; [`AudioInput::default_output_moved`] tells
/// when Windows has switched to another one since.
pub const DEFAULT_OUTPUT: &str = "Default output";

fn default_output_name() -> Option<String> {
    cpal::default_host()
        .default_output_device()
        .and_then(|d| d.description().ok().map(|d| d.name().to_owned()))
}

/// The output device name of a loopback choice, or `None` for an input.
pub fn loopback_output(name: &str) -> Option<&str> {
    name.strip_prefix(LOOPBACK_PREFIX)
}

/// Loopback choices: each output device, captured as an input. Only Windows
/// (WASAPI) can capture an output; on Linux the "Monitor of" sources of
/// PulseAudio/PipeWire already appear as inputs.
pub fn loopback_devices() -> Vec<String> {
    if cfg!(windows) {
        let mut outputs = output_devices();
        // The default output first: usually the one playing the music.
        let default = cpal::default_host()
            .default_output_device()
            .and_then(|d| d.description().ok().map(|d| d.name().to_owned()));
        if let Some(index) = default.and_then(|name| outputs.iter().position(|n| *n == name)) {
            let name = outputs.remove(index);
            outputs.insert(0, name);
        }
        // First, the default output, whichever device it is now.
        std::iter::once(DEFAULT_OUTPUT.to_owned())
            .chain(outputs)
            .map(|name| format!("{LOOPBACK_PREFIX}{name}"))
            .collect()
    } else {
        Vec::new()
    }
}

/// Names of the output devices.
pub fn output_devices() -> Vec<String> {
    let host = cpal::default_host();
    host.output_devices()
        .map(|devices| {
            devices
                .filter_map(|d| d.description().ok().map(|d| d.name().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

/// The output device named `name`, or the default output.
fn output_device(name: Option<&str>) -> Result<cpal::Device> {
    let host = cpal::default_host();
    match name {
        Some(name) => host
            .output_devices()
            .map_err(|e| Error::Audio(e.to_string()))?
            .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
            .ok_or_else(|| Error::Audio(format!("no output named {name}"))),
        None => host
            .default_output_device()
            .ok_or_else(|| Error::Audio("no default output device".into())),
    }
}

/// A short tone on an output device, to check it is the right one. Dropping
/// it stops the sound.
pub struct TestTone {
    _stream: cpal::Stream,
}

impl TestTone {
    /// Plays a quiet 440 Hz tone on the output named `name`, or the default
    /// output, until dropped.
    pub fn start(name: Option<&str>) -> Result<Self> {
        let device = output_device(name)?;
        let supported = device
            .default_output_config()
            .map_err(|e| Error::Audio(e.to_string()))?;
        let config = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => tone::<f32>(&device, &config, |s| s),
            SampleFormat::I16 => tone::<i16>(&device, &config, |s| (s * i16::MAX as f32) as i16),
            SampleFormat::U16 => tone::<u16>(&device, &config, |s| {
                ((s * 0.5 + 0.5) * u16::MAX as f32) as u16
            }),
            SampleFormat::I32 => tone::<i32>(&device, &config, |s| (s * i32::MAX as f32) as i32),
            other => return Err(Error::Audio(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(|e| Error::Audio(e.to_string()))?;
        Ok(Self { _stream: stream })
    }
}

fn tone<T: SizedSample + Copy + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    from_f32: fn(f32) -> T,
) -> Result<cpal::Stream> {
    let channels = config.channels.max(1) as usize;
    let step = std::f32::consts::TAU * 440.0 / config.sample_rate as f32;
    let mut phase = 0.0f32;
    device
        .build_output_stream::<T, _, _>(
            *config,
            move |data: &mut [T], _| {
                for frame in data.chunks_mut(channels) {
                    let sample = from_f32(0.2 * phase.sin());
                    frame.fill(sample);
                    phase = (phase + step) % std::f32::consts::TAU;
                }
            },
            |_| {},
            None,
        )
        .map_err(|e| Error::Audio(e.to_string()))
}

/// A running capture. Dropping it stops the stream.
pub struct AudioInput {
    _stream: cpal::Stream,
    analyzer: Arc<Mutex<Analyzer>>,
    ltc: Arc<Mutex<Option<Ltc>>>,
    sample_rate: u32,
    channels: usize,
    name: String,
    /// Output device captured as "the default output", by name.
    default_output: Option<String>,
    /// Last error reported by the stream.
    error: Arc<Mutex<Option<String>>>,
}

impl AudioInput {
    /// Starts capturing from the device named `name`, or the default input.
    /// A name starting with [`LOOPBACK_PREFIX`] captures what that output
    /// device plays (Windows only).
    pub fn start(name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let loopback = name.and_then(loopback_output);
        let device = match (name, loopback) {
            (_, Some(output)) => {
                if !cfg!(windows) {
                    return Err(Error::Audio(
                        "capturing system sound needs Windows; choose a monitor input".into(),
                    ));
                }
                output_device(Some(output).filter(|o| *o != DEFAULT_OUTPUT))?
            }
            (Some(name), None) => host
                .input_devices()
                .map_err(|e| Error::Audio(e.to_string()))?
                .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
                .ok_or_else(|| Error::Audio(format!("no input named {name}")))?,
            (None, None) => host
                .default_input_device()
                .ok_or_else(|| Error::Audio("no default input device".into()))?,
        };
        let device_name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "Audio input".into());
        let default_output = (loopback == Some(DEFAULT_OUTPUT)).then(|| device_name.clone());
        let name = if loopback == Some(DEFAULT_OUTPUT) {
            format!("{LOOPBACK_PREFIX}{DEFAULT_OUTPUT} ({device_name})")
        } else if loopback.is_some() {
            format!("{LOOPBACK_PREFIX}{device_name}")
        } else {
            device_name
        };
        // Loopback reads the output at its own format: an output device has
        // no input config.
        let supported = if loopback.is_some() {
            device.default_output_config()
        } else {
            device.default_input_config()
        }
        .map_err(|e| Error::Audio(e.to_string()))?;
        let config = supported.config();
        let analyzer = Arc::new(Mutex::new(Analyzer::new(config.sample_rate)));
        let ltc = Arc::new(Mutex::new(None));
        let error = Arc::new(Mutex::new(None));
        let shared = Shared {
            analyzer: analyzer.clone(),
            ltc: ltc.clone(),
            error: error.clone(),
        };
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, shared, |s| s),
            SampleFormat::I16 => {
                build::<i16>(&device, &config, shared, |s| s as f32 / i16::MAX as f32)
            }
            SampleFormat::U16 => build::<u16>(&device, &config, shared, |s| {
                s as f32 / u16::MAX as f32 * 2.0 - 1.0
            }),
            SampleFormat::I32 => {
                build::<i32>(&device, &config, shared, |s| s as f32 / i32::MAX as f32)
            }
            other => return Err(Error::Audio(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(|e| Error::Audio(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            analyzer,
            ltc,
            sample_rate: config.sample_rate,
            channels: config.channels.max(1) as usize,
            name,
            default_output,
            error,
        })
    }

    /// Whether this captures "the default output" and Windows now plays to
    /// another device (a projector plugged in over HDMI, headphones...): the
    /// capture must be started again to follow it. Asks the system, so call
    /// it every few seconds, not every frame.
    pub fn default_output_moved(&self) -> bool {
        match &self.default_output {
            Some(captured) => default_output_name().is_some_and(|now| now != *captured),
            None => false,
        }
    }

    /// Last error the stream reported (device unplugged, format lost...).
    pub fn error(&self) -> Option<String> {
        self.error.lock().ok()?.clone()
    }

    /// Number of input channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Reads LTC timecode from `channel` (0-based), or stops with `None`.
    pub fn set_ltc_channel(&mut self, channel: Option<usize>) {
        if let Ok(mut ltc) = self.ltc.lock() {
            *ltc = channel.filter(|&c| c < self.channels).map(|channel| Ltc {
                channel,
                decoder: LtcDecoder::new(self.sample_rate),
                clock: TimecodeClock::default(),
            });
        }
    }

    /// Latest LTC timecode, with its clock for smooth positions.
    pub fn ltc(&self) -> Option<(Timecode, TimecodeClock)> {
        let ltc = self.ltc.lock().ok()?;
        let ltc = ltc.as_ref()?;
        Some((ltc.clock.current()?, ltc.clock))
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn set_settings(&self, settings: AnalysisSettings) {
        if let Ok(mut analyzer) = self.analyzer.lock() {
            analyzer.set_settings(settings);
        }
    }

    /// Spectrum display, see [`Analyzer::spectrum`].
    pub fn spectrum(&self) -> [f32; SPECTRUM_BANDS] {
        self.analyzer
            .lock()
            .map(|a| a.spectrum())
            .unwrap_or([0.0; SPECTRUM_BANDS])
    }

    pub fn levels(&self) -> Levels {
        self.analyzer.lock().map(|a| a.levels()).unwrap_or_default()
    }

    /// The last mono samples and their sample rate.
    pub fn waveform(&self) -> (Vec<f32>, f32) {
        self.analyzer
            .lock()
            .map(|a| (a.waveform(), a.sample_rate()))
            .unwrap_or_else(|_| (Vec::new(), 44100.0))
    }
}

/// State the audio callback writes to.
struct Shared {
    analyzer: Arc<Mutex<Analyzer>>,
    ltc: Arc<Mutex<Option<Ltc>>>,
    error: Arc<Mutex<Option<String>>>,
}

fn build<T: SizedSample + Copy + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Shared,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream> {
    let channels = config.channels.max(1) as usize;
    let mut mono = Vec::new();
    let mut channel = Vec::new();
    let Shared {
        analyzer,
        ltc,
        error,
    } = shared;
    device
        .build_input_stream::<T, _, _>(
            *config,
            move |data: &[T], _| {
                mono.clear();
                mono.extend(
                    data.chunks(channels).map(|frame| {
                        frame.iter().map(|&s| to_f32(s)).sum::<f32>() / channels as f32
                    }),
                );
                if let Ok(mut analyzer) = analyzer.lock() {
                    analyzer.push(&mono);
                }
                if let Ok(mut ltc) = ltc.lock()
                    && let Some(ltc) = ltc.as_mut()
                {
                    channel.clear();
                    channel.extend(
                        data.chunks(channels)
                            .filter_map(|frame| frame.get(ltc.channel).map(|&s| to_f32(s))),
                    );
                    if let Some(tc) = ltc.decoder.push(&channel).pop() {
                        ltc.clock.set(tc);
                    }
                }
            },
            move |e| {
                // A buffer under- or overrun is a passing glitch: the stream
                // goes on, so it is not reported as a failure.
                if e.kind() == cpal::ErrorKind::Xrun {
                    return;
                }
                if let Ok(mut error) = error.lock() {
                    *error = Some(e.to_string());
                }
            },
            None,
        )
        .map_err(|e| Error::Audio(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_prefix_parsing() {
        assert_eq!(
            loopback_output(&format!("{LOOPBACK_PREFIX}Speakers (Realtek)")),
            Some("Speakers (Realtek)")
        );
        assert_eq!(loopback_output("Microphone (USB)"), None);
        assert_eq!(loopback_output("System sound:Speakers"), None);
    }

    #[test]
    fn loopback_devices_are_prefixed() {
        let devices = loopback_devices();
        if !cfg!(windows) {
            assert!(devices.is_empty());
        }
        assert!(devices.iter().all(|d| loopback_output(d).is_some()));
    }
}
