//! Undo and redo of edits made in the interface.
//!
//! Each edit stores the composition as it was before. Continuous edits (a
//! fader drag, typing a name) on the same control within a second merge into
//! one step. Live actions (launching clips, MIDI, OSC, scripts) are not
//! recorded: undo never stops what is playing.

use std::time::{Duration, Instant};

use verso_core::{Command, Composition, EffectEdit};

const LIMIT: usize = 100;
const MERGE: Duration = Duration::from_secs(1);

pub struct Step {
    before: Composition,
    label: String,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Control of the last edit and when it happened, to merge drags.
    last: Option<(String, Instant)>,
}

/// Whether `command` is a live action rather than an edit.
pub fn is_live(command: &Command) -> bool {
    matches!(
        command,
        Command::TriggerClip { .. }
            | Command::TriggerColumn(_)
            | Command::EjectLayer(_)
            | Command::ClearAll
    )
}

/// Identifies the control a continuous edit changes; `None` for one-off
/// edits, which never merge.
fn merge_key(command: &Command) -> Option<String> {
    Some(match command {
        Command::SetLayerOpacity { layer, .. } => format!("opacity {layer}"),
        Command::SetLayerTransition { layer, .. } => format!("transition {layer}"),
        Command::RenameLayer { layer, .. } => format!("layer name {layer}"),
        Command::RenameColumn { column, .. } => format!("column name {column}"),
        Command::RenameGroup { group, .. } => format!("group name {group}"),
        Command::SetGroupOpacity { group, .. } => format!("group opacity {group}"),
        Command::SetClip {
            layer,
            column,
            clip: Some(_),
        } => format!("clip {layer} {column}"),
        Command::EditEffects {
            target,
            edit: EffectEdit::SetParam { index, name, .. },
        } => format!("effect {target:?} {index} {name}"),
        Command::SetModulator { index, .. } => format!("modulator {index}"),
        Command::SetScript { index, .. } => format!("script {index}"),
        Command::SetMasterOpacity(_) => "master".into(),
        Command::SetCrossfader(_) => "crossfader".into(),
        Command::SetBpm(_) => "bpm".into(),
        _ => return None,
    })
}

/// Readable name of a command for the Edit menu: `MoveLayer` → "Move Layer".
pub fn label(command: &Command) -> String {
    let debug = format!("{command:?}");
    let name = debug
        .split(|c: char| !c.is_alphanumeric())
        .next()
        .unwrap_or_default();
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if i > 0 && c.is_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

impl History {
    /// Copy of `before`, the composition before `command` applies, to
    /// [`Self::push`] once the command succeeded. `None` when the command
    /// continues the previous edit (same control within a second).
    pub fn snapshot(&mut self, before: &Composition, command: &Command) -> Option<Step> {
        let now = Instant::now();
        let key = merge_key(command);
        let merges = matches!(
            (&key, &self.last),
            (Some(k), Some((last, at))) if k == last && now - *at < MERGE
        );
        self.last = key.map(|k| (k, now));
        (!merges).then(|| Step {
            before: before.clone(),
            label: label(command),
        })
    }

    pub fn push(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Restores the previous state; `current` goes to the redo stack.
    pub fn undo(&mut self, current: &Composition) -> Option<Composition> {
        let step = self.undo.pop()?;
        self.last = None;
        self.redo.push(Step {
            before: current.clone(),
            label: step.label.clone(),
        });
        Some(step.before)
    }

    pub fn redo(&mut self, current: &Composition) -> Option<Composition> {
        let step = self.redo.pop()?;
        self.last = None;
        self.undo.push(Step {
            before: current.clone(),
            label: step.label.clone(),
        });
        Some(step.before)
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opacity(comp: &mut Composition, history: &mut History, value: f32) {
        let command = Command::SetLayerOpacity {
            layer: 0,
            opacity: value,
        };
        let step = history.snapshot(comp, &command);
        comp.apply(command).unwrap();
        if let Some(step) = step {
            history.push(step);
        }
    }

    #[test]
    fn drags_merge_and_undo_redo_round_trip() {
        let mut comp = Composition::demo();
        let original = comp.clone();
        let mut history = History::default();
        for v in [0.9, 0.8, 0.7] {
            opacity(&mut comp, &mut history, v);
        }
        assert_eq!(history.undo_label(), Some("Set Layer Opacity"));
        let edited = comp.clone();
        let undone = history.undo(&comp).unwrap();
        assert_eq!(undone, original, "one step for the whole drag");
        assert!(history.undo(&undone).is_none());
        assert_eq!(history.redo(&undone).unwrap(), edited);
    }

    #[test]
    fn live_actions_are_not_edits() {
        assert!(is_live(&Command::TriggerColumn(0)));
        assert!(!is_live(&Command::RemoveLayer(0)));
        assert_eq!(label(&Command::RemoveLayer(0)), "Remove Layer");
    }
}
