//! Browser › Online: search ISF and ShaderToy and import shaders.

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;

use eframe::egui;
use verso_online::{Listing, Site, Usage, isf, shadertoy};
use verso_render::ShaderKind;

use verso_core::Source;

use crate::app::VersoApp;
use crate::grid::SourceTemplate;
use crate::source_browser::ThumbJob;
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

/// Writes a secret readable by the current user only (on Unix; on Windows
/// the per-user folder is already private).
fn write_private(path: &std::path::Path, contents: &str) {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
        {
            let _ = file.write_all(contents.as_bytes());
        }
        // Files written by older versions keep their permissions otherwise.
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    {
        let _ = std::fs::write(path, contents);
    }
}

/// Stable id for pasted code, so two pastes only share a file when their code
/// is the same.
fn content_hash(text: &str) -> u64 {
    // FNV-1a: stable across runs and Rust versions, unlike `DefaultHasher`.
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
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
    /// A result's picture, saved in the cache: (result id, file).
    Thumb(String, PathBuf),
}

/// A shader imported from a result: what it became in the library.
#[derive(Debug, Clone)]
pub struct Imported {
    /// Where it went, for the UI ("Name → Sources").
    pub label: String,
    pub id: String,
    pub name: String,
    pub kind: ShaderKind,
}

pub struct OnlineState {
    site: Site,
    query: String,
    popular: bool,
    from: usize,
    total: usize,
    results: Vec<Listing>,
    /// Per result id: import outcome to show.
    imported: std::collections::HashMap<String, Result<Imported, String>>,
    /// Result pictures downloaded so far, by result id.
    thumbs: std::collections::HashMap<String, PathBuf>,
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
            thumbs: Default::default(),
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

    /// Downloads the pictures of `listings` into the cache, in the background.
    fn fetch_thumbs(&self, listings: &[Listing]) {
        let Some(dir) = user_dir().map(|d| d.join("online-thumbs")) else {
            return;
        };
        let wanted: Vec<(String, String)> = listings
            .iter()
            .filter(|l| !self.thumbs.contains_key(&l.id))
            .filter_map(|l| Some((l.id.clone(), l.thumbnail.clone()?)))
            .collect();
        if wanted.is_empty() {
            return;
        }
        let sender = self.sender.clone();
        thread::spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            for (id, url) in wanted {
                // Ids name the file: keep only safe characters.
                let safe: String = id
                    .chars()
                    .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
                    .collect();
                let path = dir.join(format!("{safe}.jpg"));
                if !path.is_file() {
                    match verso_online::download(&url) {
                        Ok(bytes) if std::fs::write(&path, &bytes).is_ok() => {}
                        _ => continue,
                    }
                }
                if sender.send(Event::Thumb(id, path)).is_err() {
                    return;
                }
            }
        });
    }

    fn poll(&mut self) {
        for event in self.receiver.try_iter().collect::<Vec<_>>() {
            if let Event::Thumb(id, path) = event {
                self.thumbs.insert(id, path);
                continue;
            }
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
                    self.fetch_thumbs(&listings);
                    self.results = listings;
                    self.total = total;
                    self.from = from;
                }
                Event::Results { .. } => {}
                Event::Failed(e) => self.message = e,
                Event::Thumb(..) => {}
            }
        }
    }
}

