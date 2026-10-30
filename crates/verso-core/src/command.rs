use crate::{BlendMode, Clip, Composition, FaderSide, Result};

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
    fn ron_round_trip() {
        let mut comp = Composition::demo();
        comp.apply(Command::TriggerColumn(1)).unwrap();
        let text = comp.to_ron().unwrap();
        assert_eq!(Composition::from_ron(&text).unwrap(), comp);
    }
}
