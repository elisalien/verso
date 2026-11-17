//! Undo and redo of edits made in the interface.
//!
//! Each edit stores the composition as it was before. Continuous edits (a
//! fader drag, typing a name) on the same control within a second merge into
//! one step. Live actions (launching clips, going to cues, and anything sent
//! by MIDI, OSC or scripts) are not recorded: undo never stops what is
//! playing.

use std::time::{Duration, Instant};

use verso_core::{Command, Composition, EffectEdit, IndexChanges};

const LIMIT: usize = 100;
const MERGE: Duration = Duration::from_secs(1);

pub struct Step {
    before: Composition,
    label: String,
    /// How the edit renumbered layers and columns, from before to after.
    changes: Option<IndexChanges>,
    /// Input of the Advanced Output slices that showed a layer the edit
    /// removed: (slice id, source code). They fall back to the composition
    /// when the layer goes, and get the layer back when the edit is undone.
    slice_sources: Vec<(u64, String)>,
}

impl Step {
    /// Records what a structural edit moves outside the composition.
    pub fn with_structure(
        mut self,
        changes: Option<IndexChanges>,
        slice_sources: Vec<(u64, String)>,
    ) -> Self {
        self.changes = changes;
        self.slice_sources = slice_sources;
        self
    }
}

/// A composition taken from the history, and how to bring the state kept
/// outside it (players, render targets, selection) in line with it.
pub struct Restored {
    pub comp: Composition,
    /// Renumbering from the current composition to `comp`.
    pub changes: Option<IndexChanges>,
    /// Slice inputs to put back: (slice id, source code).
    pub slice_sources: Vec<(u64, String)>,
}

/// The renumbering that undoes `changes`, for a composition that now has
/// `layers` layers and `columns` columns.
fn inverse(changes: &IndexChanges, layers: usize, columns: usize) -> IndexChanges {
    let invert = |map: &[Option<usize>], count: usize| {
        let mut out = vec![None; count];
        for (old, new) in map.iter().enumerate() {
            if let Some(new) = new
                && *new < count
            {
                out[*new] = Some(old);
            }
        }
        out
    };
    IndexChanges {
        layers: invert(&changes.layers, layers),
        columns: invert(&changes.columns, columns),
    }
}

