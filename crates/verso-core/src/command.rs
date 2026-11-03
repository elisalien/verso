use serde::{Deserialize, Serialize};

use crate::control::{MidiKey, MidiMapping};
use crate::effect::edit_chain;
use crate::modulation::Modulator;
use crate::{
    BlendMode, Clip, Composition, EffectEdit, EffectTarget, Error, FaderSide, Result, Script,
};

/// A single change to a composition. Serializable so that remote clients
/// (the MCP server) can send any command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Command {
    TriggerClip {
        layer: usize,
        column: usize,
    },
    TriggerColumn(usize),
    EjectLayer(usize),
    ClearAll,
    SetClip {
        layer: usize,
        column: usize,
        clip: Option<Clip>,
    },
    SetLayerOpacity {
        layer: usize,
        opacity: f32,
    },
    SetLayerBlend {
        layer: usize,
        blend: BlendMode,
    },
    SetLayerBypass {
        layer: usize,
        bypass: bool,
    },
    SetLayerSolo {
        layer: usize,
        solo: bool,
    },
    SetLayerFader {
        layer: usize,
        side: FaderSide,
    },
    RenameLayer {
        layer: usize,
        name: String,
    },
    RenameColumn {
        column: usize,
        name: String,
    },
    SetLayerTransition {
        layer: usize,
        seconds: f32,
    },
    /// Puts a layer in a group, or takes it out with `None`.
    SetLayerGroup {
        layer: usize,
        group: Option<usize>,
    },
    /// New group holding `layers`, which must form one adjacent block once
    /// whole groups are included; groups inside become nested.
    AddGroup {
        layers: Vec<usize>,
    },
    /// Deletes a group; its layers and groups move up to its parent.
    RemoveGroup(usize),
    SetGroupSolo {
        group: usize,
        solo: bool,
    },
    SetGroupCollapsed {
        group: usize,
        collapsed: bool,
    },
    /// Launches a column on the layers of one group only.
    LaunchGroupColumn {
        group: usize,
        column: usize,
    },
    RenameGroup {
        group: usize,
        name: String,
    },
    SetGroupOpacity {
        group: usize,
        opacity: f32,
    },
    SetGroupBlend {
        group: usize,
        blend: BlendMode,
    },
    SetGroupBypass {
        group: usize,
        bypass: bool,
    },
    EditEffects {
        target: EffectTarget,
        edit: EffectEdit,
    },
    AddModulator(Modulator),
    RemoveModulator(usize),
    SetModulator {
        index: usize,
        modulator: Modulator,
    },
    /// Adds a script; its name is made unique.
    AddScript(Script),
    SetScript {
        index: usize,
        script: Script,
    },
    RemoveScript(usize),
    /// Binds a MIDI control, replacing any mapping of the same control.
    MapMidi(MidiMapping),
    UnmapMidi(MidiKey),
    SetMasterOpacity(f32),
    SetCrossfader(f32),
    SetBpm(f32),
    ResetLaunchCount {
        layer: usize,
        column: usize,
    },
    AddLayer,
    AddColumn,
    /// Deletes a layer. Modulators and MIDI mappings of that layer go away;
    /// those of the layers above move down with them.
    RemoveLayer(usize),
    /// Deletes a column and the clips in it, on every layer.
    RemoveColumn(usize),
    Rename(String),
    SetResolution {
        width: u32,
        height: u32,
    },
    /// Records the current state as a cue after the current one, with a
    /// default name when `None`.
    RecordCue {
        name: Option<String>,
    },
    /// Records the current state into an existing cue.
    UpdateCue(usize),
    RemoveCue(usize),
    MoveCue {
        from: usize,
        to: usize,
    },
    RenameCue {
        index: usize,
        name: String,
    },
    SetCueFade {
        index: usize,
        seconds: f32,
    },
    SetCueFollow {
        index: usize,
        follow: crate::cue::Follow,
    },
    /// Starts a cue: launches its clips and fades to its levels.
    GoCue(usize),
    /// The cue after the current one (the first when none is current).
    GoNextCue,
    GoPreviousCue,
    /// Adds a timecode cue, keeping timecode cues sorted by time.
    AddTimecodeCue(crate::control::Cue),
    RemoveTimecodeCue(usize),
    SetTimecodeCue {
        index: usize,
        cue: crate::control::Cue,
    },
    /// Moves a layer so it ends at index `to`.
    MoveLayer {
        from: usize,
        to: usize,
    },
    MoveColumn {
        from: usize,
        to: usize,
    },
    DuplicateLayer(usize),
    /// Moves (swapping with a full destination) or copies a clip between
    /// cells, each given as `(layer, column)`.
    MoveClip {
        from: (usize, usize),
        to: (usize, usize),
        copy: bool,
    },
}

