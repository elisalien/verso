//! Keeps compositions consistent: files from disk are repaired after
//! parsing, and commands carrying non-finite numbers are refused.

use crate::modulation::{ModSource, Modulator};
use crate::{Clip, Command, Composition, Effect, EffectEdit, ParamValue, Source};

/// Smallest and largest composition side, in pixels.
pub const MIN_SIZE: u32 = 16;
pub const MAX_SIZE: u32 = 8192;

fn finite(v: f32, default: f32, min: f32, max: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, max)
    } else {
        default
    }
}

fn param_finite(value: &ParamValue) -> bool {
    match value {
        ParamValue::Float(v) => v.is_finite(),
        ParamValue::Color(c) => c.iter().all(|v| v.is_finite()),
        ParamValue::Point(p) => p.iter().all(|v| v.is_finite()),
        ParamValue::Bool(_) => true,
    }
}

fn effect_finite(effect: &Effect) -> bool {
    effect.params.values().all(param_finite)
}

fn source_finite(source: &Source) -> bool {
    match source {
        Source::Solid { color } => color.iter().all(|v| v.is_finite()),
        Source::Generator {
            color,
            speed,
            params,
            ..
        } => speed.is_finite() && color.iter().all(|v| v.is_finite()) && params.is_finite(),
        Source::Shader(effect) => effect_finite(effect),
        Source::Video { speed, .. } => speed.is_finite(),
        Source::Milkdrop {
            blend, input_mix, ..
        } => blend.is_finite() && input_mix.is_finite(),
        Source::Image { .. } | Source::Ndi { .. } | Source::Spout { .. } => true,
        Source::Text { style, .. } => style.is_valid(),
        Source::Time { mode, style, .. } => {
            style.is_valid()
                && match mode {
                    crate::text::TimeMode::CountDown { seconds } => seconds.is_finite(),
                    _ => true,
                }
        }
    }
}

fn clip_finite(clip: &Clip) -> bool {
    source_finite(&clip.source) && clip.effects.iter().all(effect_finite)
}

fn modulator_finite(m: &Modulator) -> bool {
    m.depth.is_finite()
        && match m.source {
            ModSource::Lfo { beats, .. } => beats.is_finite(),
            ModSource::Audio { .. } | ModSource::Mouse { .. } => true,
        }
}

impl Command {
    /// Whether every number in the command is finite. NaN and infinities
    /// survive `clamp`, so they are refused before they reach the model.
    pub fn is_finite(&self) -> bool {
        match self {
            Command::SetLayerOpacity { opacity: v, .. }
            | Command::SetLayerTransition { seconds: v, .. }
            | Command::SetGroupOpacity { opacity: v, .. }
            | Command::SetMasterOpacity(v)
            | Command::SetCrossfader(v)
            | Command::SetBpm(v)
            | Command::SetLayerBlendParam { value: v, .. }
            | Command::SetGroupBlendParam { value: v, .. } => v.is_finite(),
            Command::SetLayerBlendParams { params, .. }
            | Command::SetGroupBlendParams { params, .. } => params.is_finite(),
            Command::SetClip { clip, .. } => clip.as_ref().is_none_or(clip_finite),
            Command::EditEffects { edit, .. } => match edit {
                EffectEdit::Add(effect) => effect_finite(effect),
                EffectEdit::SetParam { value, .. } => param_finite(value),
                _ => true,
            },
            Command::AddModulator(m) | Command::SetModulator { modulator: m, .. } => {
                modulator_finite(m)
            }
            Command::AddTimecodeCue(c) | Command::SetTimecodeCue { cue: c, .. } => {
                c.seconds.is_finite() && c.value.is_finite()
            }
            Command::SetCueFade { seconds, .. } => seconds.is_finite(),
            _ => true,
        }
    }
}

fn repair_effects(effects: &mut [Effect], fixes: &mut Vec<String>, at: &str) {
    for effect in effects {
        let before = effect.params.len();
        effect.params.retain(|_, v| param_finite(v));
        if effect.params.len() != before {
            fixes.push(format!("{at}: invalid values of {} reset", effect.name));
        }
    }
}

fn repair_style(style: &mut crate::text::TextStyle, fixes: &mut Vec<String>, at: &str) {
    if !style.is_valid() {
        fixes.push(format!("{at}: invalid text style reset"));
        // Font, weight and alignment are always usable: keep them.
        let default = crate::text::TextStyle::default();
        *style = crate::text::TextStyle {
            font: std::mem::take(&mut style.font),
            weight: style.weight,
            italic: style.italic,
            align: style.align,
            animation: style.animation,
            ..default
        };
    }
}