impl VersoApp {
    /// Saves a shader to the user shader folder and loads it. Returns a short
    /// result for the UI.
    fn import_shader(&mut self, file_name: &str, contents: &str) -> Result<Imported, String> {
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
            Ok(def) => Ok(Imported {
                label: match def.kind {
                    ShaderKind::Generator => format!("{} → Sources", def.name),
                    ShaderKind::Filter => format!("{} → Effects", def.name),
                },
                id: def.id.clone(),
                name: def.name.clone(),
                kind: def.kind,
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
                    write_private(&file, online.shadertoy_key.trim());
                }
                ui.hyperlink_to("Get a key", "https://www.shadertoy.com/myapps");
            });
            ui.label(
                egui::RichText::new(
                    "Only shaders published as Public+API. Single pass; iChannel0 is the input image. Respect each shader's license (default CC BY-NC-SA 3.0).",
                )
                .small()
                .color(theme::text_dim()),
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
                    .color(theme::text_dim()),
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
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.files.thumbnails, false, "List");
            ui.selectable_value(&mut self.files.thumbnails, true, "Thumbnails");
        });
        let commercial_only = self.online.commercial_only;
        let shown: Vec<Listing> = self
            .online
            .results
            .iter()
            .filter(|l| !commercial_only || l.usage.allows_commercial())
            .cloned()
            .collect();
        let thumbnails = self.files.thumbnails;
        egui::ScrollArea::vertical()
            .id_salt("online results")
            .max_height(ui.available_height() - 30.0)
            .show(ui, |ui| {
                if thumbnails {
                    let (columns, width) = crate::source_browser::tile_grid(ui);
                    for row in shown.chunks(columns) {
                        ui.horizontal(|ui| {
                            for listing in row {
                                ui.vertical(|ui| {
                                    ui.set_width(width);
                                    if self.online_item(ui, listing, Some(width)) {
                                        import = Some(listing.clone());
                                    }
                                });
                            }
                        });
                    }
                } else {
                    for listing in &shown {
                        if self.online_item(ui, listing, None) {
                            import = Some(listing.clone());
                        }
                        ui.separator();
                    }
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
                    id: format!("pasted{:016x}", content_hash(&self.online.paste_code)),
                    title: name.clone(),
                    author: String::new(),
                    url: String::new(),
                    contents: None,
                    usage: Usage::Unknown,
                    thumbnail: None,
                };
                let contents = format!("// name: {name}\n{}", self.online.paste_code);
                self.status = match self.import_shader(&listing.file_name(), &contents) {
                    Ok(done) => {
                        self.online.paste_code.clear();
                        format!("Imported {}", done.label)
                    }
                    Err(e) => format!("Shader not supported: {e}"),
                };
            }
        });
    }
    /// One search result, as a row or (with `tile_width`) a tile. Once
    /// imported, the shader renders live in its picture and drags onto a
    /// clip slot (generators) or a clip, layer or composition (effects).
    /// Returns whether its import was asked for.
    fn online_item(
        &mut self,
        ui: &mut egui::Ui,
        listing: &Listing,
        tile_width: Option<f32>,
    ) -> bool {
        let imported = self.online.imported.get(&listing.id).cloned();
        // Live render once imported, else the site's picture.
        let picture = match &imported {
            Some(Ok(done)) => match done.kind {
                ShaderKind::Generator => {
                    let source = Source::Shader(verso_core::Effect::new(&done.id, &done.name));
                    let key = format!("shader:{}", done.id);
                    if !self.browser_thumbs.textures.contains_key(&key)
                        || !self.files.still_thumbnails
                    {
                        self.browser_thumbs
                            .wanted
                            .push((key.clone(), ThumbJob::Source(source)));
                    }
                    self.browser_thumbs
                        .textures
                        .get(&key)
                        .map(|&t| (t, 16.0 / 9.0))
                }
                ShaderKind::Filter => {
                    let key = crate::source_browser::effect_thumb_key(&done.id);
                    if !self.browser_thumbs.textures.contains_key(&key)
                        || !self.files.still_thumbnails
                    {
                        self.browser_thumbs.wanted.push((
                            key,
                            ThumbJob::Effect(verso_core::Effect::new(&done.id, &done.name)),
                        ));
                    }
                    self.effect_tile_texture(&done.id)
                }
            },
            _ => None,
        }
        .or_else(|| {
            let path = self.online.thumbs.get(&listing.id).cloned()?;
            self.media
                .file_thumbnail(&Source::Image { path })
                .map(|(t, (w, h))| (t, w as f32 / h.max(1) as f32))
        });
        let mut import = false;
        let status = |ui: &mut egui::Ui, import: &mut bool| match &imported {
            Some(Ok(done)) => {
                ui.label(
                    egui::RichText::new(format!("{} · drag it", done.label))
                        .small()
                        .color(theme::accent()),
                );
            }
            Some(Err(e)) => {
                ui.label(
                    egui::RichText::new("Not supported")
                        .small()
                        .color(theme::warn()),
                )
                .on_hover_text(e);
            }
            None => {
                *import |= ui
                    .add_enabled(listing.contents.is_some(), egui::Button::new("Import"))
                    .clicked();
            }
        };
        let response = match tile_width {
            Some(width) => {
                let response =
                    self.online_picture(ui, listing, picture, imported.as_ref(), Some(width));
                ui.horizontal(|ui| status(ui, &mut import));
                response
            }
            None => {
                ui.horizontal(|ui| {
                    let response =
                        self.online_picture(ui, listing, picture, imported.as_ref(), None);
                    ui.vertical(|ui| {
                        ui.hyperlink_to(egui::RichText::new(&listing.title).strong(), &listing.url);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(&listing.author)
                                    .small()
                                    .color(theme::text_dim()),
                            );
                            let color = match listing.usage {
                                Usage::Commercial(_) => theme::accent(),
                                Usage::NonCommercial(_) => theme::warn(),
                                Usage::Unknown => theme::text_dim(),
                            };
                            ui.label(
                                egui::RichText::new(listing.usage.label())
                                    .small()
                                    .color(color),
                            );
                        });
                        status(ui, &mut import);
                    });
                    response
                })
                .inner
            }
        };
        if imported.is_none() && listing.contents.is_some() && response.double_clicked() {
            import = true;
        }
        import
    }

    /// A result's picture; draggable once imported.
    fn online_picture(
        &mut self,
        ui: &mut egui::Ui,
        listing: &Listing,
        picture: Option<(egui::TextureId, f32)>,
        imported: Option<&Result<Imported, String>>,
        tile_width: Option<f32>,
    ) -> egui::Response {
        let done = imported.and_then(|r| r.as_ref().ok());
        let paint = |ui: &mut egui::Ui| match tile_width {
            Some(width) => crate::source_browser::paint_tile(
                ui,
                width,
                &crate::source_browser::TileLook {
                    name: &listing.title,
                    texture: picture,
                    accent: theme::accent(),
                    outlined: done.is_some(),
                    favorite: None,
                    badge: None,
                },
            ),
            None => {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(96.0, 54.0), egui::Sense::click());
                ui.painter().rect_filled(rect, 2.0, theme::bg_deep());
                if let Some((texture, _)) = picture {
                    ui.painter().image(
                        texture,
                        rect,
                        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                }
                if done.is_some() || response.contains_pointer() {
                    let width = if done.is_some() { 2.0 } else { 1.0 };
                    ui.painter().rect_stroke(
                        rect,
                        2.0,
                        egui::Stroke::new(width, theme::accent()),
                        egui::StrokeKind::Inside,
                    );
                }
                response
            }
        };
        let id = egui::Id::unique(("verso-online", &listing.id));
        let response = match done {
            Some(done) if done.kind == ShaderKind::Generator => {
                let template = SourceTemplate {
                    name: done.name.clone(),
                    source: Source::Shader(verso_core::Effect::new(&done.id, &done.name)),
                };
                let response = ui.dnd_drag_source(id, template.clone(), paint).inner;
                self.browser_item_events(response.clone(), egui::Rect::NOTHING, template, "");
                response
            }
            Some(done) => {
                let template = crate::effects_ui::EffectTemplate {
                    id: done.id.clone(),
                    name: done.name.clone(),
                };
                ui.dnd_drag_source(id, template, paint).inner
            }
            None => paint(ui),
        };
        response.on_hover_text(match done {
            Some(done) if done.kind == ShaderKind::Generator => {
                format!("{}\nDrag onto a clip slot", listing.url)
            }
            Some(_) => format!(
                "{}\nDrag onto a clip, a layer header or the Composition header",
                listing.url
            ),
            None => format!("{}\nDouble-click to import", listing.url),
        })
    }
}
