//! Browser › Online: search ISF and ShaderToy and import shaders.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;
use verso_online::{Listing, Site, Usage, isf, shadertoy};
use verso_render::ShaderKind;

use crate::app::VersoApp;
use crate::theme;

/// ShaderToy results fetched per page (each needs its own request).
const SHADERTOY_PAGE: usize = 12;

/// Verso's per-user folder: `%APPDATA%\Verso`, `~/Library/Application Support/Verso`
/// or `$XDG_CONFIG_HOME/verso` (`~/.config/verso`).
pub fn user_dir() -> Option<PathBuf> {
    let env = |name: &str| std::env::var_os(name).map(PathBuf::from);
    if cfg!(windows) {
        env("APPDATA").map(|d| d.join("Verso"))
    } else if cfg!(target_os = "macos") {
        env("HOME").map(|d| d.join("Library/Application Support/Verso"))
    } else {
        env("XDG_CONFIG_HOME")
            .or_else(|| env("HOME").map(|h| h.join(".config")))
            .map(|d| d.join("verso"))
    }
}

/// Folder holding imported shaders; loaded at startup.
pub fn shaders_dir() -> Option<PathBuf> {
    user_dir().map(|d| d.join("shaders"))
}

fn key_file() -> Option<PathBuf> {
    user_dir().map(|d| d.join("shadertoy-key.txt"))
}

enum Event {
    Results {
        site: Site,
        listings: Vec<Listing>,
        total: usize,
        from: usize,
    },
    Failed(String),
}

pub struct OnlineState {
    site: Site,
    query: String,
    popular: bool,
    from: usize,
    total: usize,
    results: Vec<Listing>,
    /// Per result id: import outcome to show.
    imported: std::collections::HashMap<String, Result<String, String>>,
    busy: bool,
    /// Show only results whose code declares a license allowing paid use.
    commercial_only: bool,
    message: String,
    shadertoy_key: String,
    paste_name: String,
    paste_code: String,
    sender: Sender<Event>,
    receiver: Receiver<Event>,
}

impl Default for OnlineState {
    fn default() -> Self {
        let (sender, receiver) = channel();
        let shadertoy_key = key_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|k| k.trim().to_owned())
            .unwrap_or_default();
        Self {
            site: Site::Isf,
            query: String::new(),
            popular: true,
            from: 0,
            total: 0,
            results: Vec::new(),
            imported: Default::default(),
            busy: false,
            commercial_only: false,
            message: String::new(),
            shadertoy_key,
            paste_name: String::new(),
            paste_code: String::new(),
            sender,
            receiver,
        }
    }
}

impl OnlineState {
    fn search(&mut self, from: usize) {
        self.busy = true;
        self.message.clear();
        let (site, query, popular, key) = (
            self.site,
            self.query.clone(),
            self.popular,
            self.shadertoy_key.clone(),
        );
        let sender = self.sender.clone();
        thread::spawn(move || {
            let result = match site {
                Site::Isf => {
                    let sort = if popular {
                        isf::Sort::Popular
                    } else {
                        isf::Sort::Recent
                    };
                    isf::search(&query, sort, from)
                }
                Site::ShaderToy => {
                    shadertoy::search(&key, &query, from, SHADERTOY_PAGE).map(|(ids, total)| {
                        // Each result needs its own request for name and code.
                        let listings = ids
                            .iter()
                            .filter_map(|id| shadertoy::fetch(&key, id).ok())
                            .collect();
                        (listings, total)
                    })
                }
            };
            let _ = sender.send(match result {
                Ok((listings, total)) => Event::Results {
                    site,
                    listings,
                    total,
                    from,
                },
                Err(e) => Event::Failed(e.to_string()),
            });
        });
    }

    fn poll(&mut self) {
        for event in self.receiver.try_iter().collect::<Vec<_>>() {
            self.busy = false;
            match event {
                Event::Results {
                    site,
                    listings,
                    total,
                    from,
                } if site == self.site => {
                    self.message = if listings.is_empty() {
                        "No results.".into()
                    } else {
                        format!("{}–{} of {total}", from + 1, from + listings.len())
                    };
                    self.results = listings;
                    self.total = total;
                    self.from = from;
                }
                Event::Results { .. } => {}
                Event::Failed(e) => self.message = e,
            }
        }
    }
}

