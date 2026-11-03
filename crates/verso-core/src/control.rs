//! Remote control: the address space shared by OSC, MIDI and modulation.
//!
//! An [`Action`] is either a continuous [`Param`] or a trigger. Each has a
//! stable OSC address with 1-based indices, for example `/layer/2/opacity` or
//! `/layer/1/clip/3/launch`. Indices in Rust values stay 0-based.

use serde::{Deserialize, Serialize};

use crate::{Command, Composition, EffectEdit, EffectTarget, ParamValue};

/// A value that can be set or modulated.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Param {
    Master,
    Crossfader,
    Bpm,
    LayerOpacity(usize),
    LayerBypass(usize),
    LayerSolo(usize),
    LayerTransition(usize),
    GroupOpacity(usize),
    GroupBypass(usize),
    /// A float or bool parameter of an effect.
    Effect {
        target: EffectTarget,
        index: usize,
        name: String,
    },
    EffectBypass {
        target: EffectTarget,
        index: usize,
    },
}

/// Something a controller can do.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    Set(Param),
    LaunchClip {
        layer: usize,
        column: usize,
    },
    LaunchColumn(usize),
    LaunchGroupColumn {
        group: usize,
        column: usize,
    },
    EjectLayer(usize),
    ClearAll,
    /// Go: the next cue.
    CueGo,
    CuePrevious,
    /// A given cue.
    GoCue(usize),
}

/// Value range of a parameter. A toggle is on above the middle of its range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Range {
    pub min: f32,
    pub max: f32,
    pub toggle: bool,
}

impl Range {
    pub const UNIT: Range = Range {
        min: 0.0,
        max: 1.0,
        toggle: false,
    };
    pub const TOGGLE: Range = Range {
        min: 0.0,
        max: 1.0,
        toggle: true,
    };

    /// Maps 0..1 onto the range.
    pub fn from_unit(&self, unit: f32) -> f32 {
        self.min + unit.clamp(0.0, 1.0) * (self.max - self.min)
    }

    /// Maps a range value onto 0..1.
    pub fn to_unit(&self, value: f32) -> f32 {
        if self.max == self.min {
            return 0.0;
        }
        ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
    }
}

impl Param {
    /// Range of every parameter except effect parameters, which depend on the
    /// effect definition held by the renderer.
    pub fn builtin_range(&self) -> Option<Range> {
        Some(match self {
            Param::Master | Param::Crossfader | Param::LayerOpacity(_) | Param::GroupOpacity(_) => {
                Range::UNIT
            }
            Param::Bpm => Range {
                min: 20.0,
                max: 500.0,
                toggle: false,
            },
            Param::LayerTransition(_) => Range {
                min: 0.0,
                max: 10.0,
                toggle: false,
            },
            Param::LayerBypass(_)
            | Param::LayerSolo(_)
            | Param::GroupBypass(_)
            | Param::EffectBypass { .. } => Range::TOGGLE,
            Param::Effect { .. } => return None,
        })
    }

    /// Current value, with toggles as 0 or 1. Effect parameters still at their
    /// default return `None` since only the renderer knows the default.
    pub fn get(&self, comp: &Composition) -> Option<f32> {
        let flag = |b: bool| if b { 1.0 } else { 0.0 };
        Some(match self {
            Param::Master => comp.master.opacity,
            Param::Crossfader => comp.master.crossfader,
            Param::Bpm => comp.bpm,
            Param::LayerOpacity(l) => comp.layers.get(*l)?.opacity,
            Param::LayerBypass(l) => flag(comp.layers.get(*l)?.bypass),
            Param::LayerSolo(l) => flag(comp.layers.get(*l)?.solo),
            Param::LayerTransition(l) => comp.layers.get(*l)?.transition,
            Param::GroupOpacity(g) => comp.groups.get(*g)?.opacity,
            Param::GroupBypass(g) => flag(comp.groups.get(*g)?.bypass),
            Param::Effect {
                target,
                index,
                name,
            } => match comp.effects(*target)?.get(*index)?.params.get(name)? {
                ParamValue::Float(v) => *v,
                ParamValue::Bool(b) => flag(*b),
                _ => return None,
            },
            Param::EffectBypass { target, index } => {
                flag(comp.effects(*target)?.get(*index)?.bypass)
            }
        })
    }

