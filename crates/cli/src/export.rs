//! The `export` command: read every profile and save each as canonical JSON.
//!
//! The files are what `upload` takes back, so an export is also the restorable backup
//! of a mode's slots.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use controller_core::device::Model;
use controller_core::devices;
use controller_core::error::ErrorCategory;
use controller_core::model::CanonicalProfileSummary;
use controller_core::orchestrator::profile::detect_and_read_all;
use controller_core::service::validation::validate_profile;

use crate::commands::{emit_json, error_category_label, mode_label};

/// `profile-<mode>-slot-<N>-index-<N>.json`.
fn file_name(p: &CanonicalProfileSummary) -> String {
    format!("profile-{}-slot-{}-index-{}.json", p.mode, p.source_slot, p.source_profile_index)
}

/// Writes each profile into `dir`. Refuses before writing anything if a file exists
/// and `overwrite` is off.
///
/// # Errors
/// Returns the message for the first file that cannot be created or written.
fn write_files(
    profiles: &[CanonicalProfileSummary],
    dir: &Path,
    overwrite: bool,
) -> Result<Vec<(PathBuf, Value)>, String> {
    if profiles.is_empty() {
        return Err("No profiles available to export.".to_owned());
    }
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create output directory '{}': {e}", dir.display()))?;
    let paths: Vec<PathBuf> = profiles.iter().map(|p| dir.join(file_name(p))).collect();
    if let Some(existing) = paths.iter().find(|p| !overwrite && p.exists()) {
        return Err(format!(
            "Refusing to overwrite existing file '{}'. Use --overwrite to replace existing exports.",
            existing.display()
        ));
    }
    paths
        .into_iter()
        .zip(profiles)
        .map(|(path, p)| {
            let value = serde_json::to_value(&p.canonical).map_err(|e| e.to_string())?;
            let text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
            std::fs::write(&path, text + "\n")
                .map_err(|e| format!("Failed to write export file '{}': {e}", path.display()))?;
            Ok((path, value))
        })
        .collect()
}

/// One `files` entry: the path plus the validator's verdict on the exported JSON.
fn file_entry(model: &dyn Model, profile_id: &str, path: &Path, value: &Value) -> (Value, bool) {
    let (passed, errors) = match validate_profile(model, value) {
        Ok(v) => (v.valid, v.errors),
        Err(e) => {
            eprintln!("validation could not run: {e}");
            (false, vec![])
        }
    };
    let mut entry = json!({
        "profile_id": profile_id,
        "path": path.display().to_string(),
        "validation_passed": passed,
        "status": if passed { "ok" } else { "validation_failed" },
    });
    if !errors.is_empty() {
        let list: Vec<Value> =
            errors.iter().map(|e| json!({ "path": e.path, "reason": e.reason })).collect();
        if let Some(map) = entry.as_object_mut() {
            map.insert("validation_errors".to_owned(), Value::Array(list));
        }
    }
    (entry, passed)
}

/// What an export of `profiles` gave: its category, its message and one `files` entry
/// with its verdict per file. An empty slot, one whose profile has no id, is skipped:
/// its default profile is no backup and would not upload back.
fn export_held(
    model: &dyn Model,
    profiles: &[CanonicalProfileSummary],
    dir: &Path,
    overwrite: bool,
) -> (ErrorCategory, String, Vec<(Value, bool)>) {
    let held: Vec<CanonicalProfileSummary> =
        profiles.iter().filter(|p| !p.id.is_empty()).cloned().collect();
    if held.is_empty() && !profiles.is_empty() {
        return (ErrorCategory::None, "Every slot is empty: nothing to export.".to_owned(), vec![]);
    }
    let written = match write_files(&held, dir, overwrite) {
        Ok(written) => written,
        Err(message) => return (ErrorCategory::ExportFailure, message, vec![]),
    };
    let files: Vec<(Value, bool)> = held
        .iter()
        .zip(&written)
        .map(|(p, (path, value))| file_entry(model, &p.id, path, value))
        .collect();
    let failed = files.iter().filter(|(_, ok)| !ok).count();
    let skipped = profiles.len() - held.len();
    let message = format!("Exported {} profile(s), skipped {skipped} empty slot(s).", files.len());
    if failed == 0 {
        (ErrorCategory::None, message, files)
    } else {
        let message = format!("{failed} exported profile(s) failed validation.");
        (ErrorCategory::ValidationFailure, message, files)
    }
}

/// Runs the `export` command.
///
/// # Returns
/// Process exit code.
pub fn run_export(output_dir: &Path, overwrite: bool) -> i32 {
    let dev = devices::open(None);
    let read = detect_and_read_all(dev.as_ref());
    let abs = std::path::absolute(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    let mut payload = json!({
        "output_directory": abs.display().to_string(),
        "overwrite": overwrite,
    });

    let (category, message, files) = match dev.model() {
        _ if !read.success => (read.error_category, read.message, vec![]),
        Err(e) => (e.category(), e.to_string(), vec![]),
        Ok(model) => export_held(model, &read.profiles, output_dir, overwrite),
    };
    let mode = read.mode;

    let success = category == ErrorCategory::None;
    let failed = files.iter().filter(|(_, ok)| !ok).count();
    let code = category.exit_code();
    if let Some(map) = payload.as_object_mut() {
        map.insert("success".to_owned(), json!(success));
        map.insert("exit_code".to_owned(), json!(code));
        map.insert("mode".to_owned(), json!(mode_label(mode)));
        map.insert("message".to_owned(), json!(message));
        if !success {
            map.insert("error_category".to_owned(), json!(error_category_label(category)));
        }
        map.insert("profile_count".to_owned(), json!(files.len()));
        map.insert("profiles_succeeded".to_owned(), json!(files.len() - failed));
        map.insert("profiles_failed".to_owned(), json!(failed));
        map.insert("validation_passed".to_owned(), json!(!files.is_empty() && failed == 0));
        map.insert(
            "files".to_owned(),
            Value::Array(files.into_iter().map(|(entry, _)| entry).collect()),
        );
    }
    emit_json(&payload);
    code
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use controller_core::device::ProtocolCodec as _;
    use controller_core::devices::pro3::{Pro3, XINPUT};

    use super::*;

    fn summary(slot: u8, name: &str) -> CanonicalProfileSummary {
        let mut canonical = Pro3.default_profile(XINPUT);
        let id = if name.is_empty() { String::new() } else { format!("xinput-slot-{slot}") };
        canonical.id.clone_from(&id);
        canonical.name = name.to_owned();
        CanonicalProfileSummary {
            id,
            name: name.to_owned(),
            mode: XINPUT,
            source_slot: slot,
            source_profile_index: slot - 1,
            canonical,
        }
    }

    #[test]
    fn empty_slots_are_skipped_and_do_not_fail_the_export() {
        let dir = tempfile::tempdir().unwrap();
        let profiles = [summary(1, "Mine"), summary(2, ""), summary(3, "")];
        let (category, message, files) = export_held(&Pro3, &profiles, dir.path(), false);
        assert_eq!(category, ErrorCategory::None, "{message}");
        assert_eq!(files.len(), 1);
        assert!(message.contains("skipped 2 empty slot(s)"), "{message}");
        assert!(dir.path().join("profile-xinput-slot-1-index-0.json").exists());
        assert!(!dir.path().join("profile-xinput-slot-2-index-1.json").exists());

        let empty = tempfile::tempdir().unwrap();
        let (category, _, files) = export_held(&Pro3, &profiles[1..], empty.path(), false);
        assert_eq!((category, files.len()), (ErrorCategory::None, 0));
    }
}
