//! Reading and writing the small settings files of the user folder
//! (layout, palette, library, favorites…), all in RON.

use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;

/// Reads `file`; `None` when it is missing or cannot be parsed.
pub fn load<T: DeserializeOwned>(file: Option<PathBuf>) -> Option<T> {
    let text = std::fs::read_to_string(file?).ok()?;
    ron::from_str(&text).ok()
}

/// Writes `value` to `file`, creating its folder. The text goes to a
/// temporary file that then replaces `file`, so a crash while writing leaves
/// the previous settings instead of a truncated file read back as defaults.
pub fn save<T: Serialize>(file: Option<PathBuf>, value: &T) -> std::io::Result<()> {
    let file = file.ok_or_else(|| std::io::Error::other("no user folder"))?;
    let text = ron::to_string(value).map_err(std::io::Error::other)?;
    if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = file.clone().into_os_string();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_and_replace() {
        let dir = std::env::temp_dir().join(format!("verso-settings-{}", std::process::id()));
        let file = dir.join("sub").join("a.ron");
        assert_eq!(load::<Vec<u32>>(Some(file.clone())), None);
        save(Some(file.clone()), &vec![1u32, 2]).unwrap();
        save(Some(file.clone()), &vec![3u32]).unwrap();
        assert_eq!(load::<Vec<u32>>(Some(file.clone())), Some(vec![3]));
        assert!(!dir.join("sub").join("a.ron.tmp").exists());
        assert!(save(None, &0u32).is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
