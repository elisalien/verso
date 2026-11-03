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
        Source::Generator { color, speed, .. } => {
            speed.is_finite() && color.iter().all(|v| v.is_finite())
        }
        Source::Shader(effect) => effect_finite(effect),
        Source::Video { speed, .. } => speed.is_finite(),
        Source::Image { .. } | Source::Ndi { .. } => true,
    }
}

fn clip_finite(clip: &Clip) -> bool {
    source_finite(&clip.source) && clip.effects.iter().all(effect_finite)
}

fn modulator_finite(m: &Modulator) -> bool {
    m.depth.is_finite()
        && match m.source {
            ModSource::Lfo { beats, .. } => beats.is_finite(),
            ModSource::Audio { .. } => true,
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
            | Command::SetBpm(v) => v.is_finite(),
            Command::SetClip { clip, .. } => clip.as_ref().is_none_or(clip_finite),
            Command::EditEffects { edit, .. } => match edit {
                EffectEdit::Add(effect) => effect_finite(effect),
                EffectEdit::SetParam { value, .. } => param_finite(value),
                _ => true,
            },
            Command::AddModulator(m) | Command::SetModulator { modulator: m, .. } => {
                modulator_finite(m)
            }
            Command::AddCue(c) | Command::SetCue { cue: c, .. } => {
                c.seconds.is_finite() && c.value.is_finite()
            }
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
        Source::Generator { speed, .. } => *speed = finite(*speed, 1.0, -100.0, 100.0),
        Source::Video { speed, .. } => *speed = finite(*speed, 1.0, 0.0, 100.0),
        Source::Shader(effect) => repair_effects(std::slice::from_mut(effect), fixes, at),
        _ => {}
    }
    if !ok {
        fixes.push(format!("{at}: invalid source values reset"));
    }
}

impl Composition {
    pub(crate) fn sort_cues(&mut self) {
        self.cues.sort_by(|a, b| a.seconds.total_cmp(&b.seconds));
    }

    /// Makes a composition from a file safe to use: every layer has one cell
    /// per column, indices point at things that exist, numbers are finite and
    /// in range. Returns a description of each fix.
    pub fn repair(&mut self) -> Vec<String> {
        let mut fixes = Vec::new();
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
            group.opacity = finite(group.opacity, 1.0, 0.0, 1.0);
            repair_effects(&mut group.effects, &mut fixes, &format!("group {}", g + 1));
        }
        repair_effects(&mut self.effects, &mut fixes, "composition");
        let cues = self.cues.len();
        self.cues
            .retain(|c| c.seconds.is_finite() && c.seconds >= 0.0 && c.value.is_finite());
        if self.cues.len() != cues {
            fixes.push("invalid timecode cues removed".into());
        }
        self.sort_cues();
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
}
