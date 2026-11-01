//! Cues panel: the cue list with Go, Previous and Record, and each cue's
//! fade and follow. Space is Go everywhere in the window (see
//! `VersoApp::handle_keys`).

use eframe::egui::{self, Color32, RichText};
use verso_core::Command;
use verso_core::control::Action;
use verso_core::cue::Follow;

use crate::app::VersoApp;
use crate::theme;

impl VersoApp {
    pub(crate) fn cues_panel(&mut self, ui: &mut egui::Ui) {
        self.cue_transport(ui);
        ui.separator();
        if self.comp.cues.cues.is_empty() {
            ui.label(
                RichText::new(
                    "No cues yet. Set up the clips and faders, then Record: each cue \
                     stores the clip playing and the opacity of every layer, the \
                     master and the crossfader. Go (Space) plays the next cue with \
                     its fade.",
                )
                .small()
                .color(theme::text_dim()),
            );
            return;
        }
        egui::ScrollArea::both()
            .auto_shrink([false, true])
            .show(ui, |ui| self.cue_rows(ui));
    }

    fn cue_transport(&mut self, ui: &mut egui::Ui) {
        let cues = &self.comp.cues;
        let next = cues.current.map_or(0, |c| c + 1);
        let next_name = cues.cues.get(next).map(|c| c.name.clone());
        let current = cues.current;
        // Wraps in a narrow panel instead of squeezing the labels.
        ui.horizontal_wrapped(|ui| {
            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
            let go = ui.add_enabled(
                next_name.is_some(),
                egui::Button::new(RichText::new("GO").strong().size(18.0))
                    .min_size(egui::vec2(72.0, 34.0))
                    .fill(theme::accent().gamma_multiply(0.6)),
            );
            let learning = self.learnable(ui, go.rect, Action::CueGo);
            if go
                .on_hover_text("Next cue (Space)")
                .on_disabled_hover_text("No cue after the current one")
                .clicked()
                && !learning
            {
                self.send(Command::GoNextCue);
            }
            let back = ui.add_enabled(
                current.is_some_and(|c| c > 0),
                egui::Button::new("Back").min_size(egui::vec2(28.0, 34.0)),
            );
            let learning = self.learnable(ui, back.rect, Action::CuePrevious);
            if back.on_hover_text("Previous cue").clicked() && !learning {
                self.send(Command::GoPreviousCue);
            }
            if ui
                .button("Record")
                .on_hover_text("Stores the current state as a new cue after the current one")
                .clicked()
            {
                self.send(Command::RecordCue { name: None });
            }
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Update"))
                .on_hover_text("Stores the current state into the current cue")
                .clicked()
                && let Some(c) = current
            {
                self.send(Command::UpdateCue(c));
            }
        });
        ui.horizontal_wrapped(|ui| {
            let label = match &next_name {
                Some(name) => format!("Next: {} {name}", next + 1),
                None if current.is_some() => "End of the list".to_owned(),
                None => String::new(),
            };
            ui.label(RichText::new(label).color(theme::text_dim()));
            if let Some(run) = &self.comp.cue_run {
                ui.add(
                    egui::ProgressBar::new(run.progress())
                        .desired_width(160.0)
                        .text(format!("Cue {} {:.1} s", run.cue + 1, run.elapsed)),
                );
            }
        });
    }

