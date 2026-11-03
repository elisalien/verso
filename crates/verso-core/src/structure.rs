//! Removing, moving and copying layers, columns and clips.
//!
//! Layers and columns are addressed by index. When they move, MIDI mappings
//! and modulators that point at them are renumbered so they keep following
//! the same layer or column; ones pointing at a removed item are dropped.
//! Script addresses are text and are not rewritten.

use crate::control::{Action, Param};
use crate::{Composition, EffectTarget, Error, Result};

/// New position of each old index, `None` when the item is gone.
type IndexMap<'a> = &'a dyn Fn(usize) -> Option<usize>;

/// Index map for removing `removed`.
fn removal(removed: usize) -> impl Fn(usize) -> Option<usize> {
    move |i| match i.cmp(&removed) {
        std::cmp::Ordering::Less => Some(i),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(i - 1),
    }
}

/// Index map for moving `from` so it ends at `to`.
fn movement(from: usize, to: usize) -> impl Fn(usize) -> Option<usize> {
    move |i| {
        Some(if i == from {
            to
        } else if from < to && i > from && i <= to {
            i - 1
        } else if to < from && i >= to && i < from {
            i + 1
        } else {
            i
        })
    }
}

fn map_target(target: EffectTarget, layer: IndexMap, column: IndexMap) -> Option<EffectTarget> {
    Some(match target {
        EffectTarget::Composition | EffectTarget::Group(_) => target,
        EffectTarget::Layer(l) => EffectTarget::Layer(layer(l)?),
        EffectTarget::Clip {
            layer: l,
            column: c,
        } => EffectTarget::Clip {
            layer: layer(l)?,
            column: column(c)?,
        },
    })
}

fn map_param(param: &Param, layer: IndexMap, column: IndexMap) -> Option<Param> {
    Some(match param {
        Param::LayerOpacity(l) => Param::LayerOpacity(layer(*l)?),
        Param::LayerBypass(l) => Param::LayerBypass(layer(*l)?),
        Param::LayerSolo(l) => Param::LayerSolo(layer(*l)?),
        Param::LayerTransition(l) => Param::LayerTransition(layer(*l)?),
        Param::Effect {
            target,
            index,
            name,
        } => Param::Effect {
            target: map_target(*target, layer, column)?,
            index: *index,
            name: name.clone(),
        },
        Param::EffectBypass { target, index } => Param::EffectBypass {
            target: map_target(*target, layer, column)?,
            index: *index,
        },
        other => other.clone(),
    })
}

fn map_action(action: &Action, layer: IndexMap, column: IndexMap) -> Option<Action> {
    Some(match action {
        Action::Set(param) => Action::Set(map_param(param, layer, column)?),
        Action::LaunchClip {
            layer: l,
            column: c,
        } => Action::LaunchClip {
            layer: layer(*l)?,
            column: column(*c)?,
        },
        Action::LaunchColumn(c) => Action::LaunchColumn(column(*c)?),
        Action::EjectLayer(l) => Action::EjectLayer(layer(*l)?),
        Action::ClearAll => Action::ClearAll,
    })
}

/// Where each layer and column goes when a command changes the structure:
/// `layers[old]` is the new index of the old layer `old`, `None` if removed.
/// New items (a duplicate) have no old index and are not listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexChanges {
    pub layers: Vec<Option<usize>>,
    pub columns: Vec<Option<usize>>,
}

impl IndexChanges {
    /// Moves per-layer state kept outside the composition (render targets,
    /// players, selections) to follow the layers. Removed layers' state is
    /// dropped; slots without state get `T::default()`.
    pub fn apply_to_layers<T: Default>(&self, items: &mut Vec<T>) {
        let new_len = self
            .layers
            .iter()
            .flatten()
            .map(|i| i + 1)
            .max()
            .unwrap_or(0);
        let mut out: Vec<T> = std::iter::repeat_with(T::default).take(new_len).collect();
        for (old, item) in std::mem::take(items).into_iter().enumerate() {
            if let Some(Some(new)) = self.layers.get(old) {
                out[*new] = item;
            }
        }
        *items = out;
    }
}