/// What a command means for the document: whether it needs saving and
/// whether it can be undone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandKind {
    /// Playback: launching and ejecting clips. Does not change the document.
    Play,
    /// A value edit: faders, parameters, names. Changes the document but
    /// arrives many times a second, so it is not undone step by step.
    Tweak,
    /// A structure edit: clips, layers, columns, groups, effect chains,
    /// mappings. Changes the document and can be undone.
    Edit,
}

impl Command {
    pub fn kind(&self) -> CommandKind {
        use Command::*;
        match self {
            TriggerClip { .. }
            | TriggerColumn(_)
            | EjectLayer(_)
            | ClearAll
            | LaunchGroupColumn { .. }
            | GoCue(_)
            | GoNextCue
            | GoPreviousCue => CommandKind::Play,
            SetLayerOpacity { .. }
            | SetLayerBypass { .. }
            | SetLayerSolo { .. }
            | SetLayerFader { .. }
            | SetLayerTransition { .. }
            | SetGroupSolo { .. }
            | SetGroupCollapsed { .. }
            | SetGroupOpacity { .. }
            | SetGroupBypass { .. }
            | RenameLayer { .. }
            | RenameColumn { .. }
            | RenameGroup { .. }
            | Rename(_)
            | SetModulator { .. }
            | SetScript { .. }
            | SetMasterOpacity(_)
            | SetCrossfader(_)
            | SetBpm(_)
            | RenameCue { .. }
            | SetCueFade { .. }
            | SetCueFollow { .. } => CommandKind::Tweak,
            EditEffects { edit, .. } => match edit {
                EffectEdit::SetParam { .. } | EffectEdit::SetBypass { .. } => CommandKind::Tweak,
                EffectEdit::Add(_) | EffectEdit::Remove(_) | EffectEdit::Move { .. } => {
                    CommandKind::Edit
                }
            },
            SetClip { .. }
            | SetLayerBlend { .. }
            | SetGroupBlend { .. }
            | SetLayerGroup { .. }
            | AddGroup { .. }
            | RemoveGroup(_)
            | AddModulator(_)
            | RemoveModulator(_)
            | AddScript(_)
            | RemoveScript(_)
            | MapMidi(_)
            | UnmapMidi(_)
            | ResetLaunchCount { .. }
            | AddLayer
            | AddColumn
            | RemoveLayer(_)
            | RemoveColumn(_)
            | SetResolution { .. }
            | RecordCue { .. }
            | UpdateCue(_)
            | RemoveCue(_)
            | MoveCue { .. }
            | AddTimecodeCue(_)
            | RemoveTimecodeCue(_)
            | SetTimecodeCue { .. }
            | MoveLayer { .. }
            | MoveColumn { .. }
            | DuplicateLayer(_)
            | MoveClip { .. } => CommandKind::Edit,
        }
    }
}

