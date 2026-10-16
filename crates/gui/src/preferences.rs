//! The saved preferences: `settings.json` in `$XDG_CONFIG_HOME/8b`, else `~/.config/8b`.
//! It holds one key today, the interface language.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// The key that holds the language code, such as `pt_BR`. No key means the system default.
const LANGUAGE: &str = "language";

/// Where 8B keeps its preferences: `settings.json` in `$XDG_CONFIG_HOME/8b`, else in
/// `~/.config/8b`. An empty variable counts as unset, as the XDG spec says.
#[must_use]
pub fn settings_path(xdg_config_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let base = xdg_config_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".config")))
        .unwrap_or_else(|| PathBuf::from(".config"));
    base.join("8b/settings.json")
}

/// The saved language code, or `None` for the system default. A missing or broken file
/// also means the system default, and logs one line.
#[must_use]
pub fn saved_language(path: &Path) -> Option<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            log::info!("no {}: following the system language", path.display());
            return None;
        }
        Err(e) => {
            log::warn!("cannot read {}: {e}; following the system language", path.display());
            return None;
        }
    };
    match serde_json::from_str::<Map<String, Value>>(&text) {
        Ok(settings) => settings.get(LANGUAGE).and_then(Value::as_str).map(str::to_owned),
        Err(e) => {
            log::warn!("{} is broken: {e}; following the system language", path.display());
            None
        }
    }
}

/// Saves `code` as the language, or removes the key for `None`. Other keys stay. The
/// file is written beside itself and renamed over, so a crash leaves the old one whole.
///
/// # Errors
///
/// The folder cannot be made, or the file cannot be written or renamed.
pub fn save_language(path: &Path, code: Option<&str>) -> io::Result<()> {
    let mut settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Map<String, Value>>(&text).ok())
        .unwrap_or_default();
    match code {
        Some(code) => settings.insert(LANGUAGE.to_owned(), code.into()),
        None => settings.remove(LANGUAGE),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, serde_json::to_string_pretty(&settings).map_err(io::Error::other)?)?;
    std::fs::rename(&temp, path)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_path_follows_xdg_then_home() {
        let path = settings_path(Some("/x".into()), Some("/home/u".into()));
        assert_eq!(path, Path::new("/x/8b/settings.json"));
        let path = settings_path(Some("".into()), Some("/home/u".into()));
        assert_eq!(path, Path::new("/home/u/.config/8b/settings.json"));
    }

    #[test]
    fn a_missing_file_means_the_system_default() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(saved_language(&dir.path().join("settings.json")), None);
    }

    #[test]
    fn a_broken_file_means_the_system_default() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "{\"language\": ").unwrap();
        assert_eq!(saved_language(&path), None);
    }

    #[test]
    fn a_pick_survives_and_system_default_removes_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("8b/settings.json");
        save_language(&path, Some("pt_BR")).unwrap();
        assert_eq!(saved_language(&path).as_deref(), Some("pt_BR"));

        std::fs::write(&path, r#"{"language": "pt_BR", "other": 1}"#).unwrap();
        save_language(&path, None).unwrap();
        assert_eq!(saved_language(&path), None);
        let left: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(left, serde_json::json!({ "other": 1 }));
    }

    #[test]
    fn a_save_replaces_a_broken_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, "not json").unwrap();
        save_language(&path, Some("en")).unwrap();
        assert_eq!(saved_language(&path).as_deref(), Some("en"));
    }
}