impl Composition {
    /// How `command` would renumber layers and columns, or `None` when it
    /// does not move or remove any. Call before applying it.
    pub fn index_changes(&self, command: &crate::Command) -> Option<IndexChanges> {
        use crate::Command;
        let (layers, columns) = (self.layers.len(), self.columns.len());
        let keep = |n: usize| (0..n).map(Some).collect::<Vec<_>>();
        let map = |n: usize, f: &dyn Fn(usize) -> Option<usize>| (0..n).map(f).collect();
        Some(match *command {
            Command::RemoveLayer(i) if i < layers => IndexChanges {
                layers: map(layers, &removal(i)),
                columns: keep(columns),
            },
            Command::MoveLayer { from, to } if from < layers => IndexChanges {
                layers: map(layers, &movement(from, to.min(layers - 1))),
                columns: keep(columns),
            },
            Command::DuplicateLayer(i) if i < layers => IndexChanges {
                layers: map(layers, &|l| Some(if l > i { l + 1 } else { l })),
                columns: keep(columns),
            },
            Command::RemoveColumn(i) if i < columns => IndexChanges {
                layers: keep(layers),
                columns: map(columns, &removal(i)),
            },
            Command::MoveColumn { from, to } if from < columns => IndexChanges {
                layers: keep(layers),
                columns: map(columns, &movement(from, to.min(columns - 1))),
            },
            _ => return None,
        })
    }

    /// Renumbers controls after layers or columns moved.
    fn remap(&mut self, layer: IndexMap, column: IndexMap) {
        self.midi = std::mem::take(&mut self.midi)
            .into_iter()
            .filter_map(|mut m| {
                m.action = map_action(&m.action, layer, column)?;
                Some(m)
            })
            .collect();
        self.modulators = std::mem::take(&mut self.modulators)
            .into_iter()
            .filter_map(|mut m| {
                m.param = map_param(&m.param, layer, column)?;
                Some(m)
            })
            .collect();
    }

    pub(crate) fn remove_layer(&mut self, index: usize) -> Result<()> {
        if index >= self.layers.len() {
            return Err(Error::NoLayer(index));
        }
        self.layers.remove(index);
        self.remap(&removal(index), &Some);
        Ok(())
    }

    pub(crate) fn move_layer(&mut self, from: usize, to: usize) -> Result<()> {
        let count = self.layers.len();
        if from >= count {
            return Err(Error::NoLayer(from));
        }
        let to = to.min(count - 1);
        let layer = self.layers.remove(from);
        self.layers.insert(to, layer);
        self.remap(&movement(from, to), &Some);
        Ok(())
    }

    /// Copies a layer with its clips and effects above the original.
    pub(crate) fn duplicate_layer(&mut self, index: usize) -> Result<()> {
        let mut copy = self.layers.get(index).ok_or(Error::NoLayer(index))?.clone();
        copy.name = format!("{} copy", copy.name);
        copy.active = None;
        copy.launches = vec![0; copy.clips.len()];
        self.layers.insert(index + 1, copy);
        self.remap(&|i| Some(if i > index { i + 1 } else { i }), &Some);
        Ok(())
    }

    pub(crate) fn remove_column(&mut self, index: usize) -> Result<()> {
        self.check_column(index)?;
        self.columns.remove(index);
        for layer in &mut self.layers {
            if index < layer.clips.len() {
                layer.clips.remove(index);
            }
            if index < layer.launches.len() {
                layer.launches.remove(index);
            }
            layer.active = match layer.active {
                Some(c) if c == index => None,
                Some(c) if c > index => Some(c - 1),
                other => other,
            };
        }
        self.remap(&Some, &removal(index));
        Ok(())
    }

    pub(crate) fn move_column(&mut self, from: usize, to: usize) -> Result<()> {
        self.check_column(from)?;
        let to = to.min(self.columns.len() - 1);
        let column = self.columns.remove(from);
        self.columns.insert(to, column);
        let map = movement(from, to);
        for layer in &mut self.layers {
            let clip = layer.clips.remove(from);
            layer.clips.insert(to, clip);
            let count = layer.launches.remove(from);
            layer.launches.insert(to, count);
            layer.active = layer.active.and_then(&map);
        }
        self.remap(&Some, &map);
        Ok(())
    }

