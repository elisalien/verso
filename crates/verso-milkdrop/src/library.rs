//! Preset families found on disk.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::preset::{Family, Preset, Variant};

/// What the UI needs to list a family without touching its presets.
#[derive(Debug, Clone)]
pub struct FamilyInfo {
    /// Stable id: `<category>/<file stem>`.
    pub id: String,
    pub name: String,
    pub category: String,
    pub authors: Vec<String>,
    pub variants: Vec<String>,
    /// Preview image next to the family file, if any.
    pub preview: Option<PathBuf>,
}

#[derive(Debug, Default, Clone)]
pub struct MilkLibrary {
    families: BTreeMap<String, (FamilyInfo, Arc<Family>)>,
}

impl MilkLibrary {
    /// Loads every `.json` file under `dir`: preset families, or single
    /// Butterchurn presets (a family with one variant, category "User").
    /// Returns one message per file that failed.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<String> {
        let mut errors = Vec::new();
        let mut files = Vec::new();
        collect_json(dir, &mut files);
        for path in files {
            if let Err(e) = self.load_file(&path) {
                errors.push(format!("{}: {e}", path.display()));
            }
        }
        errors
    }

    pub fn load_file(&mut self, path: &Path) -> Result<&FamilyInfo, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("preset")
            .to_owned();
        let value: serde_json::Value =
            serde_json::from_str(&text).map_err(|e| format!("bad JSON: {e}"))?;
        let family = if value.get("variants").is_some() {
            Family::from_json(&text)?
        } else {
            Family {
                name: stem.clone(),
                category: "User".into(),
                authors: Vec::new(),
                source: String::new(),
                variants: vec![Variant {
                    name: stem.clone(),
                    preset: Preset::from_json(&text)?,
                }],
            }
        };
        if family.variants.is_empty() {
            return Err("family without variants".into());
        }
        let id = format!("{}/{stem}", family.category);
        let preview = ["jpg", "png"]
            .into_iter()
            .map(|ext| path.with_extension(ext))
            .find(|p| p.is_file());
        let info = FamilyInfo {
            id: id.clone(),
            name: family.name.clone(),
            category: family.category.clone(),
            authors: family.authors.clone(),
            variants: family.variants.iter().map(|v| v.name.clone()).collect(),
            preview,
        };
        self.families.insert(id.clone(), (info, Arc::new(family)));
        Ok(&self.families[&id].0)
    }

    pub fn get(&self, id: &str) -> Option<&Arc<Family>> {
        self.families.get(id).map(|(_, f)| f)
    }

    pub fn info(&self, id: &str) -> Option<&FamilyInfo> {
        self.families.get(id).map(|(i, _)| i)
    }

    /// Families sorted by category, then name.
    pub fn sorted(&self) -> Vec<&FamilyInfo> {
        let mut all: Vec<_> = self.families.values().map(|(i, _)| i).collect();
        all.sort_by(|a, b| {
            (a.category.as_str(), a.name.to_lowercase()).cmp(&(b.category.as_str(), b.name.to_lowercase()))
        });
        all
    }

    pub fn len(&self) -> usize {
        self.families.len()
    }

    pub fn is_empty(&self) -> bool {
        self.families.is_empty()
    }
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out);
        } else if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
            out.push(path);
        }
    }
}