fn repair_source(source: &mut Source, fixes: &mut Vec<String>, at: &str) {
    let ok = source_finite(source);
    match source {
        Source::Solid { color } | Source::Generator { color, .. } => {
            for v in color.iter_mut() {
                *v = finite(*v, 1.0, 0.0, 1.0);
            }
        }
        _ => {}
    }
    match source {
        Source::Generator {
            kind,
            speed,
            params,
            ..
        } => {
            *speed = finite(*speed, 1.0, -100.0, 100.0);
            params.repair(*kind);
        }
        Source::Video { speed, .. } => *speed = finite(*speed, 1.0, -100.0, 100.0),
        Source::Shader(effect) => repair_effects(std::slice::from_mut(effect), fixes, at),
        Source::Milkdrop {
            blend, input_mix, ..
        } => {
            *blend = finite(*blend, Source::MILK_BLEND, 0.0, 60.0);
            *input_mix = finite(*input_mix, 0.0, 0.0, 1.0);
        }
        Source::Text { style, .. } => repair_style(style, fixes, at),
        Source::Time { mode, style, .. } => {
            repair_style(style, fixes, at);
            if let crate::text::TimeMode::CountDown { seconds } = mode {
                *seconds = finite(*seconds, 0.0, 0.0, 360_000.0);
            }
        }
        _ => {}
    }
    if !ok {
        fixes.push(format!("{at}: invalid source values reset"));
    }
}

impl Composition {
    pub(crate) fn sort_timecode_cues(&mut self) {
        self.timecode_cues
            .sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
    }

