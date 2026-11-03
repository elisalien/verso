//! MilkDrop Settings panel. Blend, pattern and input mix can be set for every
//! MilkDrop clip at once or for one clip in its Properties: the last edit
//! wins, so a change here overwrites every clip, and a clip edited afterwards
//! keeps its own value. Layer blends and audio gain apply to all clips.

use eframe::egui;
use serde::{Deserialize, Serialize};
use verso_core::{Command, MilkPattern, Source};
use verso_render::milkdrop::MilkSettings;

use crate::app::{PropsTab, VersoApp};
use crate::{theme, widgets};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MilkPrefs {
    /// Values of the last edit made for all clips; new clips start with them.
    pub blend: f32,
    pub pattern: MilkPattern,
    pub input_mix: f32,
    /// See [`MilkSettings::layer_blend`].
    pub layer_blend: bool,
    /// See [`MilkSettings::audio_gain`].
    pub audio_gain: f32,
}

impl Default for MilkPrefs {
    fn default() -> Self {
        MilkPrefs {
            blend: Source::MILK_BLEND,
            pattern: MilkPattern::Random,
            input_mix: 0.0,
            layer_blend: true,
            audio_gain: 1.0,
        }
    }
}

fn prefs_file() -> Option<std::path::PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("milkdrop.ron"))
}

impl MilkPrefs {
    pub fn load() -> Self {
        prefs_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .and_then(|text| ron::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(file) = prefs_file() else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = ron::to_string(self) {
            let _ = std::fs::write(file, text);
        }
    }

    pub fn render_settings(&self) -> MilkSettings {
        MilkSettings {
            layer_blend: self.layer_blend,
            audio_gain: self.audio_gain,
        }
    }

    /// A new MilkDrop source with the values set for all clips.
    pub fn source(&self, family: &str, variant: usize) -> Source {
        let mut source = Source::milkdrop(family, variant);
        if let Source::Milkdrop {
            blend,
            pattern,
            input_mix,
            ..
        } = &mut source
        {
            (*blend, *pattern, *input_mix) = (self.blend, self.pattern, self.input_mix);
        }
        source
    }
}

/// A setting edited for all clips this frame.
#[derive(Clone, Copy)]
enum AllEdit {
    Blend(f32),
    Pattern(MilkPattern),
    InputMix(f32),
}

impl AllEdit {
    /// Sets this value on a MilkDrop source; `false` when it already had it.
    fn apply(self, source: &mut Source) -> bool {
        let Source::Milkdrop {
            blend,
            pattern,
            input_mix,
            ..
        } = source
        else {
            return false;
        };
        match self {
            AllEdit::Blend(v) if *blend != v => *blend = v,
            AllEdit::Pattern(v) if *pattern != v => *pattern = v,
            AllEdit::InputMix(v) if *input_mix != v => *input_mix = v,
            _ => return false,
        }
        true
    }
}

/// Slots of the MilkDrop clips, layer by layer from the top.
fn milk_slots(comp: &verso_core::Composition) -> Vec<(usize, usize)> {
    (0..comp.layers.len())
        .rev()
        .flat_map(|l| {
            comp.layers[l]
                .clips
                .iter()
                .enumerate()
                .filter(|(_, clip)| {
                    clip.as_ref()
                        .is_some_and(|c| matches!(c.source, Source::Milkdrop { .. }))
                })
                .map(move |(c, _)| (l, c))
        })
        .collect()
}

impl VersoApp {
    pub(crate) fn milkdrop_settings_panel(&mut self, ui: &mut egui::Ui) {
        widgets::header(ui, "MilkDrop Settings");
        let slots = milk_slots(&self.comp);
        egui::ScrollArea::vertical().show(ui, |ui| {
            self.milk_all_clips(ui, &slots);
            ui.add_space(8.0);
            self.milk_shared(ui);
            ui.add_space(8.0);
            self.milk_clip_list(ui, &slots);
        });
    }

