//! Write verbs: upload, deactivate, remap and set.
//!
//! Each handler builds a [`ProfileWriteOrchestrator`] over the attached controller and
//! prints its [`WriteResult`] as JSON.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::Path;

use serde_json::{json, Value};

use controller_core::device::Model;
use controller_core::devices;
use controller_core::error::ErrorCategory;
use controller_core::model::{Mode, Slot, WriteResult};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo as _, HidrawDevice};

use crate::commands::{emit_json, error_category_label};

/// Exit code for a write the user declined or that needs `--force`.
///
/// A script must not read a refused write as a success, so this is not 0.
const EXIT_ABORTED: i32 = 2;

/// Builds the write JSON payload and exit code.
///
/// `extra` holds verb-specific string fields (remap adds `source` and `target`).
/// Rollback fields appear only on a failure that attempted a rollback.
#[must_use]
pub fn build_write_payload(r: &WriteResult, extra: &[(&str, &str)]) -> (Value, i32) {
    let exit_code = match (r.success, r.error_category) {
        (true, _) => 0,
        (false, ErrorCategory::None) => EXIT_ABORTED,
        (false, category) => category.exit_code(),
    };
    let mut payload = json!({
        "success": r.success,
        "exit_code": exit_code,
        "mode": r.mode.to_string(),
        "slot": r.slot,
        "message": r.message,
    });
    if let Some(map) = payload.as_object_mut() {
        if !r.profile_id.is_empty() {
            map.insert("profile_id".to_owned(), json!(r.profile_id));
        }
        for &(key, value) in extra {
            map.insert(key.to_owned(), json!(value));
        }
        if !r.success {
            map.insert("error_category".to_owned(), json!(error_category_label(r.error_category)));
            if r.rollback_attempted {
                map.insert("rollback_attempted".to_owned(), json!(true));
                map.insert("rollback_succeeded".to_owned(), json!(r.rollback_succeeded));
                if let Some(path) = &r.backup_file_path {
                    map.insert("backup_file".to_owned(), json!(path));
                }
            }
        }
    }
    (payload, exit_code)
}

/// `--force` overwrites. Otherwise a terminal asks, and piped stdin refuses.
fn confirm_policy(force: bool) -> ConfirmPolicy {
    if force {
        return ConfirmPolicy::Force;
    }
    if !std::io::stdin().is_terminal() {
        return ConfirmPolicy::Abort;
    }
    ConfirmPolicy::Ask(Box::new(|warning| {
        eprint!("{warning}\nOverwrite? [y/N] ");
        let _ = std::io::stderr().flush();
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).is_ok()
            && answer.trim().eq_ignore_ascii_case("y")
    }))
}

/// The model to write with: the one the pad names, or the only built-in model when the
/// pad cannot be asked. The second keeps the checks that need no device, such as a range
/// or a profile schema, ahead of the device error.
///
/// # Errors
/// Returns the pad's error when it names no model and more than one model is built in,
/// because the rules to check by are then unknown.
fn write_model(dev: &HidrawDevice) -> controller_core::error::Result<&'static dyn Model> {
    dev.model().or_else(|e| match devices::models() {
        [only] => Ok(*only),
        _ => Err(e),
    })
}

/// Runs `op` against the attached controller on `slot` of `mode` and prints the result.
///
/// A failed rollback saves its backup file in the current directory.
///
/// # Returns
/// Process exit code.
pub fn run_write(
    mode: Mode,
    slot: Slot,
    force: bool,
    extra: &[(&str, &str)],
    op: impl FnOnce(&ProfileWriteOrchestrator<'_>, &ConfirmPolicy) -> WriteResult,
) -> i32 {
    let Ok(dev) = HidrawDevice::open() else {
        eprintln!("failed to open device");
        return ErrorCategory::ConnectionFailure.exit_code();
    };
    let result = match write_model(&dev) {
        Ok(model) => {
            op(&ProfileWriteOrchestrator::new(&dev, model, Path::new(".")), &confirm_policy(force))
        }
        Err(e) => WriteResult::failure(mode, slot, e.category(), e.to_string()),
    };
    let (payload, code) = build_write_payload(&result, extra);
    emit_json(&payload);
    code
}

/// Runs the `upload` command: writes the profile JSON at `file` into `slot` of `mode`.
///
/// A missing, unreadable or non-JSON file is a validation failure, and nothing is sent.
///
/// # Returns
/// Process exit code.
pub fn run_upload(file: &Path, mode: Mode, slot: Slot, force: bool) -> i32 {
    let profile = std::fs::read_to_string(file)
        .map_err(|e| format!("Cannot read file {}: {e}", file.display()))
        .and_then(|text| {
            serde_json::from_str::<Value>(&text)
                .map_err(|e| format!("File {} is not JSON: {e}", file.display()))
        });
    match profile {
        Ok(profile) => run_write(mode, slot, force, &[], |o, policy| {
            o.upload_profile(&profile, mode, slot, policy)
        }),
        Err(message) => {
            eprintln!("{message}");
            let r = WriteResult::failure(mode, slot, ErrorCategory::ValidationFailure, message);
            let (payload, code) = build_write_payload(&r, &[]);
            emit_json(&payload);
            code
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};

    fn slot() -> Slot {
        Slot::new(2).unwrap()
    }

    #[test]
    fn success_has_no_error_fields() {
        let mut r = WriteResult::success(XINPUT, slot(), "Profile uploaded successfully.");
        r.profile_id = "p1".to_owned();
        let (payload, code) = build_write_payload(&r, &[]);
        assert_eq!(code, 0);
        assert_eq!(payload["success"], true);
        assert_eq!(payload["slot"], 2);
        assert_eq!(payload["profile_id"], "p1");
        assert!(payload.get("error_category").is_none());
        assert!(payload.get("rollback_attempted").is_none());
    }

    #[test]
    fn remap_carries_source_and_target() {
        let r = WriteResult::success(DINPUT, slot(), "Button remapped.");
        let (payload, _) = build_write_payload(&r, &[("source", "l4"), ("target", "a")]);
        assert_eq!(payload["source"], "l4");
        assert_eq!(payload["target"], "a");
        assert!(payload.get("profile_id").is_none());
    }

    #[test]
    fn aborted_write_exits_2() {
        let r = WriteResult::failure(XINPUT, slot(), ErrorCategory::None, "Use --force.");
        let (payload, code) = build_write_payload(&r, &[]);
        assert_eq!(code, 2);
        assert_eq!(payload["exit_code"], 2);
        assert_eq!(payload["error_category"], "none");
    }

    #[test]
    fn failed_rollback_reports_backup_file() {
        let mut r = WriteResult::failure(SWITCH, slot(), ErrorCategory::WriteFailure, "x");
        r.rollback_attempted = true;
        r.backup_file_path = Some("./backup.bin".to_owned());
        let (payload, code) = build_write_payload(&r, &[]);
        assert_eq!(code, 6);
        assert_eq!(payload["rollback_attempted"], true);
        assert_eq!(payload["rollback_succeeded"], false);
        assert_eq!(payload["backup_file"], "./backup.bin");
    }

    #[test]
    fn validation_failure_exits_4() {
        let r = WriteResult::failure(XINPUT, slot(), ErrorCategory::ValidationFailure, "x");
        assert_eq!(build_write_payload(&r, &[]).1, 4);
    }
}
