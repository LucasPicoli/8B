//! The `export` command: read every profile and save each as canonical JSON.
//!
//! Mirrors `runExport` and `ProfileExportService` in the C++ code. The files are what
//! `upload` takes back, so an export is also the restorable backup of a mode's slots.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use controller_core::error::ErrorCategory;
use controller_core::model::CanonicalProfileSummary;
use controller_core::orchestrator::profile::detect_and_read_all;
use controller_core::service::validation::validate_profile;
use controller_core::transport::HidrawDevice;

use crate::commands::{emit_json, error_category_label, mode_label};

/// `profile-<mode>-slot-<N>-index-<N>.json`, as the C++ export names it.
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
fn file_entry(profile_id: &str, path: &Path, value: &Value) -> (Value, bool) {
    let (passed, errors) = match validate_profile(value) {
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

/// Runs the `export` command.
///
/// # Returns
/// Process exit code.
pub fn run_export(output_dir: &Path, overwrite: bool) -> i32 {
    let out = HidrawDevice::open().ok().map(|dev| detect_and_read_all(&dev));
    let abs = std::path::absolute(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    let mut payload = json!({
        "output_directory": abs.display().to_string(),
        "overwrite": overwrite,
    });

    let (category, message, mode, files) = match out {
        None => {
            (ErrorCategory::ConnectionFailure, "failed to open device".to_owned(), None, vec![])
        }
        Some(r) if !r.success => (r.error_category, r.message, r.mode, vec![]),
        Some(r) => match write_files(&r.profiles, output_dir, overwrite) {
            Err(message) => (ErrorCategory::ExportFailure, message, r.mode, vec![]),
            Ok(written) => {
                let files: Vec<(Value, bool)> = r
                    .profiles
                    .iter()
                    .zip(&written)
                    .map(|(p, (path, value))| file_entry(&p.id, path, value))
                    .collect();
                let failed = files.iter().filter(|(_, ok)| !ok).count();
                let (category, message) = if failed == 0 {
                    (ErrorCategory::None, format!("Exported {} profile(s).", files.len()))
                } else {
                    (
                        ErrorCategory::ValidationFailure,
                        format!("{failed} exported profile(s) failed validation."),
                    )
                };
                (category, message, r.mode, files)
            }
        },
    };

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
