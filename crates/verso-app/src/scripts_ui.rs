//! Lua scripts: running them each frame and the Scripts window.

use eframe::egui;
use verso_core::{Command, Script};
use verso_script::{FrameInfo, ScriptHost};

use crate::app::VersoApp;
use crate::{theme, widgets};

const TEMPLATE: &str = r#"-- Runs every frame.
-- t.time, t.dt, t.beats, t.bpm, t.audio.level|bass|mid|high|beat
function frame(t)
  -- verso.set("/layer/1/opacity", 0.5 + 0.5 * math.sin(t.beats * math.pi / 2))
end

-- Runs on every beat of the tempo.
function beat(n)
  -- if n % 4 == 0 then
  --   verso.trigger("/column/" .. (n // 4 % 4 + 1) .. "/launch")
  -- end
end
"#;

/// Scripts window state.
#[derive(Default)]
pub struct ScriptsState {
    pub host: ScriptHost,
    pub window_open: bool,
    pub selected: usize,
    /// Text being edited and the script index it belongs to.
    draft: Option<(usize, String)>,
    /// Seconds since start, for `t.time`.
    time: f64,
}

impl VersoApp {
    /// Runs every enabled script for this frame.
    pub(crate) fn run_scripts(&mut self, dt: f64) {
        self.scripts.time += dt;
        if self.comp.scripts.is_empty() {
            self.scripts.host.retain(&[]);
            return;
        }
        let mut host = std::mem::take(&mut self.scripts.host);
        let enabled: Vec<&Script> = self.comp.scripts.iter().filter(|s| s.enabled).collect();
        host.retain(&enabled.iter().map(|s| s.name.as_str()).collect::<Vec<_>>());
        for script in &enabled {
            host.load(&script.name, &script.source);
        }
        let info = FrameInfo {
            time: self.scripts.time,
            dt,
            beats: self.controls.beats,
            bpm: self.comp.bpm,
            audio: self.controls.levels.bands(),
        };
        let commands = host.run(&self.comp, &info, &|p| self.param_range(&self.comp, p));
        self.scripts.host = host;
        for command in commands {
            self.send(command);
        }
    }

    pub(crate) fn scripts_window(&mut self, ctx: &egui::Context) {
        let mut open = self.scripts.window_open;
        egui::Window::new("Scripts")
            .open(&mut open)
            .default_size([720.0, 460.0])
            .default_pos([260.0, 140.0])
            .show(ctx, |ui| self.scripts_body(ui));
        self.scripts.window_open = open && self.scripts.window_open;
    }

    fn scripts_body(&mut self, ui: &mut egui::Ui) {
        let count = self.comp.scripts.len();
        self.scripts.selected = self.scripts.selected.min(count.saturating_sub(1));
        egui::Panel::left("script list")
            .resizable(true)
            .default_size(170.0)
            .show(ui, |ui| {
                if ui.button("+ New Script").clicked() {
                    self.send(Command::AddScript(Script {
                        name: "Script".into(),
                        source: TEMPLATE.into(),
                        enabled: true,
                    }));
                    self.scripts.selected = count;
                    self.scripts.draft = None;
                }
                ui.separator();
                for (index, script) in self.comp.scripts.clone().into_iter().enumerate() {
                    ui.horizontal(|ui| {
                        let mut enabled = script.enabled;
                        if ui.checkbox(&mut enabled, "").on_hover_text("Run").changed() {
                            self.send(Command::SetScript {
                                index,
                                script: Script {
                                    enabled,
                                    ..script.clone()
                                },
                            });
                        }
                        let failed =
                            self.scripts.host.error(&script.name).is_some() && script.enabled;
                        let label = egui::RichText::new(&script.name).color(if failed {
                            theme::WARN
                        } else {
                            theme::TEXT
                        });
                        if ui
                            .selectable_label(self.scripts.selected == index, label)
                            .clicked()
                        {
                            self.scripts.selected = index;
                            self.scripts.draft = None;
                        }
                    });
                }
            });

        let Some(script) = self.comp.scripts.get(self.scripts.selected).cloned() else {
            ui.label(
                egui::RichText::new(
                    "Scripts automate the composition in Lua. Create one to start.",
                )
                .color(theme::TEXT_DIM),
            );
            return;
        };
        let index = self.scripts.selected;
        if self.scripts.draft.as_ref().is_none_or(|(i, _)| *i != index) {
            self.scripts.draft = Some((index, script.source.clone()));
        }

        ui.horizontal(|ui| {
            let mut name = script.name.clone();
            if ui
                .add(egui::TextEdit::singleline(&mut name).desired_width(160.0))
                .changed()
            {
                self.send(Command::SetScript {
                    index,
                    script: Script {
                        name,
                        ..script.clone()
                    },
                });
            }
            let edited = self
                .scripts
                .draft
                .as_ref()
                .is_some_and(|(_, text)| *text != script.source);
            let apply = ui
                .add_enabled(edited, egui::Button::new("Apply"))
                .on_hover_text("Reload the script (Ctrl+Enter)");
            let shortcut = ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter));
            if (apply.clicked() || (shortcut && edited))
                && let Some((_, source)) = &self.scripts.draft
            {
                self.send(Command::SetScript {
                    index,
                    script: Script {
                        source: source.clone(),
                        ..script.clone()
                    },
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::square_button(ui, "X", "Delete script").clicked() {
                    self.send(Command::RemoveScript(index));
                    self.scripts.draft = None;
                }
            });
        });

        if let Some(error) = self
            .scripts
            .host
            .error(&script.name)
            .filter(|_| script.enabled)
        {
            ui.label(egui::RichText::new(error).color(theme::WARN).monospace());
        }

        let log = self.scripts.host.log(&script.name).join("\n");
        egui::Panel::bottom("script console")
            .resizable(true)
            .default_size(90.0)
            .show(ui, |ui| {
                widgets::header(ui, "Console");
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(log)
                                .small()
                                .monospace()
                                .color(theme::TEXT_DIM),
                        );
                    });
            });
        egui::CentralPanel::default_margins().show(ui, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                if let Some((_, text)) = &mut self.scripts.draft {
                    ui.add(
                        egui::TextEdit::multiline(text)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(18),
                    );
                }
            });
        });
    }
}
