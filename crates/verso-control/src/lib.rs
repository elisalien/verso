//! Control inputs for Verso: OSC over UDP, MIDI, audio analysis and tempo.
//!
//! Inputs run on their own threads and hand events to the app through
//! channels or shared state; nothing here touches the composition directly.

mod analysis;
mod audio;
#[cfg(feature = "link")]
pub mod link;
mod midi;
mod osc;
mod tempo;
pub mod timecode;

pub use analysis::{
    AnalysisSettings, Analyzer, Levels, SPECTRUM_BANDS, SPECTRUM_HIGH_HZ, SPECTRUM_LOW_HZ,
    WAVEFORM_LEN,
};
pub use audio::{
    AudioInput, LOOPBACK_PREFIX, TestTone, input_devices, loopback_devices, loopback_output,
    output_devices,
};
pub use midi::{MidiEvent, MidiInputs, midi_input_ports};
pub use osc::{OscAccess, OscEvent, OscServer};
pub use tempo::TapTempo;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("OSC: {0}")]
    Osc(String),
    #[error("MIDI: {0}")]
    Midi(String),
    #[error("audio: {0}")]
    Audio(String),
}

pub type Result<T> = std::result::Result<T, Error>;