    /// Command setting the parameter to `value`, in its own range.
    pub fn command(&self, value: f32, range: Range) -> Command {
        let on = value > (range.min + range.max) / 2.0;
        match self.clone() {
            Param::Master => Command::SetMasterOpacity(value),
            Param::Crossfader => Command::SetCrossfader(value),
            Param::Bpm => Command::SetBpm(value),
            Param::LayerOpacity(layer) => Command::SetLayerOpacity {
                layer,
                opacity: value,
            },
            Param::LayerBypass(layer) => Command::SetLayerBypass { layer, bypass: on },
            Param::LayerSolo(layer) => Command::SetLayerSolo { layer, solo: on },
            Param::LayerTransition(layer) => Command::SetLayerTransition {
                layer,
                seconds: value,
            },
            Param::GroupOpacity(group) => Command::SetGroupOpacity {
                group,
                opacity: value,
            },
            Param::GroupBypass(group) => Command::SetGroupBypass { group, bypass: on },
            Param::Effect {
                target,
                index,
                name,
            } => Command::EditEffects {
                target,
                edit: EffectEdit::SetParam {
                    index,
                    name,
                    value: if range.toggle {
                        ParamValue::Bool(on)
                    } else {
                        ParamValue::Float(value.clamp(range.min, range.max))
                    },
                },
            },
            Param::EffectBypass { target, index } => Command::EditEffects {
                target,
                edit: EffectEdit::SetBypass { index, bypass: on },
            },
        }
    }

    /// Writes `value` directly, without a command. Used to apply modulation to
    /// a copy of the composition for one frame.
    pub fn write(&self, comp: &mut Composition, value: f32, range: Range) {
        let _ = comp.apply(self.command(value, range));
    }
}

impl Param {
    /// The parameter after layer `removed` is deleted, or `None` if it was on
    /// that layer.
    pub fn without_layer(&self, removed: usize) -> Option<Param> {
        self.remap(
            |l| crate::effect::shifted(l, removed),
            Some,
            |t| t.without_layer(removed),
        )
    }

    pub fn without_column(&self, removed: usize) -> Option<Param> {
        self.remap(Some, Some, |t| t.without_column(removed))
    }

    pub fn without_group(&self, removed: usize) -> Option<Param> {
        self.remap(
            Some,
            |g| crate::effect::shifted(g, removed),
            |t| t.without_group(removed),
        )
    }

    fn remap(
        &self,
        layer: impl Fn(usize) -> Option<usize>,
        group: impl Fn(usize) -> Option<usize>,
        target: impl Fn(EffectTarget) -> Option<EffectTarget>,
    ) -> Option<Param> {
        Some(match self.clone() {
            Param::LayerOpacity(l) => Param::LayerOpacity(layer(l)?),
            Param::LayerBypass(l) => Param::LayerBypass(layer(l)?),
            Param::LayerSolo(l) => Param::LayerSolo(layer(l)?),
            Param::LayerTransition(l) => Param::LayerTransition(layer(l)?),
            Param::GroupOpacity(g) => Param::GroupOpacity(group(g)?),
            Param::GroupBypass(g) => Param::GroupBypass(group(g)?),
            Param::Effect {
                target: t,
                index,
                name,
            } => Param::Effect {
                target: target(t)?,
                index,
                name,
            },
            Param::EffectBypass { target: t, index } => Param::EffectBypass {
                target: target(t)?,
                index,
            },
            other @ (Param::Master | Param::Crossfader | Param::Bpm) => other,
        })
    }
}

impl Action {
    pub fn without_layer(&self, removed: usize) -> Option<Action> {
        let shift = |i| crate::effect::shifted(i, removed);
        Some(match self.clone() {
            Action::Set(p) => Action::Set(p.without_layer(removed)?),
            Action::LaunchClip { layer, column } => Action::LaunchClip {
                layer: shift(layer)?,
                column,
            },
            Action::EjectLayer(l) => Action::EjectLayer(shift(l)?),
            other => other,
        })
    }

