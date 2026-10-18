//! The `export` command: read every profile with its macros and save each as canonical
//! JSON.
//!
//! The files are what `upload` takes back, so an export is also the restorable backup
//! of a mode's slots. Each profile file holds its slot's macros in its `macros` list.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use controller_core::device::Model;
use controller_core::devices;
use controller_core::error::{ErrorCategory, Result};
use controller_core::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, Mode, ProfileReadResult,
};
use controller_core::orchestrator::profile::{detect_and_read_all, DetectAndReadResult};
use controller_core::service::read::{macro_summaries, read_macro_steps, MacroAt};
use controller_core::service::validation::{validate_profile, ValidationError};
use controller_core::transport::DeviceIo;

use crate::commands::{emit_json, error_category_label, mode_label};

/// `profile-<mode>-slot-<N>-index-<N>.json`.
fn file_name(p: &CanonicalProfileSummary) -> String {
    format!("profile-{}-slot-{}-index-{}.json", p.mode, p.source_slot, p.source_profile_index)
}

/// Writes `value` to `path` as pretty JSON with a final newline.
///
/// # Errors
/// Returns the message when `value` does not serialize or the file cannot be written.
pub fn write_json(path: &Path, value: &Value) -> std::result::Result<(), String> {
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text + "\n")
        .map_err(|e| format!("Failed to write export file '{}': {e}", path.display()))
}

/// Writes each file into `dir`. Refuses before writing anything if a file exists and
/// `overwrite` is off.
///
/// # Errors
/// Returns the message for the first file that exists, or that cannot be created or
/// written.
fn write_files(
    files: &[(PathBuf, Value)],
    dir: &Path,
    overwrite: bool,
) -> std::result::Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Failed to create output directory '{}': {e}", dir.display()))?;
    if let Some((existing, _)) = files.iter().find(|(p, _)| !overwrite && p.exists()) {
        return Err(format!(
            "Refusing to overwrite existing file '{}'. Use --overwrite to replace existing exports.",
            existing.display()
        ));
    }
    files.iter().try_for_each(|(path, value)| write_json(path, value))
}

