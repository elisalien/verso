//! GPU-free composition model for Verso.
//!
//! Every change goes through [`Command`] so that UI, MIDI, OSC and scripts all
//! mutate the composition the same way.

mod command;
mod composition;
mod source;

pub use command::Command;
pub use composition::{BlendMode, Clip, Column, Composition, FaderSide, Layer, Master};
pub use source::{Generator, Source};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("layer {0} does not exist")]
    NoLayer(usize),
    #[error("column {0} does not exist")]
    NoColumn(usize),
    #[error("could not parse composition: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("could not serialize composition: {0}")]
    Serialize(#[from] ron::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