    pub fn without_column(&self, removed: usize) -> Option<Action> {
        let shift = |i| crate::effect::shifted(i, removed);
        Some(match self.clone() {
            Action::Set(p) => Action::Set(p.without_column(removed)?),
            Action::LaunchClip { layer, column } => Action::LaunchClip {
                layer,
                column: shift(column)?,
            },
            Action::LaunchColumn(c) => Action::LaunchColumn(shift(c)?),
            Action::LaunchGroupColumn { group, column } => Action::LaunchGroupColumn {
                group,
                column: shift(column)?,
            },
            other => other,
        })
    }

    pub fn without_group(&self, removed: usize) -> Option<Action> {
        let shift = |i| crate::effect::shifted(i, removed);
        Some(match self.clone() {
            Action::Set(p) => Action::Set(p.without_group(removed)?),
            Action::LaunchGroupColumn { group, column } => Action::LaunchGroupColumn {
                group: shift(group)?,
                column,
            },
            other => other,
        })
    }
}

fn effect_prefix(target: EffectTarget) -> String {
    match target {
        EffectTarget::Composition => "/composition".to_owned(),
        EffectTarget::Group(g) => format!("/group/{}", g + 1),
        EffectTarget::Layer(l) => format!("/layer/{}", l + 1),
        EffectTarget::Clip { layer, column } => format!("/layer/{}/clip/{}", layer + 1, column + 1),
    }
}

impl Action {
    pub fn address(&self) -> String {
        match self {
            Action::Set(param) => match param {
                Param::Master => "/master".into(),
                Param::Crossfader => "/crossfader".into(),
                Param::Bpm => "/bpm".into(),
                Param::LayerOpacity(l) => format!("/layer/{}/opacity", l + 1),
                Param::LayerBypass(l) => format!("/layer/{}/bypass", l + 1),
                Param::LayerSolo(l) => format!("/layer/{}/solo", l + 1),
                Param::LayerTransition(l) => format!("/layer/{}/transition", l + 1),
                Param::GroupOpacity(g) => format!("/group/{}/opacity", g + 1),
                Param::GroupBypass(g) => format!("/group/{}/bypass", g + 1),
                Param::Effect {
                    target,
                    index,
                    name,
                } => format!("{}/effect/{}/{name}", effect_prefix(*target), index + 1),
                Param::EffectBypass { target, index } => {
                    format!("{}/effect/{}/bypass", effect_prefix(*target), index + 1)
                }
            },
            Action::LaunchClip { layer, column } => {
                format!("/layer/{}/clip/{}/launch", layer + 1, column + 1)
            }
            Action::LaunchColumn(c) => format!("/column/{}/launch", c + 1),
            Action::LaunchGroupColumn { group, column } => {
                format!("/group/{}/column/{}/launch", group + 1, column + 1)
            }
            Action::EjectLayer(l) => format!("/layer/{}/eject", l + 1),
            Action::ClearAll => "/clear".into(),
            Action::CueGo => "/cue/go".into(),
            Action::CuePrevious => "/cue/previous".into(),
            Action::GoCue(i) => format!("/cue/{}/go", i + 1),
        }
    }

