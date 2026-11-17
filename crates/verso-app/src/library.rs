//! Favorites and the local media library: folders the user adds once, browsed
//! with their subfolders. Both are kept in Verso's user folder, so they stay
//! from one session to the next.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use verso_core::Source;

use crate::grid::SourceTemplate;

fn favorites_file() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("favorites.ron"))
}

fn library_file() -> Option<PathBuf> {
    crate::online_ui::user_dir().map(|d| d.join("library.ron"))
}

/// Key of a source in the favorites: its file, shader, MilkDrop family or
/// built-in name.
pub fn source_key(template: &SourceTemplate) -> String {
    match &template.source {
        Source::Image { path } | Source::Video { path, .. } => {
            format!("file:{}", path.display())
        }
        Source::Shader(effect) => format!("shader:{}", effect.id),
        Source::Milkdrop { family, .. } => format!("milkdrop:{family}"),
        Source::Ndi { name } => format!("ndi:{name}"),
        Source::Spout { name } => format!("spout:{name}"),
        _ => format!("builtin:{}", template.name),
    }
}

/// Favorites key of the source of a clip in the grid: the key of the browser
/// item it came from, whatever the clip was renamed to.
pub fn clip_source_key(source: &Source) -> String {
    let builtin = SourceTemplate::all()
        .into_iter()
        .find(|t| match (&t.source, source) {
            (Source::Generator { kind: a, .. }, Source::Generator { kind: b, .. }) => a == b,
            (a, b) => std::mem::discriminant(a) == std::mem::discriminant(b),
        });
    source_key(&builtin.unwrap_or_else(|| SourceTemplate {
        name: String::new(),
        source: source.clone(),
    }))
}

/// Label of the menu item starring or unstarring something.
pub fn favorite_label(favorite: bool) -> &'static str {
    if favorite {
        "★ Remove from Favorites"
    } else {
        "☆ Add to Favorites"
    }
}

/// Key of an effect in the favorites.
pub fn effect_key(id: &str) -> String {
    format!("effect:{id}")
}

/// Sources, effects and files marked with a star.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct Favorites {
    keys: BTreeSet<String>,
}

impl Favorites {
    pub fn load() -> Self {
        crate::settings::load(favorites_file()).unwrap_or_default()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.keys.contains(key)
    }

    /// Every favorite key, sorted.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.keys.iter().map(String::as_str)
    }

    /// Adds or removes `key`, and saves.
    pub fn toggle(&mut self, key: &str) {
        if !self.keys.remove(key) {
            self.keys.insert(key.to_owned());
        }
        let _ = crate::settings::save(favorites_file(), self);
    }
}

/// What the library remembers between sessions.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Saved {
    roots: Vec<PathBuf>,
    current: Option<PathBuf>,
}

/// The browser's Library tab: root folders, the folder shown, and its
/// subfolders and media files.
#[derive(Default)]
pub struct Library {
    pub roots: Vec<PathBuf>,
    /// Folder shown, inside one of the roots.
    pub current: Option<PathBuf>,
    pub folders: Vec<PathBuf>,
    pub entries: Vec<SourceTemplate>,
}

impl Library {
    pub fn load() -> Self {
        let saved: Saved = crate::settings::load(library_file()).unwrap_or_default();
        let mut library = Library {
            roots: saved.roots,
            current: saved.current.filter(|c| c.is_dir()),
            ..Default::default()
        };
        if library.current.is_none() {
            library.current = library.roots.first().cloned();
        }
        library.refresh();
        library
    }

    fn save(&self) {
        let saved = Saved {
            roots: self.roots.clone(),
            current: self.current.clone(),
        };
        let _ = crate::settings::save(library_file(), &saved);
    }

    /// Adds `folder` as a library root, if new, and shows it.
    pub fn add_root(&mut self, folder: PathBuf) {
        if !self.roots.contains(&folder) {
            self.roots.push(folder.clone());
        }
        self.open(folder);
    }

    /// Removes a root; the shown folder moves to another root if it was in it.
    pub fn remove_root(&mut self, folder: &Path) {
        self.roots.retain(|r| r != folder);
        if self
            .current
            .as_deref()
            .is_some_and(|c| c.starts_with(folder))
        {
            self.current = self.roots.first().cloned();
        }
        self.refresh();
        self.save();
    }

    /// Shows `folder`.
    pub fn open(&mut self, folder: PathBuf) {
        self.current = Some(folder);
        self.refresh();
        self.save();
    }

    /// The root holding the shown folder.
    pub fn root(&self) -> Option<&PathBuf> {
        let current = self.current.as_ref()?;
        self.roots.iter().find(|r| current.starts_with(r))
    }

    /// Folders from the root down to the shown one, for the breadcrumb.
    pub fn trail(&self) -> Vec<PathBuf> {
        let (Some(root), Some(current)) = (self.root(), self.current.as_ref()) else {
            return self.current.iter().cloned().collect();
        };
        let mut trail: Vec<PathBuf> = current
            .ancestors()
            .take_while(|p| p.starts_with(root))
            .map(Path::to_path_buf)
            .collect();
        trail.reverse();
        trail
    }

    /// Lists the subfolders and supported media files of the shown folder,
    /// sorted by name.
    pub fn refresh(&mut self) {
        self.folders.clear();
        self.entries.clear();
        let Some(folder) = &self.current else { return };
        let mut paths: Vec<_> = std::fs::read_dir(folder)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        paths.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
        for path in paths {
            if path.is_dir() {
                self.folders.push(path);
            } else if let Some(template) = SourceTemplate::from_path(&path) {
                self.entries.push(template);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_lists_subfolders_and_media_and_navigates() {
        let dir = std::env::temp_dir().join(format!("verso-library-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Loops/Abstract")).unwrap();
        std::fs::write(dir.join("a.png"), b"").unwrap();
        std::fs::write(dir.join("notes.txt"), b"").unwrap();
        std::fs::write(dir.join("Loops/b.mp4"), b"").unwrap();
        let mut library = Library {
            roots: vec![dir.clone()],
            current: Some(dir.clone()),
            ..Default::default()
        };
        library.refresh();
        assert_eq!(library.folders, [dir.join("Loops")]);
        assert_eq!(
            library.entries.len(),
            1,
            "only the image, not the text file"
        );
        library.current = Some(dir.join("Loops/Abstract"));
        library.refresh();
        assert_eq!(
            library.trail(),
            [dir.clone(), dir.join("Loops"), dir.join("Loops/Abstract")]
        );
        assert_eq!(library.root(), Some(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn favorite_keys_name_what_they_mark() {
        let file = SourceTemplate::from_path(Path::new("/clips/loop.mov")).unwrap();
        assert!(source_key(&file).starts_with("file:"));
        assert_eq!(effect_key("blur"), "effect:blur");
        // A clip keeps the key of its browser item, even renamed or edited.
        assert_eq!(clip_source_key(&file.source), source_key(&file));
        for template in SourceTemplate::all() {
            let mut source = template.source.clone();
            if let Source::Generator { speed, .. } = &mut source {
                *speed = 3.0;
            }
            assert_eq!(clip_source_key(&source), source_key(&template));
        }
        let mut favorites = Favorites::default();
        favorites.keys.insert("effect:blur".into());
        assert!(favorites.contains("effect:blur"));
    }
}