#[derive(Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Controls edited since the last one-off edit and when, to merge
    /// drags. Several, because one drag can move many controls at once (a
    /// slider for every MilkDrop clip): each gets one step per drag, where
    /// remembering only the last one made a step per clip per frame and
    /// pushed everything else out of the history.
    last: Vec<(String, Instant)>,
    /// Control of the edit being applied, until it is known to succeed.
    pending: Option<(String, Instant)>,
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
        Command::SetLayerBlendParam { layer, name, .. } => format!("blend {layer} {name}"),
        Command::SetGroupBlendParam { group, name, .. } => format!("group blend {group} {name}"),
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
        let merges = key.as_ref().is_some_and(|k| {
            self.last
                .iter()
                .any(|(last, at)| k == last && now - *at < MERGE)
        });
        // The key is remembered by `push`, once the command succeeded: a
        // refused command must not swallow the step of the next one.
        self.pending = key.map(|k| (k, now));
        (!merges).then(|| Step {
            before: before.clone(),
            label: label(command),
            changes: None,
            slice_sources: Vec::new(),
        })
    }

    /// Call when the command given to [`Self::snapshot`] succeeded without a
    /// step to push (it continued the previous edit).
    pub fn merged(&mut self) {
        self.remember();
    }

    /// Notes the control of the edit that just succeeded. A one-off edit
    /// ends every drag.
    fn remember(&mut self) {
        match self.pending.take() {
            Some((key, now)) => {
                self.last.retain(|(k, at)| *k != key && now - *at < MERGE);
                self.last.push((key, now));
            }
            None => self.last.clear(),
        }
    }

    /// Call when the command given to [`Self::snapshot`] was refused.
    pub fn refused(&mut self) {
        self.pending = None;
    }

    /// Steps that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    pub fn push(&mut self, step: Step) {
        self.remember();
        self.undo.push(step);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Restores the previous state; `current` goes to the redo stack.
    pub fn undo(&mut self, current: &Composition) -> Option<Restored> {
        let step = self.undo.pop()?;
        self.last.clear();
        let changes = step
            .changes
            .as_ref()
            .map(|c| inverse(c, current.layers.len(), current.columns.len()));
        self.redo.push(Step {
            before: current.clone(),
            label: step.label,
            changes: step.changes,
            slice_sources: step.slice_sources.clone(),
        });
        Some(Restored {
            comp: step.before,
            changes,
            slice_sources: step.slice_sources,
        })
    }

    pub fn redo(&mut self, current: &Composition) -> Option<Restored> {
        let step = self.redo.pop()?;
        self.last.clear();
        let changes = step.changes.clone();
        self.undo.push(Step {
            before: current.clone(),
            label: step.label,
            changes: step.changes,
            slice_sources: step.slice_sources,
        });
        Some(Restored {
            comp: step.before,
            changes,
            // Doing the edit again moves the slices by itself.
            slice_sources: Vec::new(),
        })
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
        match step {
            Some(step) => history.push(step),
            None => history.merged(),
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
        let undone = history.undo(&comp).unwrap().comp;
        assert_eq!(undone, original, "one step for the whole drag");
        assert!(history.undo(&undone).is_none());
        assert_eq!(history.redo(&undone).unwrap().comp, edited);
    }

    #[test]
    fn a_drag_over_several_controls_makes_one_step_each() {
        let mut comp = Composition::demo_with_layers(3);
        let mut history = History::default();
        // Twenty frames of one slider moving the opacity of two layers.
        for frame in 0..20 {
            for layer in 0..2 {
                let command = Command::SetLayerOpacity {
                    layer,
                    opacity: frame as f32 / 20.0,
                };
                let step = history.snapshot(&comp, &command);
                comp.apply(command).unwrap();
                match step {
                    Some(step) => history.push(step),
                    None => history.merged(),
                }
            }
        }
        assert_eq!(history.undo_len(), 2);
    }

    #[test]
    fn a_refused_command_does_not_swallow_the_next_step() {
        let mut comp = Composition::demo();
        let mut history = History::default();
        let refused = Command::SetLayerOpacity {
            layer: 99,
            opacity: 0.5,
        };
        // Same control as the refused command would have had.
        assert!(history.snapshot(&comp, &refused).is_some());
        assert!(comp.apply(refused).is_err());
        history.refused();
        opacity(&mut comp, &mut history, 0.4);
        assert_eq!(history.undo_len(), 1);
    }

    #[test]
    fn undoing_a_removal_renumbers_back() {
        let mut comp = Composition::demo_with_layers(3);
        let mut history = History::default();
        let command = Command::RemoveLayer(1);
        let changes = comp.index_changes(&command);
        let step = history.snapshot(&comp, &command).unwrap();
        comp.apply(command).unwrap();
        history.push(step.with_structure(changes, vec![(7, "layer".into())]));

        let undone = history.undo(&comp).unwrap();
        assert_eq!(undone.comp.layers.len(), 3);
        // Current layers 0 and 1 are layers 0 and 2 of the restored one.
        assert_eq!(undone.changes.unwrap().layers, vec![Some(0), Some(2)]);
        assert_eq!(undone.slice_sources.len(), 1);

        let redone = history.redo(&undone.comp).unwrap();
        assert_eq!(redone.comp.layers.len(), 2);
        assert_eq!(redone.changes.unwrap().layers, vec![Some(0), None, Some(1)]);
        assert!(redone.slice_sources.is_empty());
    }

    #[test]
    fn labels_read_as_words() {
        assert_eq!(label(&Command::RemoveLayer(0)), "Remove Layer");
    }
}