    /// Parses an address made by [`Action::address`].
    pub fn parse(address: &str) -> Option<Action> {
        let parts: Vec<&str> = address.trim_start_matches('/').split('/').collect();
        // 1-based index in the address to 0-based.
        let index = |s: &str| s.parse::<usize>().ok()?.checked_sub(1);
        let effect = |target: EffectTarget, rest: &[&str]| -> Option<Action> {
            match rest {
                ["effect", i, "bypass"] => Some(Action::Set(Param::EffectBypass {
                    target,
                    index: index(i)?,
                })),
                ["effect", i, name] => Some(Action::Set(Param::Effect {
                    target,
                    index: index(i)?,
                    name: (*name).to_owned(),
                })),
                _ => None,
            }
        };
        Some(match parts.as_slice() {
            ["master"] => Action::Set(Param::Master),
            ["crossfader"] => Action::Set(Param::Crossfader),
            ["bpm"] => Action::Set(Param::Bpm),
            ["clear"] => Action::ClearAll,
            ["cue", "go"] => Action::CueGo,
            ["cue", "previous"] => Action::CuePrevious,
            ["cue", i, "go"] => Action::GoCue(index(i)?),
            ["column", c, "launch"] => Action::LaunchColumn(index(c)?),
            ["layer", l, "opacity"] => Action::Set(Param::LayerOpacity(index(l)?)),
            ["layer", l, "bypass"] => Action::Set(Param::LayerBypass(index(l)?)),
            ["layer", l, "solo"] => Action::Set(Param::LayerSolo(index(l)?)),
            ["layer", l, "transition"] => Action::Set(Param::LayerTransition(index(l)?)),
            ["layer", l, "eject"] => Action::EjectLayer(index(l)?),
            ["layer", l, "clip", c, "launch"] => Action::LaunchClip {
                layer: index(l)?,
                column: index(c)?,
            },
            ["group", g, "column", c, "launch"] => Action::LaunchGroupColumn {
                group: index(g)?,
                column: index(c)?,
            },
            ["group", g, "opacity"] => Action::Set(Param::GroupOpacity(index(g)?)),
            ["group", g, "bypass"] => Action::Set(Param::GroupBypass(index(g)?)),
            ["composition", rest @ ..] => return effect(EffectTarget::Composition, rest),
            ["group", g, rest @ ..] => return effect(EffectTarget::Group(index(g)?), rest),
            ["layer", l, "clip", c, rest @ ..] => {
                return effect(
                    EffectTarget::Clip {
                        layer: index(l)?,
                        column: index(c)?,
                    },
                    rest,
                );
            }
            ["layer", l, rest @ ..] => return effect(EffectTarget::Layer(index(l)?), rest),
            _ => return None,
        })
    }

    /// Parses an OSC message in either Verso's own address space or the
    /// address scheme used by clip-grid VJ software and their controller
    /// presets (`/composition/layers/1/clips/2/connect`, see
    /// [`Action::parse_arena`]). Returns the action and its value in 0..1.
    ///
    /// Values arriving in their own unit (tempo in BPM, transition in
    /// seconds) are converted to 0..1 here. A message without a value is a
    /// button press.
    pub fn parse_osc(address: &str, value: Option<f32>) -> Option<(Action, f32)> {
        if let Some(action) = Action::parse(address) {
            return Some((action, value.unwrap_or(1.0)));
        }
        let (action, raw) = Action::parse_arena(address)?;
        let value = value.unwrap_or(1.0);
        let unit = match (raw, &action) {
            (true, Action::Set(param)) => param.builtin_range()?.to_unit(value),
            _ => value,
        };
        Some((action, unit))
    }

