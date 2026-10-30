use crate::control::{MidiKey, MidiMapping};
use crate::effect::edit_chain;
use crate::modulation::Modulator;
use crate::{BlendMode, Clip, Composition, EffectEdit, EffectTarget, Error, FaderSide, Result};

/// A single change to a composition.
#[derive(Debug, Clone, PartialEq)]
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
    /// New group holding `layers`.
    AddGroup {
        layers: Vec<usize>,
    },
    /// Deletes a group; its layers stay, ungrouped.
    RemoveGroup(usize),
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
    /// Binds a MIDI control, replacing any mapping of the same control.
    MapMidi(MidiMapping),
    UnmapMidi(MidiKey),
    SetMasterOpacity(f32),
    SetCrossfader(f32),
    SetBpm(f32),
    AddLayer,
    AddColumn,
}

impl Composition {
    pub fn apply(&mut self, command: Command) -> Result<()> {
        match command {
            Command::TriggerClip { layer, column } => self.trigger_clip(layer, column)?,
            Command::TriggerColumn(column) => self.trigger_column(column)?,
            Command::EjectLayer(layer) => self.layer_mut(layer)?.active = None,
            Command::ClearAll => self.layers.iter_mut().for_each(|l| l.active = None),
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
                self.layer_mut(layer)?.opacity = opacity.clamp(0.0, 1.0)
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
            Command::SetLayerGroup { layer, group } => {
                if let Some(g) = group
                    && g >= self.groups.len()
                {
                    return Err(Error::NoGroup(g));
                }
                self.layer_mut(layer)?.group = group
            }
            Command::AddGroup { layers } => self.add_group(&layers)?,
            Command::RemoveGroup(group) => self.remove_group(group)?,
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
            Command::MapMidi(mapping) => {
                self.midi.retain(|m| m.key != mapping.key);
                self.midi.push(mapping);
            }
            Command::UnmapMidi(key) => self.midi.retain(|m| m.key != key),
            Command::SetCrossfader(value) => self.master.crossfader = value.clamp(0.0, 1.0),
            Command::SetBpm(bpm) => self.bpm = bpm.clamp(20.0, 500.0),
            Command::SetMasterOpacity(opacity) => self.master.opacity = opacity.clamp(0.0, 1.0),
            Command::AddLayer => self.add_layer(),
            Command::AddColumn => self.add_column(),
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
    fn ron_round_trip() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(1)).unwrap();
        let text = comp.to_ron().unwrap();
        assert_eq!(Composition::from_ron(&text).unwrap(), comp);
    }
}
