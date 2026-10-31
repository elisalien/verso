//! Audio capture from an input device, analysed on the audio thread.

use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, SizedSample};

use crate::{Analyzer, Error, Levels, Result};

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

/// A running capture. Dropping it stops the stream.
pub struct AudioInput {
    _stream: cpal::Stream,
    analyzer: Arc<Mutex<Analyzer>>,
    name: String,
}

impl AudioInput {
    /// Starts capturing from the device named `name`, or the default input.
    pub fn start(name: Option<&str>) -> Result<Self> {
        let host = cpal::default_host();
        let device = match name {
            Some(name) => host
                .input_devices()
                .map_err(|e| Error::Audio(e.to_string()))?
                .find(|d| d.description().is_ok_and(|desc| desc.name() == name))
                .ok_or_else(|| Error::Audio(format!("no input named {name}")))?,
            None => host
                .default_input_device()
                .ok_or_else(|| Error::Audio("no default input device".into()))?,
        };
        let name = device
            .description()
            .map(|d| d.name().to_owned())
            .unwrap_or_else(|_| "Audio input".into());
        let supported = device
            .default_input_config()
            .map_err(|e| Error::Audio(e.to_string()))?;
        let config = supported.config();
        let analyzer = Arc::new(Mutex::new(Analyzer::new(config.sample_rate)));
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, &config, analyzer.clone(), |s| s),
            SampleFormat::I16 => build::<i16>(&device, &config, analyzer.clone(), |s| {
                s as f32 / i16::MAX as f32
            }),
            SampleFormat::U16 => build::<u16>(&device, &config, analyzer.clone(), |s| {
                s as f32 / u16::MAX as f32 * 2.0 - 1.0
            }),
            SampleFormat::I32 => build::<i32>(&device, &config, analyzer.clone(), |s| {
                s as f32 / i32::MAX as f32
            }),
            other => return Err(Error::Audio(format!("unsupported sample format {other}"))),
        }?;
        stream.play().map_err(|e| Error::Audio(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            analyzer,
            name,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
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

fn build<T: SizedSample + Copy + 'static>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    analyzer: Arc<Mutex<Analyzer>>,
    to_f32: fn(T) -> f32,
) -> Result<cpal::Stream> {
    let channels = config.channels.max(1) as usize;
    let mut mono = Vec::new();
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
            },
            |_| {},
            None,
        )
        .map_err(|e| Error::Audio(e.to_string()))
}