    /// Parses the `/composition/...` address scheme shared by common VJ
    /// software, so controller presets and OSC apps built for it drive Verso
    /// unchanged. Only addresses documented publicly in their user manuals
    /// are covered. Returns the action and whether the value comes in its own
    /// unit (BPM, seconds) rather than 0..1.
    ///
    /// Layers count from the bottom and indices start at 1, as in Verso.
    pub fn parse_arena(address: &str) -> Option<(Action, bool)> {
        let parts: Vec<&str> = address.trim_start_matches('/').split('/').collect();
        let index = |s: &str| s.parse::<usize>().ok()?.checked_sub(1);
        let unit = |a| Some((a, false));
        let raw = |a| Some((a, true));
        match parts.as_slice() {
            ["composition", "master"] => unit(Action::Set(Param::Master)),
            ["composition", "crossfader", "phase"] => unit(Action::Set(Param::Crossfader)),
            ["composition", "tempocontroller", "tempo"] => raw(Action::Set(Param::Bpm)),
            ["composition", "disconnectall"] => unit(Action::ClearAll),
            ["composition", "columns", c, "connect"] => unit(Action::LaunchColumn(index(c)?)),
            ["composition", "layers", l, "clips", c, "connect"] => unit(Action::LaunchClip {
                layer: index(l)?,
                column: index(c)?,
            }),
            ["composition", "layers", l, "clear"] => unit(Action::EjectLayer(index(l)?)),
            ["composition", "layers", l, "video", "opacity"] => {
                unit(Action::Set(Param::LayerOpacity(index(l)?)))
            }
            ["composition", "layers", l, "bypassed"] => {
                unit(Action::Set(Param::LayerBypass(index(l)?)))
            }
            ["composition", "layers", l, "solo"] => unit(Action::Set(Param::LayerSolo(index(l)?))),
            ["composition", "layers", l, "transition", "duration"] => {
                raw(Action::Set(Param::LayerTransition(index(l)?)))
            }
            ["composition", "groups", g, "columns", c, "connect"] => {
                unit(Action::LaunchGroupColumn {
                    group: index(g)?,
                    column: index(c)?,
                })
            }
            ["composition", "groups", g, "video", "opacity"] => {
                unit(Action::Set(Param::GroupOpacity(index(g)?)))
            }
            ["composition", "groups", g, "bypassed"] => {
                unit(Action::Set(Param::GroupBypass(index(g)?)))
            }
            _ => None,
        }
    }

    /// Command for a control value in 0..1. Triggers fire when the value rises
    /// above 0.5, so a button's release does nothing.
    pub fn command(&self, unit: f32, range: Option<Range>) -> Option<Command> {
        if !unit.is_finite() {
            return None;
        }
        let pressed = unit > 0.5;
        match self {
            Action::Set(param) => {
                let range = range.or_else(|| param.builtin_range())?;
                Some(param.command(range.from_unit(unit), range))
            }
            Action::LaunchClip { layer, column } => pressed.then_some(Command::TriggerClip {
                layer: *layer,
                column: *column,
            }),
            Action::LaunchColumn(c) => pressed.then_some(Command::TriggerColumn(*c)),
            Action::LaunchGroupColumn { group, column } => {
                pressed.then_some(Command::LaunchGroupColumn {
                    group: *group,
                    column: *column,
                })
            }
            Action::EjectLayer(l) => pressed.then_some(Command::EjectLayer(*l)),
            Action::ClearAll => pressed.then_some(Command::ClearAll),
            Action::CueGo => pressed.then_some(Command::GoNextCue),
            Action::CuePrevious => pressed.then_some(Command::GoPreviousCue),
            Action::GoCue(i) => pressed.then_some(Command::GoCue(*i)),
        }
    }

    /// Short label for lists.
    pub fn label(&self) -> String {
        self.address()
    }
}

/// A MIDI message identity, without its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MidiKey {
    /// Control change: channel 0-15, controller 0-127.
    Cc { channel: u8, controller: u8 },
    /// Note on/off: channel 0-15, note 0-127.
    Note { channel: u8, note: u8 },
}

impl MidiKey {
    /// Parses a channel message into its key and a value in 0..1.
    pub fn parse(bytes: &[u8]) -> Option<(MidiKey, f32)> {
        let (&status, data) = bytes.split_first()?;
        let channel = status & 0x0F;
        match (status & 0xF0, data) {
            (0xB0, [controller, value, ..]) => Some((
                MidiKey::Cc {
                    channel,
                    controller: *controller,
                },
                *value as f32 / 127.0,
            )),
            (0x90, [note, velocity, ..]) => Some((
                MidiKey::Note {
                    channel,
                    note: *note,
                },
                *velocity as f32 / 127.0,
            )),
            (0x80, [note, ..]) => Some((
                MidiKey::Note {
                    channel,
                    note: *note,
                },
                0.0,
            )),
            _ => None,
        }
    }

    pub fn label(&self) -> String {
        match self {
            MidiKey::Cc {
                channel,
                controller,
            } => format!("CC {controller} · ch {}", channel + 1),
            MidiKey::Note { channel, note } => format!("Note {note} · ch {}", channel + 1),
        }
    }
}