impl Composition {
    pub fn apply(&mut self, command: Command) -> Result<()> {
        if !command.is_finite() {
            return Err(Error::NotFinite);
        }
        match command {
            Command::TriggerClip { layer, column } => self.trigger_clip(layer, column)?,
            Command::TriggerColumn(column) => self.trigger_column(column)?,
            Command::EjectLayer(layer) => {
                let layer = self.layer_mut(layer)?;
                layer.active = None;
                layer.cue_transition = None;
            }
            Command::ClearAll => self.layers.iter_mut().for_each(|l| {
                l.active = None;
                l.cue_transition = None;
            }),
            Command::SetClip {
                layer,
                column,
                clip,
            } => {
                let layer = self.layer_mut(layer)?;
                let cell = layer
                    .clips
                    .get_mut(column)
                    .ok_or(crate::Error::NoColumn(column))?;
                if clip.is_none() && layer.active == Some(column) {
                    layer.active = None;
                }
                *cell = clip;
            }
            Command::SetLayerOpacity { layer, opacity } => {
                self.layer_mut(layer)?.opacity = opacity.clamp(0.0, 1.0);
                self.stop_cue_fade();
            }
            Command::SetLayerBlend { layer, blend } => self.layer_mut(layer)?.blend = blend,
            Command::SetLayerBypass { layer, bypass } => self.layer_mut(layer)?.bypass = bypass,
            Command::SetLayerSolo { layer, solo } => self.layer_mut(layer)?.solo = solo,
            Command::SetLayerFader { layer, side } => self.layer_mut(layer)?.fader = side,
            Command::RenameLayer { layer, name } => self.layer_mut(layer)?.name = name,
            Command::RenameColumn { column, name } => {
                self.columns
                    .get_mut(column)
                    .ok_or(crate::Error::NoColumn(column))?
                    .name = name
            }
            Command::SetLayerTransition { layer, seconds } => {
                self.layer_mut(layer)?.transition = seconds.clamp(0.0, 30.0)
            }
            Command::SetLayerGroup { layer, group } => self.set_layer_group(layer, group)?,
            Command::AddGroup { layers } => {
                self.group_layers_together(&layers)?;
            }
            Command::RemoveGroup(group) => self.ungroup(group)?,
            Command::SetGroupSolo { group, solo } => self.group_mut(group)?.solo = solo,
            Command::SetGroupCollapsed { group, collapsed } => {
                self.group_mut(group)?.collapsed = collapsed
            }
            Command::LaunchGroupColumn { group, column } => {
                self.launch_group_column(group, column)?
            }
            Command::RenameGroup { group, name } => self.group_mut(group)?.name = name,
            Command::SetGroupOpacity { group, opacity } => {
                self.group_mut(group)?.opacity = opacity.clamp(0.0, 1.0)
            }
            Command::SetGroupBlend { group, blend } => self.group_mut(group)?.blend = blend,
            Command::SetGroupBypass { group, bypass } => self.group_mut(group)?.bypass = bypass,
            Command::EditEffects { target, edit } => edit_chain(self.effects_mut(target)?, edit)?,
            Command::AddModulator(modulator) => self.modulators.push(modulator),
            Command::RemoveModulator(index) => {
                if index >= self.modulators.len() {
                    return Err(Error::NoModulator(index));
                }
                self.modulators.remove(index);
            }
            Command::SetModulator { index, modulator } => {
                *self
                    .modulators
                    .get_mut(index)
                    .ok_or(Error::NoModulator(index))? = modulator
            }
            Command::AddScript(mut script) => {
                let base = script.name.clone();
                let mut n = 1;
                while self.scripts.iter().any(|s| s.name == script.name) {
                    n += 1;
                    script.name = format!("{base} {n}");
                }
                self.scripts.push(script);
            }
            Command::SetScript { index, script } => {
                *self.scripts.get_mut(index).ok_or(Error::NoScript(index))? = script
            }
            Command::RemoveScript(index) => {
                if index >= self.scripts.len() {
                    return Err(Error::NoScript(index));
                }
                self.scripts.remove(index);
            }
            Command::MapMidi(mapping) => {
                self.midi.retain(|m| m.key != mapping.key);
                self.midi.push(mapping);
            }
            Command::UnmapMidi(key) => self.midi.retain(|m| m.key != key),
            Command::SetCrossfader(value) => {
                self.master.crossfader = value.clamp(0.0, 1.0);
                self.stop_cue_fade();
            }
            Command::SetBpm(bpm) => self.bpm = bpm.clamp(20.0, 500.0),
            Command::SetMasterOpacity(opacity) => {
                self.master.opacity = opacity.clamp(0.0, 1.0);
                self.stop_cue_fade();
            }
            Command::ResetLaunchCount { layer, column } => {
                if let Some(count) = self.layer_mut(layer)?.launches.get_mut(column) {
                    *count = 0;
                }
            }
            Command::AddLayer => self.add_layer(),
            Command::AddColumn => self.add_column(),
            Command::RemoveLayer(layer) => self.remove_layer(layer)?,
            Command::RemoveColumn(column) => self.remove_column(column)?,
            Command::Rename(name) => self.name = name,
            Command::SetResolution { width, height } => {
                self.width = width.clamp(16, 8192);
                self.height = height.clamp(16, 8192);
            }
            Command::RecordCue { name } => self.record_cue(name),
            Command::UpdateCue(index) => {
                let state = self.cue_state();
                self.cue_mut(index)?.state = state;
            }
            Command::RemoveCue(index) => self.remove_cue(index)?,
            Command::MoveCue { from, to } => self.move_cue(from, to)?,
            Command::RenameCue { index, name } => self.cue_mut(index)?.name = name,
            Command::SetCueFade { index, seconds } => {
                self.cue_mut(index)?.fade = seconds.clamp(0.0, 600.0)
            }
            Command::SetCueFollow { index, follow } => self.cue_mut(index)?.follow = follow,
            Command::GoCue(index) => self.go_cue(index)?,
            Command::GoNextCue => self.go_next_cue()?,
            Command::GoPreviousCue => self.go_previous_cue()?,
            Command::AddTimecodeCue(cue) => {
                self.timecode_cues.push(cue);
                self.sort_timecode_cues();
            }
            Command::RemoveTimecodeCue(index) => {
                if index >= self.timecode_cues.len() {
                    return Err(Error::NoCue(index));
                }
                self.timecode_cues.remove(index);
            }
            Command::SetTimecodeCue { index, cue } => {
                *self
                    .timecode_cues
                    .get_mut(index)
                    .ok_or(Error::NoCue(index))? = cue;
                self.sort_timecode_cues();
            }
            Command::MoveLayer { from, to } => self.move_layer(from, to)?,
            Command::MoveColumn { from, to } => self.move_column(from, to)?,
            Command::DuplicateLayer(layer) => self.duplicate_layer(layer)?,
            Command::MoveClip { from, to, copy } => self.move_clip(from, to, copy)?,
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Error;

    #[test]
    fn trigger_column_starts_one_clip_per_layer() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(2)).unwrap();
        assert!(comp.layers.iter().all(|l| l.active == Some(2)));
    }