    /// Makes a composition from a file safe to use: every layer has one cell
    /// per column, indices point at things that exist, numbers are finite and
    /// in range. Returns a description of each fix.
    pub fn repair(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
        let legacy = std::mem::take(&mut self.cues.legacy_timecode);
        if !legacy.is_empty() && self.timecode_cues.is_empty() {
            self.timecode_cues = legacy;
        }
        let (w, h) = (self.width, self.height);
        self.width = w.clamp(MIN_SIZE, MAX_SIZE);
        self.height = h.clamp(MIN_SIZE, MAX_SIZE);
        if (w, h) != (self.width, self.height) {
            fixes.push(format!(
                "size {w}×{h} changed to {}×{}",
                self.width, self.height
            ));
        }
        let bpm = self.bpm;
        self.bpm = finite(bpm, 120.0, 20.0, 500.0);
        if bpm != self.bpm {
            fixes.push(format!("tempo {bpm} changed to {}", self.bpm));
        }
        self.master.opacity = finite(self.master.opacity, 1.0, 0.0, 1.0);
        self.master.crossfader = finite(self.master.crossfader, 0.5, 0.0, 1.0);

        let columns = self.columns.len();
        let groups = self.groups.len();
        for (l, layer) in self.layers.iter_mut().enumerate() {
            let at = format!("layer {}", l + 1);
            if layer.clips.len() != columns {
                fixes.push(format!(
                    "{at}: {} cells for {columns} columns",
                    layer.clips.len()
                ));
                layer.clips.resize(columns, None);
            }
            layer.launches.resize(columns, 0);
            if let Some(c) = layer.active
                && layer.clips.get(c).is_none_or(Option::is_none)
            {
                layer.active = None;
            }
            if let Some(g) = layer.group
                && g >= groups
            {
                fixes.push(format!("{at}: group {} does not exist", g + 1));
                layer.group = None;
            }
            layer.opacity = finite(layer.opacity, 1.0, 0.0, 1.0);
            layer.transition = finite(layer.transition, 0.0, 0.0, 30.0);
            if layer.blend_params.repair() {
                fixes.push(format!("{at}: invalid blend parameters reset"));
            }
            repair_effects(&mut layer.effects, &mut fixes, &at);
            for (c, clip) in layer.clips.iter_mut().enumerate() {
                if let Some(clip) = clip {
                    let at = format!("{at}, column {}", c + 1);
                    repair_source(&mut clip.source, &mut fixes, &at);
                    repair_effects(&mut clip.effects, &mut fixes, &at);
                }
            }
        }
        for (g, group) in self.groups.iter_mut().enumerate() {
            if group.parent.is_some_and(|p| p >= groups || p == g) {
                fixes.push(format!("group {}: parent does not exist", g + 1));
                group.parent = None;
            }
            group.opacity = finite(group.opacity, 1.0, 0.0, 1.0);
            if group.blend_params.repair() {
                fixes.push(format!("group {}: invalid blend parameters reset", g + 1));
            }
            repair_effects(&mut group.effects, &mut fixes, &format!("group {}", g + 1));
        }
        repair_effects(&mut self.effects, &mut fixes, "composition");
        // Going to a cue writes its values into the layers and the master.
        for cue in &mut self.cues.cues {
            cue.fade = finite(cue.fade, 0.0, 0.0, 3600.0);
            cue.state.master = finite(cue.state.master, 1.0, 0.0, 1.0);
            cue.state.crossfader = finite(cue.state.crossfader, 0.5, 0.0, 1.0);
            for layer in &mut cue.state.layers {
                layer.opacity = finite(layer.opacity, 1.0, 0.0, 1.0);
                layer.active = layer.active.filter(|&c| c < columns);
            }
        }
        if self.cues.current.is_some_and(|c| c >= self.cues.cues.len()) {
            self.cues.current = None;
        }
        let cues = self.timecode_cues.len();
        self.timecode_cues
            .retain(|c| c.seconds.is_finite() && c.seconds >= 0.0 && c.value.is_finite());
        if self.timecode_cues.len() != cues {
            fixes.push("invalid timecode cues removed".into());
        }
        self.sort_timecode_cues();
        let before = self.modulators.len();
        self.modulators.retain(modulator_finite);
        if self.modulators.len() != before {
            fixes.push("invalid modulators removed".into());
        }
        fixes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inconsistent_files_are_repaired() {
        let mut comp = Composition::demo();
        comp.layers[0].clips.truncate(1);
        comp.layers[0].launches.clear();
        comp.layers[1].group = Some(7);
        comp.layers[2].active = Some(99);
        comp.layers[2].opacity = f32::NAN;
        comp.width = 100_000;
        comp.bpm = f32::INFINITY;
        let text = comp.to_ron().unwrap();
        let mut comp = Composition::from_ron(&text).unwrap();
        // Launching every column must not panic.
        for c in 0..comp.columns.len() {
            comp.apply(Command::TriggerColumn(c)).unwrap();
        }
        assert_eq!(comp.layers[0].clips.len(), comp.columns.len());
        assert_eq!(comp.layers[1].group, None);
        assert_eq!(comp.layers[2].opacity, 1.0);
        assert_eq!(comp.width, MAX_SIZE);
        assert_eq!(comp.bpm, 120.0);
    }

    #[test]
    fn non_finite_commands_are_refused() {
        let mut comp = Composition::demo();
        let before = comp.clone();
        assert!(
            comp.apply(Command::SetLayerOpacity {
                layer: 0,
                opacity: f32::NAN
            })
            .is_err()
        );
        assert!(comp.apply(Command::SetBpm(f32::INFINITY)).is_err());
        assert_eq!(comp, before);
    }

    #[test]
    fn cues_groups_and_text_are_repaired() {
        let mut comp = Composition::demo();
        comp.groups.push(crate::Group::new("A"));
        comp.groups[0].parent = Some(9);
        comp.apply(Command::RecordCue { name: None }).unwrap();
        comp.cues.cues[0].state.master = f32::NAN;
        comp.cues.cues[0].state.layers[0].opacity = 7.0;
        comp.cues.cues[0].state.layers[0].active = Some(99);
        comp.cues.current = Some(5);
        let style = crate::text::TextStyle {
            size: 1000.0,
            weight: 700,
            ..Default::default()
        };
        comp.layers[0].clips[0] = Some(Clip::new(
            "Text",
            Source::Text {
                text: "hi".into(),
                style,
            },
        ));
        comp.layers[1].clips[0] = Some(Clip::new("Milk", Source::milkdrop("x", 0)));
        if let Some(Clip {
            source: Source::Milkdrop { input_mix, .. },
            ..
        }) = &mut comp.layers[1].clips[0]
        {
            *input_mix = f32::NAN;
        }
        let fixes = comp.repair();
        assert!(!fixes.is_empty());
        assert_eq!(comp.groups[0].parent, None);
        let cue = &comp.cues.cues[0];
        assert_eq!(cue.state.master, 1.0);
        assert_eq!(cue.state.layers[0].opacity, 1.0);
        assert_eq!(cue.state.layers[0].active, None);
        assert_eq!(comp.cues.current, None);
        match &comp.layers[0].clips[0].as_ref().unwrap().source {
            Source::Text { style, .. } => {
                assert!(style.is_valid());
                assert_eq!(style.weight, 700);
            }
            other => panic!("{other:?}"),
        }
        match &comp.layers[1].clips[0].as_ref().unwrap().source {
            Source::Milkdrop { input_mix, .. } => assert_eq!(*input_mix, 0.0),
            other => panic!("{other:?}"),
        }
        assert!(comp.repair().is_empty());
    }

    #[test]
    fn removing_a_layer_renumbers_milkdrop_inputs() {
        let mut comp = Composition::demo_with_layers(4);
        let mut source = Source::milkdrop("x", 0);
        if let Source::Milkdrop { input, .. } = &mut source {
            *input = Some(3);
        }
        comp.layers[0].clips[0] = Some(Clip::new("Milk", source.clone()));
        comp.layers[1].clips[0] = Some(Clip::new("Milk", source));
        comp.apply(Command::RemoveLayer(2)).unwrap();
        let input = |c: &Composition, l: usize| match &c.layers[l].clips[0].as_ref().unwrap().source
        {
            Source::Milkdrop { input, .. } => *input,
            other => panic!("{other:?}"),
        };
        assert_eq!(input(&comp, 0), Some(2));
        comp.apply(Command::RemoveLayer(2)).unwrap();
        assert_eq!(input(&comp, 0), None);
    }

    #[test]
    fn backwards_parameter_ranges_do_not_panic() {
        use crate::control::{Param, Range};
        let range = Range {
            min: 1.0,
            max: 0.0,
            toggle: false,
        };
        let param = Param::Effect {
            target: crate::EffectTarget::Composition,
            index: 0,
            name: "x".into(),
        };
        let _ = param.command(0.5, range);
        let _ = param.command(
            0.5,
            Range {
                min: f32::NAN,
                max: f32::NAN,
                toggle: false,
            },
        );
    }
}
