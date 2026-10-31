//! GPU-free composition model for Verso.
//!
//! Every change goes through [`Command`] so that UI, MIDI, OSC and scripts all
//! mutate the composition the same way.

mod command;
mod composition;
pub mod control;
mod effect;
mod groups;
pub mod modulation;
mod source;

pub use command::{Command, CommandKind};
pub use composition::{
    BlendMode, Clip, Column, Composition, FaderSide, Group, Layer, Master, Script,
};
pub use effect::{Effect, EffectEdit, EffectTarget, ParamValue};
pub use source::{Generator, Source};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("layer {0} does not exist")]
    NoLayer(usize),
    #[error("column {0} does not exist")]
    NoColumn(usize),
    #[error("group {0} does not exist")]
    NoGroup(usize),
    #[error("clip slot is empty")]
    NoClip,
    #[error("effect {0} does not exist")]
    NoEffect(usize),
    #[error("modulator {0} does not exist")]
    NoModulator(usize),
    #[error("script {0} does not exist")]
    NoScript(usize),
    #[error("only adjacent layers can be grouped")]
    NotAdjacent,
    #[error("a composition keeps at least one layer")]
    LastLayer,
    #[error("a composition keeps at least one column")]
    LastColumn,
    #[error("could not parse composition: {0}")]
    Parse(#[from] ron::error::SpannedError),
    #[error("could not serialize composition: {0}")]
    Serialize(#[from] ron::Error),
}

pub type Result<T> = std::result::Result<T, Error>;