    /// Settings written to every MilkDrop clip.
    fn milk_all_clips(&mut self, ui: &mut egui::Ui, slots: &[(usize, usize)]) {
        section(ui, "All MilkDrop clips");
        hint(
            ui,
            "A change here applies to every MilkDrop clip and to new ones. \
             Editing a clip afterwards overrides it for that clip only.",
        );
        // How many clips differ from each value.
        let sources: Vec<&Source> = slots
            .iter()
            .filter_map(|&(l, c)| self.comp.layers[l].clips[c].as_ref())
            .map(|clip| &clip.source)
            .collect();
        let differ = |test: &dyn Fn(f32, MilkPattern, f32) -> bool| {
            sources
                .iter()
                .filter(|s| match s {
                    Source::Milkdrop {
                        blend,
                        pattern,
                        input_mix,
                        ..
                    } => test(*blend, *pattern, *input_mix),
                    _ => false,
                })
                .count()
        };
        let prefs = self.milk_prefs;
        let overrides = [
            differ(&|b, _, _| b != prefs.blend),
            differ(&|_, p, _| p != prefs.pattern),
            differ(&|_, _, m| m != prefs.input_mix),
        ];
        let mut edit = None;
        let mut values = prefs;
        egui::Grid::new("milkdrop all")
            .num_columns(3)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Blend");
                if ui
                    .add(egui::Slider::new(&mut values.blend, 0.0..=10.0).suffix(" s"))
                    .on_hover_text("Seconds of preset blend; 0 cuts")
                    .changed()
                {
                    edit = Some(AllEdit::Blend(values.blend));
                }
                override_note(ui, overrides[0]);
                ui.end_row();

                ui.label("Pattern");
                egui::ComboBox::from_id_salt("milkdrop all pattern")
                    .selected_text(values.pattern.name())
                    .show_ui(ui, |ui| {
                        for p in MilkPattern::ALL {
                            if ui
                                .selectable_value(&mut values.pattern, p, p.name())
                                .clicked()
                            {
                                edit = Some(AllEdit::Pattern(p));
                            }
                        }
                    })
                    .response
                    .on_hover_text("Shape of the blend: where the new preset takes over first");
                override_note(ui, overrides[1]);
                ui.end_row();

                ui.label("Input Mix");
                if ui
                    .add(egui::Slider::new(&mut values.input_mix, 0.0..=1.0))
                    .on_hover_text(
                        "How much of the input layer is mixed into the feedback, \
                         for clips that have an input",
                    )
                    .changed()
                {
                    edit = Some(AllEdit::InputMix(values.input_mix));
                }
                override_note(ui, overrides[2]);
                ui.end_row();
            });
        if let Some(edit) = edit {
            self.milk_prefs = values;
            self.milk_prefs.save();
            for &(layer, column) in slots {
                let Some(mut clip) = self.comp.layers[layer].clips[column].clone() else {
                    continue;
                };
                if edit.apply(&mut clip.source) {
                    self.send(Command::SetClip {
                        layer,
                        column,
                        clip: Some(clip),
                    });
                }
            }
        }
    }

    /// Settings that only exist for all clips.
    fn milk_shared(&mut self, ui: &mut egui::Ui) {
        section(ui, "Playback");
        let mut prefs = self.milk_prefs;
        egui::Grid::new("milkdrop shared")
            .num_columns(2)
            .spacing([8.0, 6.0])
            .show(ui, |ui| {
                ui.label("Between clips");
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut prefs.layer_blend, true, "Preset blend")
                        .on_hover_text(
                            "Launching a MilkDrop clip over another on the same layer \
                             blends the presets, using the new clip's Blend and Pattern",
                        );
                    ui.selectable_value(&mut prefs.layer_blend, false, "Layer transition")
                        .on_hover_text(
                            "The layer's transition crossfades the two images, \
                             like other sources",
                        );
                });
                ui.end_row();

                ui.label("Audio Gain");
                ui.add(egui::Slider::new(&mut prefs.audio_gain, 0.0..=4.0))
                    .on_hover_text("How strongly the presets react to the audio input");
                ui.end_row();

                ui.label("Presets");
                let count = self.renderer.milkdrop().library().sorted().len();
                ui.label(format!("{count} families"));
                ui.end_row();
                if let Some(dir) = crate::milkdrop_ui::user_milkdrop_dir() {
                    ui.label("Your folder");
                    ui.label(egui::RichText::new(dir.display().to_string()).small())
                        .on_hover_text("Put Butterchurn .json presets here; they load at startup");
                    ui.end_row();
                }
            });
        if prefs != self.milk_prefs {
            self.milk_prefs = prefs;
            self.milk_prefs.save();
        }
    }

    /// Every MilkDrop clip, with its own values and a way back to the values
    /// set for all clips.
    fn milk_clip_list(&mut self, ui: &mut egui::Ui, slots: &[(usize, usize)]) {
        section(ui, &format!("Clips ({})", slots.len()));
        if slots.is_empty() {
            hint(ui, "No MilkDrop clip in the composition.");
            return;
        }
        let prefs = self.milk_prefs;
        egui::Grid::new("milkdrop clips")
            .num_columns(4)
            .striped(true)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for &(layer, column) in slots {
                    let Some(clip) = self.comp.layers[layer].clips[column].clone() else {
                        continue;
                    };
                    let Source::Milkdrop {
                        blend,
                        pattern,
                        input,
                        input_mix,
                        ..
                    } = clip.source
                    else {
                        continue;
                    };
                    let selected = self.selected_clip == Some((layer, column));
                    let label = format!("{} · {}", self.comp.layers[layer].name, clip.name);
                    if ui
                        .selectable_label(selected, label)
                        .on_hover_text("Select this clip to edit it in Properties")
                        .clicked()
                    {
                        self.selected_clip = Some((layer, column));
                        self.selected_layer = layer;
                        self.props_tab = PropsTab::Clip;
                    }
                    ui.label(
                        egui::RichText::new(format!("{blend:.1} s · {}", pattern.name()))
                            .small()
                            .color(theme::text_dim()),
                    );
                    let input_text = match input {
                        Some(i) => format!("← L{} · {:.0}%", i + 1, input_mix * 100.0),
                        None => "No input".to_owned(),
                    };
                    ui.label(
                        egui::RichText::new(input_text)
                            .small()
                            .color(theme::text_dim()),
                    );
                    let own = blend != prefs.blend
                        || pattern != prefs.pattern
                        || input_mix != prefs.input_mix;
                    if ui
                        .add_enabled(own, egui::Button::new("Reset").small())
                        .on_hover_text("Back to the values set for all clips")
                        .clicked()
                    {
                        let mut clip = clip.clone();
                        if let Source::Milkdrop {
                            blend,
                            pattern,
                            input_mix,
                            ..
                        } = &mut clip.source
                        {
                            (*blend, *pattern, *input_mix) =
                                (prefs.blend, prefs.pattern, prefs.input_mix);
                        }
                        self.send(Command::SetClip {
                            layer,
                            column,
                            clip: Some(clip),
                        });
                    }
                    ui.end_row();
                }
            });
    }
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.label(egui::RichText::new(title).strong().color(theme::milkdrop()));
}

fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(theme::text_dim()));
}

/// Number of clips whose own value differs from the one set for all.
fn override_note(ui: &mut egui::Ui, count: usize) {
    if count > 0 {
        ui.label(
            egui::RichText::new(format!("{count} own"))
                .small()
                .color(theme::milkdrop()),
        )
        .on_hover_text(format!(
            "{count} clip(s) have their own value; a change here sets it on all of them"
        ));
    } else {
        ui.label("");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_for_all_change_only_their_field() {
        let mut source = Source::milkdrop("a/b", 0);
        if let Source::Milkdrop { pattern, .. } = &mut source {
            *pattern = MilkPattern::Radial;
        }
        assert!(AllEdit::Blend(5.0).apply(&mut source));
        assert!(!AllEdit::Blend(5.0).apply(&mut source), "unchanged");
        let Source::Milkdrop { blend, pattern, .. } = source else {
            unreachable!()
        };
        assert_eq!((blend, pattern), (5.0, MilkPattern::Radial));
        assert!(!AllEdit::Blend(1.0).apply(&mut Source::Solid { color: [0.0; 4] }));
    }

    #[test]
    fn new_sources_take_the_values_for_all() {
        let prefs = MilkPrefs {
            blend: 7.0,
            pattern: MilkPattern::Plasma,
            input_mix: 0.4,
            ..MilkPrefs::default()
        };
        let Source::Milkdrop {
            blend,
            pattern,
            input_mix,
            ..
        } = prefs.source("a/b", 1)
        else {
            unreachable!()
        };
        assert_eq!((blend, pattern, input_mix), (7.0, MilkPattern::Plasma, 0.4));
        let text = ron::to_string(&prefs).unwrap();
        assert_eq!(ron::from_str::<MilkPrefs>(&text).unwrap(), prefs);
    }
}