    fn cue_rows(&mut self, ui: &mut egui::Ui) {
        let count = self.comp.cues.cues.len();
        let current = self.comp.cues.current;
        let mut commands = Vec::new();
        egui::Grid::new("cues")
            .num_columns(6)
            .spacing([6.0, 3.0])
            .striped(true)
            .show(ui, |ui| {
                ui.label(RichText::new("#").small().color(theme::text_dim()));
                ui.label(RichText::new("Name").small().color(theme::text_dim()));
                ui.label(RichText::new("Fade").small().color(theme::text_dim()));
                ui.label(RichText::new("Then").small().color(theme::text_dim()));
                ui.label("");
                ui.label("");
                ui.end_row();
                for index in 0..count {
                    let cue = self.comp.cues.cues[index].clone();
                    let is_current = current == Some(index);
                    let number = egui::Button::new(RichText::new(format!("{}", index + 1)).color(
                        if is_current {
                            Color32::BLACK
                        } else {
                            theme::text()
                        },
                    ))
                    .min_size(egui::vec2(28.0, 0.0))
                    .fill(if is_current {
                        theme::accent()
                    } else {
                        theme::bg_raised()
                    });
                    let response = ui.add(number);
                    let learning = self.learnable(ui, response.rect, Action::GoCue(index));
                    if response.on_hover_text("Go to this cue").clicked() && !learning {
                        commands.push(Command::GoCue(index));
                    }

                    let mut name = cue.name.clone();
                    if ui
                        .add(egui::TextEdit::singleline(&mut name).desired_width(140.0))
                        .changed()
                    {
                        commands.push(Command::RenameCue { index, name });
                    }

                    let mut fade = cue.fade;
                    if ui
                        .add(
                            egui::DragValue::new(&mut fade)
                                .range(0.0..=600.0)
                                .speed(0.05)
                                .suffix(" s"),
                        )
                        .on_hover_text("Time to reach this cue; clips crossfade over it")
                        .changed()
                    {
                        commands.push(Command::SetCueFade {
                            index,
                            seconds: fade,
                        });
                    }

                    if let Some(follow) = follow_editor(ui, index, cue.follow) {
                        commands.push(Command::SetCueFollow { index, follow });
                    }

                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(index > 0, egui::Button::new("↑").small())
                            .clicked()
                        {
                            commands.push(Command::MoveCue {
                                from: index,
                                to: index - 1,
                            });
                        }
                        if ui
                            .add_enabled(index + 1 < count, egui::Button::new("↓").small())
                            .clicked()
                        {
                            commands.push(Command::MoveCue {
                                from: index,
                                to: index + 1,
                            });
                        }
                    });
                    if ui
                        .small_button("✕")
                        .on_hover_text("Delete cue (Ctrl+Z undoes)")
                        .clicked()
                    {
                        commands.push(Command::RemoveCue(index));
                    }
                    ui.end_row();
                }
            });
        for command in commands {
            self.send(command);
        }
    }
}

/// Edits what follows a cue; returns the new value when it changed.
fn follow_editor(ui: &mut egui::Ui, index: usize, follow: Follow) -> Option<Follow> {
    let mut changed = None;
    ui.horizontal(|ui| {
        let label = match follow {
            Follow::Manual => "Wait",
            Follow::After(_) => "After",
            Follow::Beats(_) => "Beats",
        };
        egui::ComboBox::from_id_salt(("cue follow", index))
            .selected_text(label)
            .width(64.0)
            .show_ui(ui, |ui| {
                let choices = [
                    (Follow::Manual, "Wait", "Wait for Go"),
                    (Follow::After(5.0), "After", "Next cue after some seconds"),
                    (Follow::Beats(16.0), "Beats", "Next cue after some beats"),
                ];
                for (choice, name, tip) in choices {
                    let same = std::mem::discriminant(&choice) == std::mem::discriminant(&follow);
                    if ui.selectable_label(same, name).on_hover_text(tip).clicked() && !same {
                        changed = Some(choice);
                    }
                }
            });
        match follow {
            Follow::Manual => {}
            Follow::After(mut s) => {
                if ui
                    .add(
                        egui::DragValue::new(&mut s)
                            .range(0.0..=3600.0)
                            .speed(0.1)
                            .suffix(" s"),
                    )
                    .changed()
                {
                    changed = Some(Follow::After(s));
                }
            }
            Follow::Beats(mut b) => {
                if ui
                    .add(egui::DragValue::new(&mut b).range(0.0..=1024.0).speed(0.25))
                    .on_hover_text("Beats at the composition tempo")
                    .changed()
                {
                    changed = Some(Follow::Beats(b));
                }
            }
        }
    });
    changed
}