impl VersoApp {
    /// Saves a shader to the user shader folder and loads it. Returns a short
    /// result for the UI.
    fn import_shader(&mut self, file_name: &str, contents: &str) -> Result<String, String> {
        let dir = shaders_dir().ok_or("no user folder to save shaders in")?;
        let path = verso_online::save(&dir, file_name, contents).map_err(|e| e.to_string())?;
        let def = self.renderer.library_mut().load_file(&path).map_err(|e| {
            // Keep the first meaningful line of a compiler report.
            e.lines()
                .find(|l| l.contains("error") || l.contains("not supported"))
                .unwrap_or(e.lines().next().unwrap_or(""))
                .rsplit(": ")
                .next()
                .unwrap_or_default()
                .to_owned()
        });
        match def {
            Ok(def) => Ok(match def.kind {
                ShaderKind::Generator => format!("{} → Sources", def.name),
                ShaderKind::Filter => format!("{} → Effects", def.name),
            }),
            Err(e) => {
                // Do not leave an unusable file in the library folder.
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
        }
    }

    pub(crate) fn online_browser(&mut self, ui: &mut egui::Ui) {
        self.online.poll();
        let online = &mut self.online;
        ui.horizontal(|ui| {
            ui.selectable_value(&mut online.site, Site::Isf, "ISF");
            ui.selectable_value(&mut online.site, Site::ShaderToy, "ShaderToy");
        });

        if online.site == Site::ShaderToy {
            ui.horizontal(|ui| {
                ui.label("API key");
                let edit = ui.add(
                    egui::TextEdit::singleline(&mut online.shadertoy_key)
                        .password(true)
                        .desired_width(110.0),
                );
                if edit.lost_focus()
                    && let Some(file) = key_file()
                {
                    if let Some(dir) = file.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    let _ = std::fs::write(file, online.shadertoy_key.trim());
                }
                ui.hyperlink_to("Get a key", "https://www.shadertoy.com/myapps");
            });
            ui.label(
                egui::RichText::new(
                    "Only shaders published as Public+API. Single pass; iChannel0 is the input image. Respect each shader's license (default CC BY-NC-SA 3.0).",
                )
                .small()
                .color(theme::TEXT_DIM),
            );
        }

        let mut search = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut online.query)
                    .hint_text("Search shaders…")
                    .desired_width(ui.available_width() - 64.0),
            );
            search |= field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            search |= ui
                .add_enabled(!online.busy, egui::Button::new("Search"))
                .clicked();
        });
        if online.site == Site::Isf {
            ui.horizontal(|ui| {
                search |= ui
                    .selectable_value(&mut online.popular, true, "Popular")
                    .clicked();
                search |= ui
                    .selectable_value(&mut online.popular, false, "Recent")
                    .clicked();
            });
        }
        let needs_key = online.site == Site::ShaderToy && online.shadertoy_key.trim().is_empty();
        if search {
            if needs_key {
                online.message = "Enter your ShaderToy API key first.".into();
            } else {
                online.search(0);
            }
        }

        ui.horizontal(|ui| {
            if online.busy {
                ui.spinner();
            }
            ui.label(
                egui::RichText::new(&online.message)
                    .small()
                    .color(theme::TEXT_DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let page = match online.site {
                    Site::Isf => online.results.len().max(1),
                    Site::ShaderToy => SHADERTOY_PAGE,
                };
                let next = online.from + online.results.len() < online.total;
                if ui
                    .add_enabled(next && !online.busy, egui::Button::new("›"))
                    .clicked()
                {
                    let from = online.from + page;
                    online.search(from);
                }
                if ui
                    .add_enabled(online.from > 0 && !online.busy, egui::Button::new("‹"))
                    .clicked()
                {
                    let from = online.from.saturating_sub(page);
                    online.search(from);
                }
            });
        });

        ui.checkbox(&mut self.online.commercial_only, "Commercial use only")
            .on_hover_text(
                "Keeps shaders whose code declares a license allowing paid use (MIT, CC0, public domain…).                  ShaderToy shaders without a statement are CC BY-NC-SA. Detection reads comments only: check the shader page.",
            );
        let mut import = None;
        egui::ScrollArea::vertical()
            .max_height(ui.available_height() - 30.0)
            .show(ui, |ui| {
                let commercial_only = self.online.commercial_only;
                let shown = self
                    .online
                    .results
                    .iter()
                    .filter(|l| !commercial_only || l.usage.allows_commercial());
                for listing in shown {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.hyperlink_to(
                                egui::RichText::new(&listing.title).strong(),
                                &listing.url,
                            );
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&listing.author)
                                        .small()
                                        .color(theme::TEXT_DIM),
                                );
                                let color = match listing.usage {
                                    Usage::Commercial(_) => theme::ACCENT,
                                    Usage::NonCommercial(_) => theme::WARN,
                                    Usage::Unknown => theme::TEXT_DIM,
                                };
                                ui.label(
                                    egui::RichText::new(listing.usage.label())
                                        .small()
                                        .color(color),
                                );
                            });
                        });
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            match self.online.imported.get(&listing.id) {
                                Some(Ok(done)) => {
                                    ui.label(
                                        egui::RichText::new(done).small().color(theme::ACCENT),
                                    );
                                }
                                Some(Err(e)) => {
                                    ui.label(
                                        egui::RichText::new("Not supported")
                                            .small()
                                            .color(theme::WARN),
                                    )
                                    .on_hover_text(e);
                                }
                                None => {
                                    if ui
                                        .add_enabled(
                                            listing.contents.is_some(),
                                            egui::Button::new("Import"),
                                        )
                                        .clicked()
                                    {
                                        import = Some(listing.clone());
                                    }
                                }
                            }
                        });
                    });
                    ui.separator();
                }
            });
        if let Some(listing) = import {
            let result = self.import_shader(
                &listing.file_name(),
                listing.contents.as_deref().unwrap_or(""),
            );
            self.online.imported.insert(listing.id.clone(), result);
        }

        ui.collapsing("Paste ShaderToy code", |ui| {
            ui.add(egui::TextEdit::singleline(&mut self.online.paste_name).hint_text("Name"));
            ui.add(
                egui::TextEdit::multiline(&mut self.online.paste_code)
                    .code_editor()
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .hint_text("void mainImage(out vec4 fragColor, in vec2 fragCoord) { … }"),
            );
            let ready = !self.online.paste_name.trim().is_empty()
                && self.online.paste_code.contains("mainImage");
            if ui.add_enabled(ready, egui::Button::new("Import")).clicked() {
                let name = self.online.paste_name.trim().to_owned();
                let listing = Listing {
                    site: Site::ShaderToy,
                    id: format!("pasted{}", self.online.paste_code.len()),
                    title: name.clone(),
                    author: String::new(),
                    url: String::new(),
                    contents: None,
                    usage: Usage::Unknown,
                };
                let contents = format!("// name: {name}\n{}", self.online.paste_code);
                self.status = match self.import_shader(&listing.file_name(), &contents) {
                    Ok(done) => {
                        self.online.paste_code.clear();
                        format!("Imported {done}")
                    }
                    Err(e) => format!("Shader not supported: {e}"),
                };
            }
        });
    }
}
