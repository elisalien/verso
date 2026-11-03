//! Control inputs for Verso: OSC over UDP, MIDI, audio analysis and tempo.
//!
//! Inputs run on their own threads and hand events to the app through
//! channels or shared state; nothing here touches the composition directly.

mod analysis;
mod audio;
mod midi;
mod osc;
mod tempo;

pub use analysis::{Analyzer, Levels};
pub use audio::{AudioInput, input_devices};
pub use midi::{MidiEvent, MidiInputs};
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