/// The `files` entry of the profile `id` written to `path`: the path, its macro count
/// and the validator's verdict. Also gives whether it passed and how many of its macros
/// have an error.
fn file_entry(
    id: &str,
    path: &Path,
    macro_count: usize,
    verdict: Result<(bool, Vec<ValidationError>)>,
) -> (Value, bool, usize) {
    let (passed, errors) = verdict.unwrap_or_else(|e| {
        eprintln!("validation could not run: {e}");
        (false, vec![])
    });
    let mut bad: Vec<&str> = errors
        .iter()
        .filter_map(|e| e.path.strip_prefix("/macros/"))
        .map(|rest| rest.split('/').next().unwrap_or(rest))
        .collect();
    bad.sort_unstable();
    bad.dedup();
    let mut entry = json!({
        "profile_id": id,
        "path": path.display().to_string(),
        "macro_count": macro_count,
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
    (entry, passed, bad.len())
}

/// What an export gave: its category, its message, one entry with its verdict per
/// profile file, and the count of macros and of those with an error.
struct Exported {
    category: ErrorCategory,
    message: String,
    profiles: Vec<(Value, bool)>,
    macros: (usize, usize),
}

impl Exported {
    /// An export that wrote nothing.
    const fn nothing(category: ErrorCategory, message: String) -> Self {
        Self { category, message, profiles: Vec::new(), macros: (0, 0) }
    }
}

/// Exports `profiles`, each held one with its macros from `macros` in its `macros`
/// list. An empty slot, one whose profile has no id, is skipped: its default profile is
/// no backup and would not upload back.
fn export_held(
    model: &dyn Model,
    profiles: &[CanonicalProfileSummary],
    macros: &[MacroAt],
    dir: &Path,
    overwrite: bool,
) -> Exported {
    if profiles.is_empty() {
        let message = "No profiles available to export.".to_owned();
        return Exported::nothing(ErrorCategory::ExportFailure, message);
    }
    let held: Vec<&CanonicalProfileSummary> =
        profiles.iter().filter(|p| !p.id.is_empty()).collect();
    if held.is_empty() {
        let message = "Every slot is empty: nothing to export.".to_owned();
        return Exported::nothing(ErrorCategory::None, message);
    }
    let mut files = Vec::with_capacity(held.len());
    let mut counts = Vec::with_capacity(held.len());
    for p in &held {
        let own: Vec<MacroDefinition> = macros
            .iter()
            .filter(|(at, _)| *at == (p.mode, p.source_slot))
            .map(|(_, def)| def.clone())
            .collect();
        counts.push(own.len());
        let canonical =
            CanonicalProfile { macro_refs: Vec::new(), macros: Some(own), ..p.canonical.clone() };
        match serde_json::to_value(&canonical) {
            Ok(value) => files.push((dir.join(file_name(p)), value)),
            Err(e) => return Exported::nothing(ErrorCategory::ExportFailure, e.to_string()),
        }
    }
    if let Err(message) = write_files(&files, dir, overwrite) {
        return Exported::nothing(ErrorCategory::ExportFailure, message);
    }

    let mut macros_failed = 0;
    let entries: Vec<(Value, bool)> = held
        .iter()
        .zip(&files)
        .zip(&counts)
        .map(|((p, (path, value)), count)| {
            let verdict = validate_profile(model, value).map(|v| (v.valid, v.errors));
            let (entry, passed, bad) = file_entry(&p.id, path, *count, verdict);
            macros_failed += bad;
            (entry, passed)
        })
        .collect();

    let macro_count = counts.iter().sum();
    let failed = entries.iter().filter(|(_, ok)| !ok).count();
    let message = format!(
        "Exported {} profile(s) with {macro_count} macro(s), skipped {} empty slot(s).",
        held.len(),
        profiles.len() - held.len()
    );
    let category = if failed == 0 { ErrorCategory::None } else { ErrorCategory::ValidationFailure };
    let message =
        if failed == 0 { message } else { format!("{failed} exported file(s) failed validation.") };
    Exported { category, message, profiles: entries, macros: (macro_count, macros_failed) }
}

/// The macros of every held slot of `read`, with their steps, read in one session.
///
/// # Errors
/// Any error of [`read_macro_steps`].
fn held_macros(
    dev: &dyn DeviceIo,
    model: &dyn Model,
    read: &DetectAndReadResult,
) -> Result<Vec<MacroAt>> {
    let read = ProfileReadResult {
        profiles: read.profiles.clone(),
        raw_blobs: read.raw_blobs.clone(),
        ..Default::default()
    };
    let held = |at: &(Mode, u8)| {
        read.profiles.iter().any(|p| (p.mode, p.source_slot) == *at && !p.id.is_empty())
    };
    let macros: Vec<MacroAt> = macro_summaries(model, &read)
        .into_iter()
        .filter(|(at, _)| held(at))
        .flat_map(|(at, defs)| defs.into_iter().map(move |def| (at, def)))
        .collect();
    if macros.is_empty() {
        return Ok(macros);
    }
    read_macro_steps(dev, macros)
}

/// Reads `dev` and exports into `output_dir`.
///
/// # Returns
/// The `export` JSON and the process exit code.
fn export(dev: &dyn DeviceIo, output_dir: &Path, overwrite: bool) -> (Value, i32) {
    let read = detect_and_read_all(dev);
    let abs = std::path::absolute(output_dir).unwrap_or_else(|_| output_dir.to_path_buf());
    let mut payload = json!({
        "output_directory": abs.display().to_string(),
        "overwrite": overwrite,
    });

    let exported = match dev.model() {
        _ if !read.success => Exported::nothing(read.error_category, read.message.clone()),
        Err(e) => Exported::nothing(e.category(), e.to_string()),
        Ok(model) => match held_macros(dev, model, &read) {
            Ok(macros) => export_held(model, &read.profiles, &macros, output_dir, overwrite),
            Err(e) => Exported::nothing(e.category(), e.to_string()),
        },
    };
    let Exported { category, message, profiles, macros } = exported;

    let success = category == ErrorCategory::None;
    let failed = profiles.iter().filter(|(_, ok)| !ok).count();
    let (macro_count, macros_failed) = macros;
    let code = category.exit_code();
    if let Some(map) = payload.as_object_mut() {
        map.insert("success".to_owned(), json!(success));
        map.insert("exit_code".to_owned(), json!(code));
        map.insert("mode".to_owned(), json!(mode_label(read.mode)));
        map.insert("message".to_owned(), json!(message));
        if !success {
            map.insert("error_category".to_owned(), json!(error_category_label(category)));
        }
        map.insert("profile_count".to_owned(), json!(profiles.len()));
        map.insert("profiles_succeeded".to_owned(), json!(profiles.len() - failed));
        map.insert("profiles_failed".to_owned(), json!(failed));
        map.insert("macro_count".to_owned(), json!(macro_count));
        map.insert("macros_failed".to_owned(), json!(macros_failed));
        map.insert(
            "validation_passed".to_owned(),
            json!(!profiles.is_empty() && failed == 0 && macros_failed == 0),
        );
        let entries = profiles.into_iter().map(|(entry, _)| entry).collect();
        map.insert("files".to_owned(), Value::Array(entries));
    }
    (payload, code)
}

/// Runs the `export` command.
///
/// # Returns
/// Process exit code.
pub fn run_export(output_dir: &Path, overwrite: bool) -> i32 {
    let dev = devices::open(None);
    let (payload, code) = export(dev.as_ref(), output_dir, overwrite);
    emit_json(&payload);
    code
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::{Pro3, DINPUT, XINPUT};
    use controller_core::model::{
        CanonicalProfile, DeviceReadiness, MacroSlot, MacroStep, RawProfilePayload, Slot,
    };
    use controller_core::transport::MockDevice;

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
        let done = export_held(&Pro3, &profiles, &[], dir.path(), false);
        assert_eq!(done.category, ErrorCategory::None, "{}", done.message);
        assert_eq!(done.profiles.len(), 1);
        assert!(done.message.contains("skipped 2 empty slot(s)"), "{}", done.message);
        assert!(dir.path().join("profile-xinput-slot-1-index-0.json").exists());
        assert!(!dir.path().join("profile-xinput-slot-2-index-1.json").exists());

        let empty = tempfile::tempdir().unwrap();
        let done = export_held(&Pro3, &profiles[1..], &[], empty.path(), false);
        assert_eq!((done.category, done.profiles.len()), (ErrorCategory::None, 0));
    }

    /// A 3-step macro in `macro_slot` that `trigger` starts.
    fn three_steps(name: &str, mode: Mode, trigger: &str, macro_slot: u8) -> MacroDefinition {
        let step = |x| MacroStep {
            duration_ms: 40,
            pressed_buttons: vec!["bottom face".to_owned()],
            left_stick_x: x,
            ..MacroStep::default()
        };
        MacroDefinition {
            name: name.to_owned(),
            mode,
            trigger_buttons: vec![trigger.to_owned()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![step(0), step(127), step(255)],
            macro_slot: Some(macro_slot),
        }
    }

    /// A pad in `XInput` whose slot 1 holds a profile with one macro in `XInput` and
    /// two in `DInput`, and whose Switch bank is empty.
    fn pad() -> MockDevice {
        let xinput = [three_steps("Dash", XINPUT, "l1", 0)];
        let dinput = [three_steps("Combo", DINPUT, "r1", 1), three_steps("Spin", DINPUT, "l4", 3)];
        let slot1 = Slot::new(1).unwrap();
        let bank = |mode, macros: &[MacroDefinition]| {
            let profile = CanonicalProfile { name: "Pad".to_owned(), ..Pro3.default_profile(mode) };
            Pro3.compile_profile(&profile, slot1, &[], macros).unwrap()
        };
        let banks = [(XINPUT, bank(XINPUT, &xinput)), (DINPUT, bank(DINPUT, &dinput))];
        let empty = vec![0; Pro3.blob_size()];
        let raw_blobs = vec![banks[0].1.clone(), empty, banks[1].1.clone()];
        let mut profiles = Vec::new();
        for (mode, blob) in &banks {
            for slot in 1..=3 {
                let raw = RawProfilePayload {
                    payload: blob.clone(),
                    source_slot: slot,
                    source_profile_index: slot - 1,
                    mode_hint: *mode,
                };
                let mut p = Pro3.map_profile(&raw).unwrap();
                if slot > 1 {
                    p.id = String::new();
                }
                profiles.push(p);
            }
        }
        let readiness = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(XINPUT),
            ..DeviceReadiness::default()
        };
        let mut dev = MockDevice::new()
            .with_model(&Pro3)
            .with_readiness(readiness)
            .with_profiles(ProfileReadResult { profiles, raw_blobs, ..Default::default() });
        for def in xinput.iter().chain(&dinput) {
            let stream = Pro3.encode_macro_steps(&def.steps, def.mode).unwrap();
            let m = MacroSlot::new(def.macro_slot.unwrap()).unwrap();
            dev = dev.with_macro_stream(def.mode, slot1, m, stream);
        }
        dev
    }

    #[test]
    fn every_macro_is_embedded_in_its_profile_and_validates() {
        let dir = tempfile::tempdir().unwrap();
        let (payload, code) = export(&pad(), dir.path(), false);
        assert_eq!(code, 0, "{payload}");
        assert_eq!((&payload["macro_count"], &payload["macros_failed"]), (&json!(3), &json!(0)));
        assert!(payload.get("macro_files").is_none(), "{payload}");

        let mut macros = Vec::new();
        for entry in payload["files"].as_array().unwrap() {
            let text = std::fs::read_to_string(entry["path"].as_str().unwrap()).unwrap();
            let profile: Value = serde_json::from_str(&text).unwrap();
            assert!(validate_profile(&Pro3, &profile).unwrap().valid, "{profile}");
            assert_eq!(profile["macro_refs"], json!([]), "{profile}");
            let held = profile["macros"].as_array().unwrap();
            assert_eq!(entry["macro_count"], held.len(), "{entry}");
            for m in held {
                assert_eq!(m["steps"].as_array().unwrap().len(), 3, "the steps were read");
                macros.push((m["mode"].clone(), m["macro_slot"].clone(), m["name"].clone()));
            }
        }
        let want = [("xinput", 0, "Dash"), ("dinput", 1, "Combo"), ("dinput", 3, "Spin")];
        let want: Vec<(Value, Value, Value)> =
            want.iter().map(|(m, s, n)| (json!(m), json!(s), json!(n))).collect();
        assert_eq!(macros, want);
        let count = std::fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(count, 2, "one file per held slot, no macro files");

        let (_, code) = export(&pad(), dir.path(), false);
        assert_eq!(code, 5, "every file exists");
    }

    #[test]
    fn an_invalid_embedded_macro_counts_as_failed() {
        let dir = tempfile::tempdir().unwrap();
        let mut bad = three_steps("Dash", XINPUT, "l1", 0);
        bad.steps[0].pressed_buttons = vec!["nope".to_owned()];
        let at = ((XINPUT, 1), bad);
        let done = export_held(&Pro3, &[summary(1, "Mine")], &[at], dir.path(), false);
        assert_eq!(done.category, ErrorCategory::ValidationFailure, "{}", done.message);
        assert_eq!(done.macros, (1, 1));
    }
}