/// An action fired when incoming timecode passes `seconds`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Cue {
    /// Position in seconds from 00:00:00:00.
    pub seconds: f64,
    pub action: Action,
    /// Control value in 0..1 (1 presses a trigger).
    #[serde(default = "Cue::default_value")]
    pub value: f32,
}

impl Cue {
    fn default_value() -> f32 {
        1.0
    }

    /// Indices of `cues` passed when the timecode moves from `from` to `to`
    /// seconds. Only forward motion of under a second fires cues, so a jump
    /// (seek, new song) does not fire everything in between.
    pub fn passed(cues: &[Cue], from: f64, to: f64) -> Vec<usize> {
        if !(to > from && to - from < 1.0) {
            return Vec::new();
        }
        cues.iter()
            .enumerate()
            .filter(|(_, c)| c.seconds > from && c.seconds <= to)
            .map(|(i, _)| i)
            .collect()
    }
}

/// A MIDI control bound to an action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiMapping {
    pub key: MidiKey,
    pub action: Action,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_round_trip() {
        let actions = [
            Action::Set(Param::Master),
            Action::Set(Param::Crossfader),
            Action::Set(Param::Bpm),
            Action::Set(Param::LayerOpacity(0)),
            Action::Set(Param::LayerBypass(2)),
            Action::Set(Param::LayerSolo(1)),
            Action::Set(Param::LayerTransition(3)),
            Action::Set(Param::GroupOpacity(0)),
            Action::Set(Param::GroupBypass(1)),
            Action::Set(Param::Effect {
                target: EffectTarget::Composition,
                index: 0,
                name: "amount".into(),
            }),
            Action::Set(Param::Effect {
                target: EffectTarget::Clip {
                    layer: 1,
                    column: 4,
                },
                index: 2,
                name: "radius".into(),
            }),
            Action::Set(Param::EffectBypass {
                target: EffectTarget::Layer(0),
                index: 1,
            }),
            Action::Set(Param::Effect {
                target: EffectTarget::Group(0),
                index: 0,
                name: "hue".into(),
            }),
            Action::LaunchClip {
                layer: 0,
                column: 5,
            },
            Action::LaunchColumn(3),
            Action::LaunchGroupColumn {
                group: 1,
                column: 2,
            },
            Action::EjectLayer(2),
            Action::ClearAll,
            Action::CueGo,
            Action::CuePrevious,
            Action::GoCue(4),
        ];
        for action in actions {
            let address = action.address();
            assert_eq!(Action::parse(&address), Some(action), "{address}");
        }
        assert_eq!(
            Action::parse("/layer/1/opacity"),
            Some(Action::Set(Param::LayerOpacity(0)))
        );
        assert_eq!(
            Action::parse("/layer/0/opacity"),
            None,
            "indices are 1-based"
        );
        assert_eq!(Action::parse("/nope"), None);
    }

    #[test]
    fn arena_style_addresses_are_accepted() {
        let cases = [
            (
                "/composition/layers/2/clips/3/connect",
                Action::LaunchClip {
                    layer: 1,
                    column: 2,
                },
            ),
            ("/composition/columns/4/connect", Action::LaunchColumn(3)),
            ("/composition/layers/1/clear", Action::EjectLayer(0)),
            ("/composition/disconnectall", Action::ClearAll),
            (
                "/composition/layers/3/video/opacity",
                Action::Set(Param::LayerOpacity(2)),
            ),
            (
                "/composition/layers/1/bypassed",
                Action::Set(Param::LayerBypass(0)),
            ),
            (
                "/composition/layers/1/solo",
                Action::Set(Param::LayerSolo(0)),
            ),
            ("/composition/master", Action::Set(Param::Master)),
            (
                "/composition/crossfader/phase",
                Action::Set(Param::Crossfader),
            ),
            (
                "/composition/groups/2/columns/1/connect",
                Action::LaunchGroupColumn {
                    group: 1,
                    column: 0,
                },
            ),
            (
                "/composition/groups/1/video/opacity",
                Action::Set(Param::GroupOpacity(0)),
            ),
        ];
        for (address, action) in cases {
            assert_eq!(
                Action::parse_osc(address, Some(1.0)),
                Some((action, 1.0)),
                "{address}"
            );
        }
        // Tempo arrives in BPM and transition in seconds: converted to 0..1.
        let (action, unit) =
            Action::parse_osc("/composition/tempocontroller/tempo", Some(260.0)).unwrap();
        assert_eq!(action, Action::Set(Param::Bpm));
        assert!((unit - 0.5).abs() < 1e-6);
        let (action, unit) =
            Action::parse_osc("/composition/layers/1/transition/duration", Some(5.0)).unwrap();
        assert_eq!(action, Action::Set(Param::LayerTransition(0)));
        assert!((unit - 0.5).abs() < 1e-6);
        // Verso's own addresses still win, and a missing value is a press.
        assert_eq!(
            Action::parse_osc("/column/1/launch", None),
            Some((Action::LaunchColumn(0), 1.0))
        );
        assert_eq!(
            Action::parse_osc("/composition/layers/0/clear", Some(1.0)),
            None
        );
        assert_eq!(Action::parse_osc("/composition/nope", Some(1.0)), None);
    }

    #[test]
    fn unit_values_map_to_ranges() {
        let mut comp = Composition::demo();
        let bpm = Action::Set(Param::Bpm).command(0.5, None).unwrap();
        comp.apply(bpm).unwrap();
        assert_eq!(comp.bpm, 260.0);

        let bypass = Action::Set(Param::LayerBypass(0))
            .command(1.0, None)
            .unwrap();
        comp.apply(bypass).unwrap();
        assert!(comp.layers[0].bypass);

        // Effect parameters need a range from the effect definition.
        let effect = Action::Set(Param::Effect {
            target: EffectTarget::Layer(0),
            index: 0,
            name: "x".into(),
        });
        assert_eq!(effect.command(0.5, None), None);
    }

    #[test]
    fn triggers_fire_on_press_only() {
        let launch = Action::LaunchColumn(1);
        assert_eq!(launch.command(1.0, None), Some(Command::TriggerColumn(1)));
        assert_eq!(launch.command(0.0, None), None);
    }

    #[test]
    fn param_get_and_write() {
        let mut comp = Composition::demo();
        let param = Param::LayerOpacity(2);
        param.write(&mut comp, 0.25, Range::UNIT);
        assert_eq!(param.get(&comp), Some(0.25));
        assert_eq!(Param::LayerOpacity(99).get(&comp), None);
    }

    #[test]
    fn cues_fire_on_forward_play_only() {
        let cue = |seconds| Cue {
            seconds,
            action: Action::ClearAll,
            value: 1.0,
        };
        let cues = [cue(10.0), cue(10.5), cue(30.0)];
        assert_eq!(Cue::passed(&cues, 9.98, 10.02), [0]);
        assert_eq!(Cue::passed(&cues, 10.0, 10.5), [1], "start is exclusive");
        assert!(
            Cue::passed(&cues, 0.0, 30.0).is_empty(),
            "a jump fires nothing"
        );
        assert!(Cue::passed(&cues, 10.6, 10.4).is_empty(), "backwards");
    }

    #[test]
    fn midi_messages_parse() {
        assert_eq!(
            MidiKey::parse(&[0xB2, 7, 127]),
            Some((
                MidiKey::Cc {
                    channel: 2,
                    controller: 7
                },
                1.0
            ))
        );
        assert_eq!(
            MidiKey::parse(&[0x90, 60, 0]),
            Some((
                MidiKey::Note {
                    channel: 0,
                    note: 60
                },
                0.0
            ))
        );
        assert_eq!(
            MidiKey::parse(&[0x81, 60, 64]),
            Some((
                MidiKey::Note {
                    channel: 1,
                    note: 60
                },
                0.0
            ))
        );
        assert_eq!(MidiKey::parse(&[0xF8]), None, "clock is ignored");
    }
}
