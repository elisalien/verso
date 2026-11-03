//! User preferences, saved as `settings.ron` in Verso's user folder.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const RECENT: usize = 10;

/// What a plain click on a clip cell does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CellClick {
    /// Click launches (and selects); Ctrl/Cmd+click only selects.
    #[default]
    Launch,
    /// Click selects; double-click launches.
    Select,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub recent: Vec<PathBuf>,
    pub cell_click: CellClick,
    /// Interface zoom, 1 is normal.
    pub ui_scale: f32,
    /// Saves a recovery copy of unsaved work every minute.
    pub autosave: bool,
    /// Opens the last composition at startup.
    pub reopen_last: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            recent: Vec::new(),
            cell_click: CellClick::default(),
            ui_scale: 1.0,
            autosave: true,
            reopen_last: false,
        }
    }
}

fn path() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("settings.ron"))
}

impl Prefs {
    pub fn load() -> Self {
        path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|text| ron::from_str::<Prefs>(&text).ok())
            .map(|mut p| {
                p.ui_scale = if p.ui_scale.is_finite() {
                    p.ui_scale.clamp(0.75, 2.0)
                } else {
                    1.0
                };
                p
            })
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = ron::ser::to_string_pretty(self, Default::default()) {
            let _ = crate::app::write_atomic(&path, text.as_bytes());
        }
    }

    /// Puts `file` first in the recent list.
    pub fn add_recent(&mut self, file: &Path) {
        self.recent.retain(|p| p != file);
        self.recent.insert(0, file.to_owned());
        self.recent.truncate(RECENT);
        self.save();
    }
}

/// Recovery copy of unsaved work, and a marker telling whether the last run
/// ended cleanly.
pub mod recovery {
    use std::path::PathBuf;

    fn dir() -> Option<PathBuf> {
        crate::online_ui::user_dir()
    }

    /// `name` in the user folder, tagged for an MCP test instance.
    fn file(name: &str, extension: &str) -> Option<PathBuf> {
        let name = match crate::logging::instance_tag() {
            Some(tag) => format!("{name}-{tag}{extension}"),
            None => format!("{name}{extension}"),
        };
        dir().map(|d| d.join(name))
    }

    pub fn autosave_path() -> Option<PathBuf> {
        file("autosave", ".verso")
    }

    fn running_path() -> Option<PathBuf> {
        file("running", "")
    }

    /// Marks this run as started. Returns whether the previous run did not
    /// end cleanly while it had unsaved work, i.e. there is something to
    /// recover.
    pub fn start() -> bool {
        let (Some(running), Some(autosave)) = (running_path(), autosave_path()) else {
            return false;
        };
        let crashed = running.exists() && autosave.exists();
        if let Some(dir) = running.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&running, std::process::id().to_string());
        crashed
    }

    /// Marks a clean exit and drops the recovery copy.
    pub fn finish() {
        if let Some(p) = running_path() {
            let _ = std::fs::remove_file(p);
        }
        discard();
    }

    pub fn discard() {
        if let Some(p) = autosave_path() {
            let _ = std::fs::remove_file(p);
        }
    }
}
