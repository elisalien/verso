use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Value of one effect parameter.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ParamValue {
    Float(f32),
    /// Straight (non-premultiplied) RGBA.
    Color([f32; 4]),
    Bool(bool),
    Point([f32; 2]),
}

/// An effect instance in a chain. Its definition (shader, parameter list) lives
/// in the renderer's effect library and is looked up by `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    /// `builtin:<name>` for bundled effects, else the shader file path.
    pub id: String,
    /// Display name, kept so a missing effect still shows what it was.
    pub name: String,
    #[serde(default)]
    pub bypass: bool,
    /// Parameters changed from their defaults.
    #[serde(default)]
    pub params: BTreeMap<String, ParamValue>,
}

impl Effect {
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            bypass: false,
            params: BTreeMap::new(),
        }
    }
}

/// Where an effect chain lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EffectTarget {
    Composition,
    Group(usize),
    Layer(usize),
    Clip { layer: usize, column: usize },
}

/// Change to one effect chain.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectEdit {
    Add(Effect),
    Remove(usize),
    /// Moves the effect at `from` so it ends at index `to`.
    Move {
        from: usize,
        to: usize,
    },
    SetBypass {
        index: usize,
        bypass: bool,
    },
    SetParam {
        index: usize,
        name: String,
        value: ParamValue,
    },
}

pub(crate) fn edit_chain(chain: &mut Vec<Effect>, edit: EffectEdit) -> crate::Result<()> {
    let missing = |index: usize| crate::Error::NoEffect(index);
    match edit {
        EffectEdit::Add(effect) => chain.push(effect),
        EffectEdit::Remove(index) => {
            if index >= chain.len() {
                return Err(missing(index));
            }
            chain.remove(index);
        }
        EffectEdit::Move { from, to } => {
            if from >= chain.len() {
                return Err(missing(from));
            }
            let effect = chain.remove(from);
            chain.insert(to.min(chain.len()), effect);
        }
        EffectEdit::SetBypass { index, bypass } => {
            chain.get_mut(index).ok_or(missing(index))?.bypass = bypass
        }
        EffectEdit::SetParam { index, name, value } => {
            chain
                .get_mut(index)
                .ok_or(missing(index))?
                .params
                .insert(name, value);
        }
    }
    Ok(())
}