    #[test]
    fn trigger_clip_replaces_playing_clip_on_same_layer_only() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::TriggerClip {
            layer: 1,
            column: 3,
        })
        .unwrap();
        assert_eq!(comp.layers[0].active, Some(0));
        assert_eq!(comp.layers[1].active, Some(3));
    }

    #[test]
    fn empty_cell_ejects_layer() {
        let mut comp = Composition::new("t", 2, 2);
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();
        assert_eq!(comp.layers[0].active, None);
    }

    #[test]
    fn removing_playing_clip_ejects_it() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();
        comp.apply(Command::SetClip {
            layer: 0,
            column: 1,
            clip: None,
        })
        .unwrap();
        assert_eq!(comp.layers[0].active, None);
    }

    #[test]
    fn out_of_range_indices_are_errors() {
        let mut comp = Composition::new("t", 2, 2);
        assert!(matches!(
            comp.apply(Command::TriggerColumn(5)),
            Err(Error::NoColumn(5))
        ));
        assert!(matches!(
            comp.apply(Command::EjectLayer(9)),
            Err(Error::NoLayer(9))
        ));
    }

    #[test]
    fn add_column_extends_every_layer() {
        let mut comp = Composition::new("t", 3, 2);
        comp.apply(Command::AddColumn).unwrap();
        assert_eq!(comp.columns.len(), 3);
        assert!(comp.layers.iter().all(|l| l.clips.len() == 3));
    }

    #[test]
    fn opacity_is_clamped() {
        let mut comp = Composition::new("t", 1, 1);
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 3.0,
        })
        .unwrap();
        assert_eq!(comp.layers[0].opacity, 1.0);
    }

    #[test]
    fn solo_mutes_other_layers() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::SetLayerSolo {
            layer: 1,
            solo: true,
        })
        .unwrap();
        assert_eq!(comp.layer_gain(0), 0.0);
        assert_eq!(comp.layer_gain(1), comp.layers[1].opacity);
        assert_eq!(comp.layer_gain(2), 0.0);
    }

    #[test]
    fn crossfader_levels() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 1.0,
        })
        .unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 1,
            opacity: 1.0,
        })
        .unwrap();
        comp.apply(Command::SetLayerFader {
            layer: 0,
            side: FaderSide::A,
        })
        .unwrap();
        comp.apply(Command::SetLayerFader {
            layer: 1,
            side: FaderSide::B,
        })
        .unwrap();

        // Center: both sides at full level.
        assert_eq!((comp.layer_gain(0), comp.layer_gain(1)), (1.0, 1.0));
        comp.apply(Command::SetCrossfader(0.0)).unwrap();
        assert_eq!((comp.layer_gain(0), comp.layer_gain(1)), (1.0, 0.0));
        comp.apply(Command::SetCrossfader(0.75)).unwrap();
        assert_eq!((comp.layer_gain(0), comp.layer_gain(1)), (0.5, 1.0));
        // Unassigned layers ignore the crossfader.
        assert_eq!(comp.layer_gain(2), comp.layers[2].opacity);
    }

    #[test]
    fn idle_layer_has_no_gain() {
        let comp = Composition::demo();
        assert_eq!(comp.layer_gain(0), 0.0);
        assert_eq!(comp.layer_gain(99), 0.0);
    }

    #[test]
    fn files_without_new_fields_still_load() {
        let mut comp = Composition::new("t", 1, 1);
        let text = comp
            .to_ron()
            .unwrap()
            .replace("solo: false,", "")
            .replace("fader: None,", "")
            .replace("crossfader: 0.5,", "");
        comp.master.crossfader = 0.5;
        assert_eq!(Composition::from_ron(&text).unwrap(), comp);
    }

    #[test]
    fn effect_chain_edits() {
        use crate::{Effect, ParamValue};
        let mut comp = Composition::demo();
        let target = EffectTarget::Layer(1);
        let edit = |edit| Command::EditEffects { target, edit };
        comp.apply(edit(EffectEdit::Add(Effect::new(
            "builtin:invert",
            "Invert",
        ))))
        .unwrap();
        comp.apply(edit(EffectEdit::Add(Effect::new("builtin:blur", "Blur"))))
            .unwrap();
        comp.apply(edit(EffectEdit::Move { from: 1, to: 0 }))
            .unwrap();
        comp.apply(edit(EffectEdit::SetParam {
            index: 0,
            name: "radius".into(),
            value: ParamValue::Float(0.5),
        }))
        .unwrap();
        comp.apply(edit(EffectEdit::SetBypass {
            index: 1,
            bypass: true,
        }))
        .unwrap();
        let chain = comp.effects(target).unwrap();
        assert_eq!(chain[0].id, "builtin:blur");
        assert_eq!(chain[0].params["radius"], ParamValue::Float(0.5));
        assert!(chain[1].bypass);
        comp.apply(edit(EffectEdit::Remove(0))).unwrap();
        assert_eq!(comp.effects(target).unwrap().len(), 1);
        assert!(matches!(
            comp.apply(edit(EffectEdit::Remove(7))),
            Err(Error::NoEffect(7))
        ));
    }

    #[test]
    fn clip_effects_need_a_clip() {
        let mut comp = Composition::new("t", 1, 1);
        let result = comp.apply(Command::EditEffects {
            target: EffectTarget::Clip {
                layer: 0,
                column: 0,
            },
            edit: EffectEdit::Add(crate::Effect::new("builtin:invert", "Invert")),
        });
        assert!(matches!(result, Err(Error::NoClip)));
    }

    #[test]
    fn groups_add_and_remove_reindex_layers() {
        let mut comp = Composition::new("t", 4, 1);
        comp.apply(Command::AddGroup { layers: vec![0, 1] })
            .unwrap();
        comp.apply(Command::AddGroup { layers: vec![3] }).unwrap();
        assert_eq!(comp.group_layers(0).collect::<Vec<_>>(), [0, 1]);
        comp.apply(Command::RemoveGroup(0)).unwrap();
        assert_eq!(comp.groups.len(), 1);
        assert_eq!(comp.layers[0].group, None);
        assert_eq!(comp.layers[3].group, Some(0));
        assert!(matches!(
            comp.apply(Command::SetLayerGroup {
                layer: 0,
                group: Some(4)
            }),
            Err(Error::NoGroup(4))
        ));
        comp.apply(Command::SetGroupBypass {
            group: 0,
            bypass: true,
        })
        .unwrap();
        assert_eq!(comp.group_gain(0), 0.0);
    }

    #[test]
    fn ejected_layer_keeps_level_for_fade_out() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::EjectLayer(0)).unwrap();
        assert_eq!(comp.layer_gain(0), 0.0);
        assert_eq!(comp.layer_level(0), comp.layers[0].opacity);
    }

    #[test]
    fn midi_mapping_replaces_same_control() {
        use crate::control::{Action, Param};
        let mut comp = Composition::new("t", 2, 2);
        let key = MidiKey::Cc {
            channel: 0,
            controller: 1,
        };
        comp.apply(Command::MapMidi(MidiMapping {
            key,
            action: Action::ClearAll,
        }))
        .unwrap();
        comp.apply(Command::MapMidi(MidiMapping {
            key,
            action: Action::Set(Param::Master),
        }))
        .unwrap();
        assert_eq!(comp.midi.len(), 1);
        assert_eq!(comp.midi[0].action, Action::Set(Param::Master));
        comp.apply(Command::UnmapMidi(key)).unwrap();
        assert!(comp.midi.is_empty());
    }

    #[test]
    fn script_names_stay_unique() {
        let mut comp = Composition::new("t", 1, 1);
        let script = Script {
            name: "Chase".into(),
            source: String::new(),
            enabled: true,
        };
        comp.apply(Command::AddScript(script.clone())).unwrap();
        comp.apply(Command::AddScript(script)).unwrap();
        assert_eq!(comp.scripts[1].name, "Chase 2");
        comp.apply(Command::RemoveScript(0)).unwrap();
        assert!(matches!(
            comp.apply(Command::RemoveScript(4)),
            Err(Error::NoScript(4))
        ));
    }

    #[test]
    fn launches_are_counted_per_slot() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 2,
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 2,
        })
        .unwrap();
        comp.apply(Command::TriggerColumn(2)).unwrap();
        // Relaunching the playing clip is not counted.
        assert_eq!(comp.layers[0].launch_count(2), 1);
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();
        comp.apply(Command::TriggerColumn(2)).unwrap();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 2,
        })
        .unwrap();
        assert_eq!(comp.layers[0].launch_count(2), 3);
        assert_eq!(comp.layers[0].launch_count(1), 2);
        // Same column, other layer: its own count.
        assert_eq!(comp.layers[1].launch_count(2), 1);

        // Empty slots eject without counting.
        comp.apply(Command::SetClip {
            layer: 0,
            column: 4,
            clip: None,
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 4,
        })
        .unwrap();
        assert_eq!(comp.layers[0].launch_count(4), 0);

        // Replacing the clip keeps the slot's count; reset clears it.
        let clip = comp.layers[1].clips[0].clone();
        comp.apply(Command::SetClip {
            layer: 0,
            column: 2,
            clip,
        })
        .unwrap();
        assert_eq!(comp.layers[0].launch_count(2), 3);
        comp.apply(Command::ResetLaunchCount {
            layer: 0,
            column: 2,
        })
        .unwrap();
        assert_eq!(comp.layers[0].launch_count(2), 0);
    }

    #[test]
    fn remove_layer_shifts_references_and_keeps_one() {
        use crate::control::{Action, Param};
        use crate::modulation::{AudioBand, ModSource, Modulator};
        let mut comp = Composition::new("t", 3, 2);
        let modulator = |layer| Modulator {
            param: Param::LayerOpacity(layer),
            source: ModSource::Audio {
                band: AudioBand::Bass,
            },
            depth: 1.0,
            bypass: false,
        };
        comp.apply(Command::AddModulator(modulator(0))).unwrap();
        comp.apply(Command::AddModulator(modulator(1))).unwrap();
        comp.apply(Command::AddModulator(modulator(2))).unwrap();
        comp.apply(Command::MapMidi(MidiMapping {
            key: MidiKey::Note {
                channel: 0,
                note: 1,
            },
            action: Action::LaunchClip {
                layer: 2,
                column: 1,
            },
        }))
        .unwrap();
        comp.apply(Command::MapMidi(MidiMapping {
            key: MidiKey::Note {
                channel: 0,
                note: 2,
            },
            action: Action::EjectLayer(1),
        }))
        .unwrap();
        comp.apply(Command::AddGroup { layers: vec![1, 2] })
            .unwrap();

        comp.apply(Command::RemoveLayer(1)).unwrap();
        assert_eq!(comp.layers.len(), 2);
        assert_eq!(comp.layers[1].name, "Layer 3");
        let params: Vec<_> = comp.modulators.iter().map(|m| m.param.clone()).collect();
        assert_eq!(params, [Param::LayerOpacity(0), Param::LayerOpacity(1)]);
        assert_eq!(comp.midi.len(), 1);
        assert_eq!(
            comp.midi[0].action,
            Action::LaunchClip {
                layer: 1,
                column: 1
            }
        );
        // The group keeps its remaining layer.
        assert_eq!(comp.layers[1].group, Some(0));

        // Removing the last member dissolves the group.
        comp.apply(Command::RemoveLayer(1)).unwrap();
        assert!(comp.groups.is_empty());
        assert!(matches!(
            comp.apply(Command::RemoveLayer(0)),
            Err(Error::LastLayer)
        ));
        assert!(matches!(
            comp.apply(Command::RemoveLayer(5)),
            Err(Error::NoLayer(5))
        ));
    }

    #[test]
    fn remove_column_drops_clips_and_shifts_active() {
        use crate::control::Action;
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(3)).unwrap();
        comp.apply(Command::MapMidi(MidiMapping {
            key: MidiKey::Note {
                channel: 0,
                note: 1,
            },
            action: Action::LaunchColumn(1),
        }))
        .unwrap();
        comp.apply(Command::MapMidi(MidiMapping {
            key: MidiKey::Note {
                channel: 0,
                note: 2,
            },
            action: Action::LaunchColumn(4),
        }))
        .unwrap();
        comp.apply(Command::RemoveColumn(1)).unwrap();
        assert_eq!(comp.columns.len(), 5);
        assert!(comp.layers.iter().all(|l| l.clips.len() == 5));
        assert!(comp.layers.iter().all(|l| l.launches.len() == 5));
        // The playing column moved down by one.
        assert!(comp.layers.iter().all(|l| l.active == Some(2)));
        assert_eq!(comp.midi.len(), 1);
        assert_eq!(comp.midi[0].action, Action::LaunchColumn(3));

        // Removing the playing column ejects the layers.
        comp.apply(Command::RemoveColumn(2)).unwrap();
        assert!(comp.layers.iter().all(|l| l.active.is_none()));
        let mut one = Composition::new("t", 1, 1);
        assert!(matches!(
            one.apply(Command::RemoveColumn(0)),
            Err(Error::LastColumn)
        ));
    }

    #[test]
    fn ungroup_shifts_group_references() {
        use crate::control::Param;
        use crate::modulation::{ModSource, Modulator, Waveform};
        let mut comp = Composition::new("t", 4, 1);
        comp.apply(Command::AddGroup { layers: vec![0] }).unwrap();
        comp.apply(Command::AddGroup { layers: vec![2] }).unwrap();
        for g in 0..2 {
            comp.apply(Command::AddModulator(Modulator {
                param: Param::GroupOpacity(g),
                source: ModSource::Lfo {
                    waveform: Waveform::Sine,
                    beats: 4.0,
                },
                depth: 1.0,
                bypass: false,
            }))
            .unwrap();
        }
        comp.apply(Command::RemoveGroup(0)).unwrap();
        let params: Vec<_> = comp.modulators.iter().map(|m| m.param.clone()).collect();
        assert_eq!(params, [Param::GroupOpacity(0)]);
    }

    #[test]
    fn command_kinds() {
        assert_eq!(Command::TriggerColumn(0).kind(), CommandKind::Play);
        assert_eq!(Command::SetBpm(1.0).kind(), CommandKind::Tweak);
        assert_eq!(Command::AddLayer.kind(), CommandKind::Edit);
        assert_eq!(
            Command::EditEffects {
                target: EffectTarget::Composition,
                edit: EffectEdit::Remove(0)
            }
            .kind(),
            CommandKind::Edit
        );
    }

    #[test]
    fn ron_round_trip() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(1)).unwrap();
        let text = comp.to_ron().unwrap();
        assert_eq!(Composition::from_ron(&text).unwrap(), comp);
    }
}