    /// Moves or copies the clip of one cell to another. Moving onto a full
    /// cell swaps the two clips; copying replaces the destination.
    pub(crate) fn move_clip(
        &mut self,
        from: (usize, usize),
        to: (usize, usize),
        copy: bool,
    ) -> Result<()> {
        for (l, c) in [from, to] {
            self.check_column(c)?;
            self.layers.get(l).ok_or(Error::NoLayer(l))?;
        }
        if from == to {
            return Ok(());
        }
        let source = self.layers[from.0].clips[from.1].clone();
        if source.is_none() {
            return Err(Error::NoClip);
        }
        let previous = std::mem::replace(&mut self.layers[to.0].clips[to.1], source);
        if !copy {
            self.layers[from.0].clips[from.1] = previous;
        }
        // A playing cell that became empty stops.
        for (l, c) in [from, to] {
            let layer = &mut self.layers[l];
            if layer.active == Some(c) && layer.clips[c].is_none() {
                layer.active = None;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::control::{Action, MidiKey, MidiMapping, Param};
    use crate::modulation::{AudioBand, ModSource, Modulator};
    use crate::{Command, Composition};

    fn mapping(action: Action) -> MidiMapping {
        MidiMapping {
            key: MidiKey::Cc {
                channel: 0,
                controller: 1,
            },
            action,
        }
    }

    #[test]
    fn mappings_follow_moved_layers() {
        let mut comp = Composition::demo();
        comp.midi.push(mapping(Action::EjectLayer(0)));
        comp.modulators.push(Modulator {
            param: Param::LayerOpacity(2),
            source: ModSource::Audio {
                band: AudioBand::Bass,
            },
            depth: 1.0,
            bypass: false,
        });
        let bottom = comp.layers[0].name.clone();
        comp.apply(Command::MoveLayer { from: 0, to: 2 }).unwrap();
        assert_eq!(comp.layers[2].name, bottom);
        assert_eq!(comp.midi[0].action, Action::EjectLayer(2));
        assert_eq!(comp.modulators[0].param, Param::LayerOpacity(1));

        comp.apply(Command::RemoveLayer(2)).unwrap();
        assert!(comp.midi.is_empty(), "the mapped layer is gone");
        assert_eq!(comp.modulators[0].param, Param::LayerOpacity(1));
    }

    #[test]
    fn columns_move_and_remove_with_their_clips() {
        let mut comp = Composition::demo();
        comp.midi.push(mapping(Action::LaunchColumn(4)));
        comp.apply(Command::TriggerColumn(1)).unwrap();
        let clip = comp.layers[0].clips[1].clone();
        comp.apply(Command::MoveColumn { from: 1, to: 3 }).unwrap();
        assert_eq!(comp.layers[0].clips[3], clip);
        assert_eq!(comp.layers[0].active, Some(3));
        assert_eq!(comp.layers[0].launch_count(3), 1);
        assert_eq!(comp.midi[0].action, Action::LaunchColumn(4));

        comp.apply(Command::RemoveColumn(0)).unwrap();
        assert_eq!(comp.columns.len(), 5);
        assert!(comp.layers.iter().all(|l| l.clips.len() == 5));
        assert_eq!(comp.layers[0].active, Some(2));
        assert_eq!(comp.midi[0].action, Action::LaunchColumn(3));
    }

    #[test]
    fn clips_move_swap_and_copy() {
        let mut comp = Composition::demo();
        let a = comp.layers[0].clips[0].clone();
        let b = comp.layers[1].clips[2].clone();
        comp.apply(Command::MoveClip {
            from: (0, 0),
            to: (1, 2),
            copy: false,
        })
        .unwrap();
        assert_eq!(comp.layers[1].clips[2], a);
        assert_eq!(comp.layers[0].clips[0], b);
        comp.apply(Command::MoveClip {
            from: (1, 2),
            to: (2, 5),
            copy: true,
        })
        .unwrap();
        assert_eq!(comp.layers[2].clips[5], a);
        assert_eq!(comp.layers[1].clips[2], a);
    }

    #[test]
    fn duplicated_layer_sits_above() {
        let mut comp = Composition::demo();
        comp.midi.push(mapping(Action::EjectLayer(1)));
        comp.apply(Command::DuplicateLayer(0)).unwrap();
        assert_eq!(comp.layers.len(), 4);
        assert_eq!(comp.layers[1].clips, comp.layers[0].clips);
        assert_eq!(comp.midi[0].action, Action::EjectLayer(2));
    }
}
