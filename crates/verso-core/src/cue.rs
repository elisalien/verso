//! Cue list: recorded states of the composition, replayed in order with a
//! fade, for shows that run the same way every night.
//!
//! A cue stores, for each layer, the column playing and the opacity, plus
//! the master and the crossfader. Going to a cue launches its clips with the
//! layer transition set to the cue's fade (a crossfade between clips, as the
//! renderer does for any clip change; from an empty layer it cuts) and moves
//! the opacities, master and crossfader to the cue's values over the fade.
//! Layers added after the cue was recorded are left alone.
//!
//! Cues refer to layers and columns by index; deleting a layer or a column
//! updates them, as for modulators and MIDI mappings.

use serde::{Deserialize, Serialize};

use crate::effect::shifted;
use crate::{Composition, Error, Result};

/// What makes the next cue start by itself.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub enum Follow {
    /// Waits for Go.
    #[default]
    Manual,
    /// Goes to the next cue this many seconds after this one started.
    After(f32),
    /// Goes to the next cue after this many beats at the composition tempo.
    Beats(f32),
}

impl Follow {
    /// Seconds before the next cue starts, if it starts by itself.
    pub fn seconds(self, bpm: f32) -> Option<f32> {
        match self {
            Follow::Manual => None,
            Follow::After(s) => Some(s.max(0.0)),
            Follow::Beats(b) => Some(b.max(0.0) * 60.0 / bpm.max(1.0)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LayerCue {
    /// Column playing, `None` for an empty layer.
    pub active: Option<usize>,
    pub opacity: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CueState {
    /// One entry per layer, bottom layer first.
    pub layers: Vec<LayerCue>,
    pub master: f32,
    pub crossfader: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    pub name: String,
    pub state: CueState,
    /// Seconds to reach the cue.
    pub fade: f32,
    pub follow: Follow,
}

/// The cues of a composition, saved with it.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct CueList {
    pub cues: Vec<Cue>,
    /// Last cue gone to.
    #[serde(default)]
    pub current: Option<usize>,
}

impl CueList {
    pub fn is_empty(&self) -> bool {
        self.cues.is_empty()
    }
}

/// A cue in progress: levels moving and the wait before the next cue.
#[derive(Debug, Clone, PartialEq)]
pub struct CueRun {
    pub cue: usize,
    from: CueState,
    to: CueState,
    /// Seconds since Go.
    pub elapsed: f32,
    pub fade: f32,
}

impl CueRun {
    /// Progress of the fade, 0 to 1.
    pub fn progress(&self) -> f32 {
        if self.fade <= 0.0 {
            1.0
        } else {
            (self.elapsed / self.fade).clamp(0.0, 1.0)
        }
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Composition {
    /// The current state, as a cue would record it.
    pub fn cue_state(&self) -> CueState {
        CueState {
            layers: self
                .layers
                .iter()
                .map(|l| LayerCue {
                    active: l.active,
                    opacity: l.opacity,
                })
                .collect(),
            master: self.master.opacity,
            crossfader: self.master.crossfader,
        }
    }

    pub(crate) fn cue(&self, index: usize) -> Result<&Cue> {
        self.cues.cues.get(index).ok_or(Error::NoCue(index))
    }

    pub(crate) fn cue_mut(&mut self, index: usize) -> Result<&mut Cue> {
        self.cues.cues.get_mut(index).ok_or(Error::NoCue(index))
    }

    /// Records the current state as a new cue after the current one (or at
    /// the end), and makes it current.
    pub(crate) fn record_cue(&mut self, name: Option<String>) {
        let at = self
            .cues
            .current
            .map_or(self.cues.cues.len(), |c| (c + 1).min(self.cues.cues.len()));
        let name = name.unwrap_or_else(|| format!("Cue {}", self.cues.cues.len() + 1));
        self.cues.cues.insert(
            at,
            Cue {
                name,
                state: self.cue_state(),
                fade: 2.0,
                follow: Follow::Manual,
            },
        );
        self.cues.current = Some(at);
        self.cue_run = None;
    }

    pub(crate) fn remove_cue(&mut self, index: usize) -> Result<()> {
        self.cue(index)?;
        self.cues.cues.remove(index);
        self.cues.current = self.cues.current.and_then(|c| shifted(c, index));
        self.cue_run = None;
        Ok(())
    }

    pub(crate) fn move_cue(&mut self, from: usize, to: usize) -> Result<()> {
        self.cue(from)?;
        let to = to.min(self.cues.cues.len() - 1);
        let cue = self.cues.cues.remove(from);
        self.cues.cues.insert(to, cue);
        if self.cues.current == Some(from) {
            self.cues.current = Some(to);
        }
        self.cue_run = None;
        Ok(())
    }

    /// Starts cue `index`: launches its clips with its fade as transition
    /// and starts moving the levels.
    pub(crate) fn go_cue(&mut self, index: usize) -> Result<()> {
        let cue = self.cue(index)?.clone();
        let from = self.cue_state();
        for (layer, target) in self.layers.iter_mut().zip(&cue.state.layers) {
            let column = target
                .active
                .filter(|&c| layer.clips.get(c).is_some_and(Option::is_some));
            if layer.active != column {
                match column {
                    Some(c) => layer.launch(c),
                    None => layer.active = None,
                }
                layer.cue_transition = Some(cue.fade.max(0.0));
            }
        }
        self.cues.current = Some(index);
        self.cue_run = Some(CueRun {
            cue: index,
            from,
            to: cue.state,
            elapsed: 0.0,
            fade: cue.fade.max(0.0),
        });
        self.advance_cues(0.0);
        Ok(())
    }

    /// Goes to the cue after the current one; the first cue when none is
    /// current. Does nothing after the last cue.
    pub(crate) fn go_next_cue(&mut self) -> Result<()> {
        let next = self.cues.current.map_or(0, |c| c + 1);
        if next < self.cues.cues.len() {
            self.go_cue(next)?;
        }
        Ok(())
    }

    pub(crate) fn go_previous_cue(&mut self) -> Result<()> {
        if let Some(previous) = self.cues.current.and_then(|c| c.checked_sub(1)) {
            self.go_cue(previous)?;
        }
        Ok(())
    }

    /// Moves the running cue on by `dt` seconds: fades the levels, then
    /// starts the next cue when the current one follows by itself. Call once
    /// per frame.
    pub fn advance_cues(&mut self, dt: f32) {
        let Some(run) = &mut self.cue_run else {
            return;
        };
        run.elapsed += dt.max(0.0);
        let t = run.progress();
        let (from, to) = (&run.from, &run.to);
        for (i, layer) in self.layers.iter_mut().enumerate() {
            if let (Some(a), Some(b)) = (from.layers.get(i), to.layers.get(i)) {
                layer.opacity = lerp(a.opacity, b.opacity, t);
            }
        }
        self.master.opacity = lerp(from.master, to.master, t);
        self.master.crossfader = lerp(from.crossfader, to.crossfader, t);

        let (cue, elapsed) = (run.cue, run.elapsed);
        let wait = self
            .cues
            .cues
            .get(cue)
            .and_then(|c| c.follow.seconds(self.bpm));
        match wait {
            Some(wait) if elapsed >= wait && cue + 1 < self.cues.cues.len() => {
                let _ = self.go_cue(cue + 1);
            }
            // Done: nothing moves until the next Go.
            None if t >= 1.0 => self.cue_run = None,
            Some(_) if t >= 1.0 && cue + 1 >= self.cues.cues.len() => self.cue_run = None,
            _ => {}
        }
    }

    /// Stops the fade in progress, leaving the levels where they are. A
    /// fader moved by hand takes over from the cue.
    pub(crate) fn stop_cue_fade(&mut self) {
        self.cue_run = None;
    }

    /// Cue references after `removed` layer is deleted.
    pub(crate) fn cues_without_layer(&mut self, removed: usize) {
        for cue in &mut self.cues.cues {
            if removed < cue.state.layers.len() {
                cue.state.layers.remove(removed);
            }
        }
        self.cue_run = None;
    }

    /// Cue references after `removed` column is deleted: layers that played
    /// it are empty in the cue.
    pub(crate) fn cues_without_column(&mut self, removed: usize) {
        for layer in self.cues.cues.iter_mut().flat_map(|c| &mut c.state.layers) {
            layer.active = layer.active.and_then(|c| shifted(c, removed));
        }
        self.cue_run = None;
    }
}

#[cfg(test)]
mod tests {
    use crate::{Clip, Command, Composition, Source};

    use super::Follow;

    /// Two layers, three columns, every slot filled.
    fn comp() -> Composition {
        let mut comp = Composition::new("t", 2, 3);
        for layer in 0..2 {
            for column in 0..3 {
                comp.apply(Command::SetClip {
                    layer,
                    column,
                    clip: Some(Clip::new("c", Source::milkdrop("a/b", 0))),
                })
                .unwrap();
            }
        }
        comp
    }

    #[test]
    fn records_and_replays_a_state() {
        let mut comp = comp();
        comp.apply(Command::TriggerColumn(1)).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 0.25,
        })
        .unwrap();
        comp.apply(Command::RecordCue { name: None }).unwrap();
        assert_eq!(comp.cues.cues[0].name, "Cue 1");

        comp.apply(Command::TriggerColumn(2)).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 1.0,
        })
        .unwrap();
        comp.apply(Command::SetCueFade {
            index: 0,
            seconds: 0.0,
        })
        .unwrap();
        comp.apply(Command::GoCue(0)).unwrap();
        assert!(comp.layers.iter().all(|l| l.active == Some(1)));
        assert_eq!(comp.layers[0].opacity, 0.25);
        assert_eq!(comp.cues.current, Some(0));
        assert!(comp.cue_run.is_none());
    }

    #[test]
    fn fades_levels_and_sets_the_clip_transition() {
        let mut comp = comp();
        comp.apply(Command::SetMasterOpacity(1.0)).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 1,
            opacity: 0.0,
        })
        .unwrap();
        comp.apply(Command::RecordCue { name: None }).unwrap();
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 1,
            opacity: 1.0,
        })
        .unwrap();
        comp.apply(Command::SetMasterOpacity(0.0)).unwrap();
        comp.apply(Command::RecordCue {
            name: Some("Bright".into()),
        })
        .unwrap();

        comp.apply(Command::SetCueFade {
            index: 1,
            seconds: 4.0,
        })
        .unwrap();
        comp.apply(Command::GoCue(0)).unwrap();
        comp.advance_cues(10.0);
        comp.apply(Command::GoCue(1)).unwrap();
        // The clips launched with the fade as transition.
        assert!(comp.layers.iter().all(|l| l.active == Some(0)));
        assert_eq!(comp.layers[1].transition_time(), 4.0);
        comp.advance_cues(2.0);
        assert!((comp.layers[1].opacity - 0.5).abs() < 1e-5);
        assert!((comp.master.opacity - 0.5).abs() < 1e-5);
        comp.advance_cues(2.0);
        assert_eq!(comp.layers[1].opacity, 1.0);
        assert!(comp.cue_run.is_none());
        // A launch by hand uses the layer's own transition again.
        comp.apply(Command::TriggerClip {
            layer: 1,
            column: 2,
        })
        .unwrap();
        assert_eq!(comp.layers[1].transition_time(), 0.0);
    }

    #[test]
    fn moving_a_fader_stops_the_fade() {
        let mut comp = comp();
        comp.apply(Command::RecordCue { name: None }).unwrap();
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 0.0,
        })
        .unwrap();
        comp.apply(Command::SetCueFade {
            index: 0,
            seconds: 4.0,
        })
        .unwrap();
        comp.apply(Command::GoCue(0)).unwrap();
        comp.advance_cues(1.0);
        comp.apply(Command::SetLayerOpacity {
            layer: 0,
            opacity: 0.1,
        })
        .unwrap();
        comp.advance_cues(1.0);
        assert_eq!(comp.layers[0].opacity, 0.1);
    }

    #[test]
    fn follow_goes_on_by_itself() {
        let mut comp = comp();
        for column in 0..3 {
            comp.apply(Command::TriggerColumn(column)).unwrap();
            comp.apply(Command::RecordCue { name: None }).unwrap();
        }
        comp.apply(Command::SetCueFollow {
            index: 0,
            follow: Follow::After(1.0),
        })
        .unwrap();
        // 120 BPM: two beats are one second.
        comp.apply(Command::SetCueFollow {
            index: 1,
            follow: Follow::Beats(2.0),
        })
        .unwrap();
        comp.apply(Command::GoCue(0)).unwrap();
        comp.advance_cues(0.5);
        assert_eq!(comp.cues.current, Some(0));
        comp.advance_cues(0.6);
        assert_eq!(comp.cues.current, Some(1));
        comp.advance_cues(1.0);
        assert_eq!(comp.cues.current, Some(2));
        assert!(comp.layers.iter().all(|l| l.active == Some(2)));
        // The last cue waits.
        comp.advance_cues(10.0);
        assert_eq!(comp.cues.current, Some(2));
    }

    #[test]
    fn next_previous_and_edits() {
        let mut comp = comp();
        comp.apply(Command::GoNextCue).unwrap();
        assert_eq!(comp.cues.current, None);
        for column in 0..3 {
            comp.apply(Command::TriggerColumn(column)).unwrap();
            comp.apply(Command::RecordCue { name: None }).unwrap();
        }
        comp.apply(Command::GoCue(0)).unwrap();
        comp.apply(Command::GoNextCue).unwrap();
        assert_eq!(comp.cues.current, Some(1));
        comp.apply(Command::GoPreviousCue).unwrap();
        assert_eq!(comp.cues.current, Some(0));

        // Recording inserts after the current cue.
        comp.apply(Command::RecordCue {
            name: Some("Inserted".into()),
        })
        .unwrap();
        assert_eq!(comp.cues.cues[1].name, "Inserted");
        comp.apply(Command::MoveCue { from: 1, to: 3 }).unwrap();
        assert_eq!(comp.cues.cues[3].name, "Inserted");
        assert_eq!(comp.cues.current, Some(3));
        comp.apply(Command::RemoveCue(0)).unwrap();
        assert_eq!(comp.cues.current, Some(2));
        comp.apply(Command::RenameCue {
            index: 0,
            name: "Intro".into(),
        })
        .unwrap();
        assert_eq!(comp.cues.cues[0].name, "Intro");
        assert!(comp.apply(Command::GoCue(9)).is_err());

        // Updating takes the current state again.
        comp.apply(Command::TriggerColumn(0)).unwrap();
        comp.apply(Command::UpdateCue(0)).unwrap();
        assert!(
            comp.cues.cues[0]
                .state
                .layers
                .iter()
                .all(|l| l.active == Some(0))
        );
    }

    #[test]
    fn deleting_layers_and_columns_updates_cues() {
        let mut comp = comp();
        comp.apply(Command::TriggerClip {
            layer: 0,
            column: 1,
        })
        .unwrap();
        comp.apply(Command::TriggerClip {
            layer: 1,
            column: 2,
        })
        .unwrap();
        comp.apply(Command::RecordCue { name: None }).unwrap();
        comp.apply(Command::RemoveColumn(1)).unwrap();
        let layers = &comp.cues.cues[0].state.layers;
        assert_eq!((layers[0].active, layers[1].active), (None, Some(1)));
        comp.apply(Command::RemoveLayer(0)).unwrap();
        assert_eq!(comp.cues.cues[0].state.layers.len(), 1);
        assert_eq!(comp.cues.cues[0].state.layers[0].active, Some(1));
    }

    #[test]
    fn old_files_open_without_cues() {
        let text = Composition::new("old", 1, 1).to_ron().unwrap();
        let mut comp = Composition::from_ron(&text).unwrap();
        assert!(comp.cues.cues.is_empty());
        comp.apply(Command::RecordCue { name: None }).unwrap();
        let reopened = Composition::from_ron(&comp.to_ron().unwrap()).unwrap();
        assert_eq!(reopened.cues, comp.cues);
    }
}
