//! Pure JSON-builder functions and command handlers for the 8BitDo Pro 3 CLI.
//!
//! Each handler is thin: it opens the device, calls into `controller-core`, then
//! delegates to the corresponding `build_*_payload` function to build the JSON.
//! The `build_*` functions take only plain structs so they can be unit-tested
//! without a real device.

use std::fs;
use std::io::Write as _;
use std::path::Path;

use serde_json::{json, Value};

use controller_core::devices;
use controller_core::error::{Error, ErrorCategory};
use controller_core::model::macros::{macro_file_name, macro_to_json};
use controller_core::model::{DeviceReadiness, Mode, Slot};
use controller_core::orchestrator::profile::{detect_and_read_all, DetectAndReadResult};
use controller_core::service::read::{read_macros, MacroReadResult};

// ---------------------------------------------------------------------------
// Helper functions
// ---------------------------------------------------------------------------

/// Maps an [`ErrorCategory`] to its deterministic process exit code.
#[must_use]
pub const fn exit_code_for_category(c: ErrorCategory) -> i32 {
    c.exit_code()
}

/// Returns the stable machine-readable string label for an [`ErrorCategory`].
#[must_use]
pub const fn error_category_label(c: ErrorCategory) -> &'static str {
    c.label()
}

/// Returns the mode string, or `"unknown"` when `None`.
///
/// `detect` and `read` print "unknown" for an absent mode, never null.
#[must_use]
pub fn mode_label(m: Option<Mode>) -> String {
    m.map_or_else(|| "unknown".to_owned(), |mode| mode.to_string())
}

// ---------------------------------------------------------------------------
// JSON payload builders (device-free; unit-testable)
// ---------------------------------------------------------------------------

/// Builds the detect/readiness JSON payload and exit code.
#[must_use]
pub fn build_detect_payload(r: &DeviceReadiness) -> (Value, i32) {
    let exit_code = i32::from(!r.supported_device_connected);
    let payload = json!({
        "supported_device_connected": r.supported_device_connected,
        "mode": mode_label(r.mode),
        "active_slot_marker": r.active_slot_marker,
        "active_slot_marker_verified": r.active_slot_marker_verified,
        "vendor_id": r.vendor_id,
        "product_id": r.product_id,
        "sysfs_path": r.sysfs_path,
        "firmware_version": if r.firmware_version.is_empty() { "unknown" } else { &r.firmware_version },
        "message": r.message,
        "exit_code": exit_code,
    });
    (payload, exit_code)
}

/// Builds the read JSON payload and exit code.
///
/// `error_category` is inserted only when `!success`.
#[must_use]
pub fn build_read_payload(out: &DetectAndReadResult) -> (Value, i32) {
    let exit_code = if out.success { 0 } else { exit_code_for_category(out.error_category) };

    let profiles: Vec<Value> = out
        .profiles
        .iter()
        .map(|p| {
            json!({
                "id": p.id,
                "name": p.name,
                "mode": p.mode.to_string(),
                "source_slot": p.source_slot,
                "source_profile_index": p.source_profile_index,
                "status": "ok",
            })
        })
        .collect();

    let profile_count = profiles.len();

    // Build the object field by field to keep the documented key order.
    let mut map = serde_json::Map::new();
    map.insert("success".to_owned(), Value::Bool(out.success));
    map.insert("exit_code".to_owned(), Value::Number(exit_code.into()));
    map.insert("mode".to_owned(), Value::String(mode_label(out.mode)));
    if !out.success {
        map.insert(
            "error_category".to_owned(),
            Value::String(error_category_label(out.error_category).to_owned()),
        );
    }
    map.insert("message".to_owned(), Value::String(out.message.clone()));
    map.insert("profiles".to_owned(), Value::Array(profiles));
    map.insert("profile_count".to_owned(), Value::Number(profile_count.into()));

    (Value::Object(map), exit_code)
}

/// Builds the read-macro success JSON payload and exit code.
///
/// Emits one `macros` entry per macro slot of the model, `macro_slots` of them (4 on a
/// Pro 3); each active entry carries `trigger`/`name`/`step_count`/`repeat_count`/
/// `interval_ms`; inactive entries carry only `macro_slot` and `active: false`.
#[must_use]
pub fn build_read_macro_ok_payload(
    mode: Mode,
    slot: u8,
    res: &MacroReadResult,
    macro_slots: u8,
) -> (Value, i32) {
    let macros: Vec<Value> = (0..macro_slots)
        .map(|macro_slot| {
            let active_def = res.macros.iter().find(|d| d.macro_slot == Some(macro_slot));
            active_def.map_or_else(
                || {
                    json!({
                        "macro_slot": macro_slot,
                        "active": false,
                    })
                },
                |def| {
                    json!({
                        "macro_slot": macro_slot,
                        "active": true,
                        "trigger": def.trigger,
                        "name": def.name,
                        "step_count": def.steps.len(),
                        "repeat_count": def.repeat_count,
                        "interval_ms": def.interval_ms,
                    })
                },
            )
        })
        .collect();

    let message = format!("Read {} active macro(s) from {} slot {}", res.macros.len(), mode, slot);

    let mut map = serde_json::Map::new();
    map.insert("success".to_owned(), Value::Bool(true));
    map.insert("exit_code".to_owned(), Value::Number(0.into()));
    map.insert("mode".to_owned(), Value::String(mode.to_string()));
    map.insert("slot".to_owned(), Value::Number(slot.into()));
    map.insert("macros".to_owned(), Value::Array(macros));
    map.insert("message".to_owned(), Value::String(message));

    (Value::Object(map), 0)
}

/// Builds the read-macro failure JSON payload and exit code. The exit code is the
/// error category's, as for every other verb: an empty slot is a validation failure,
/// exit 4.
#[must_use]
pub fn build_read_macro_err_payload(mode: Mode, slot: u8, err: &Error) -> (Value, i32) {
    let exit_code = exit_code_for_category(err.category());

    let payload = json!({
        "success": false,
        "exit_code": exit_code,
        "mode": mode.to_string(),
        "slot": slot,
        "message": err.to_string(),
        "error_category": error_category_label(err.category()),
    });
    (payload, exit_code)
}

// ---------------------------------------------------------------------------
// Command handlers (open device, call core, emit JSON)
// ---------------------------------------------------------------------------

/// Emits compact JSON to stdout followed by a newline.
pub fn emit_json(payload: &Value) {
    // `Display` for `Value` produces compact JSON.
    println!("{payload}");
}

/// Runs the `detect` / `readiness` command.
///
/// # Returns
/// Process exit code.
pub fn run_detect() -> i32 {
    let dev = devices::open(None);

    let readiness = dev.detect_readiness().unwrap_or_default();
    let (payload, code) = build_detect_payload(&readiness);
    emit_json(&payload);
    code
}

/// Runs the `read` command.
///
/// # Returns
/// Process exit code.
pub fn run_read() -> i32 {
    let dev = devices::open(None);

    let out = detect_and_read_all(dev.as_ref());
    let (payload, code) = build_read_payload(&out);
    emit_json(&payload);
    code
}

/// Runs the `dump` command.
///
/// Writes one raw blob per mode to `output_dir/raw-blob-{i}.bin`.
/// Not JSON.
///
/// # Returns
/// Process exit code.
pub fn run_dump(output_dir: &str) -> i32 {
    let dev = devices::open(None);

    let out = detect_and_read_all(dev.as_ref());
    if !out.success {
        eprintln!("{}", out.message);
        return exit_code_for_category(out.error_category);
    }

    if let Err(e) = fs::create_dir_all(output_dir) {
        eprintln!("Failed to create output directory '{output_dir}': {e}");
        return 5; // export_failure
    }

    for (i, blob) in out.raw_blobs.iter().enumerate() {
        let filename = format!("raw-blob-{i}.bin");
        let file_path = Path::new(output_dir).join(&filename);
        match fs::write(&file_path, blob) {
            Ok(()) => {
                println!("Wrote {} ({} bytes)", file_path.display(), blob.len());
            }
            Err(e) => {
                eprintln!("Failed to write {}: {e}", file_path.display());
                // A file that fails to open is skipped; the others are still written.
            }
        }
    }

    0
}

/// Runs the `read-macro` command.
///
/// # Returns
/// Process exit code.
pub fn run_read_macro(mode: Mode, slot: u8, output_dir: Option<&str>) -> i32 {
    let slot_typed = match Slot::new(slot) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };

    let dev = devices::open(None);

    match read_macros(dev.as_ref(), mode, slot_typed) {
        Err(e) => {
            eprintln!("{e}");
            let (payload, code) = build_read_macro_err_payload(mode, slot, &e);
            emit_json(&payload);
            code
        }
        Ok(res) => {
            let model = match dev.model() {
                Ok(model) => model,
                Err(e) => {
                    eprintln!("{e}");
                    return e.category().exit_code();
                }
            };
            let macro_slots = model.description().map_or(0, |d| d.macro_slot_count);
            let (payload, code) = build_read_macro_ok_payload(mode, slot, &res, macro_slots);
            emit_json(&payload);

            if let Some(dir) = output_dir {
                if let Err(e) = fs::create_dir_all(dir) {
                    eprintln!("Failed to create output directory '{dir}': {e}");
                } else {
                    export_macros(&res, mode, slot, dir, &model.default_profile(mode).device);
                }
            }

            code
        }
    }
}

/// Exports each active macro in `res` to `<dir>/<mode>-slot<slot>-macro<m>-<name>.json`,
/// marked as a macro of `device`.
fn export_macros(res: &MacroReadResult, mode: Mode, slot: u8, dir: &str, device: &str) {
    for def in &res.macros {
        let filename = macro_file_name(mode, slot, def);
        let file_path = Path::new(dir).join(&filename);

        let json_val = macro_to_json(def, device);
        match serde_json::to_string_pretty(&json_val) {
            Ok(content) => {
                match fs::File::create(&file_path).and_then(|mut f| f.write_all(content.as_bytes()))
                {
                    Ok(()) => {}
                    Err(e) => eprintln!("Failed to write {}: {e}", file_path.display()),
                }
            }
            Err(e) => eprintln!("Failed to serialize macro: {e}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Unit tests (device-free — test the JSON builders with hand-built structs)
// ---------------------------------------------------------------------------
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use controller_core::model::{MacroDefinition, MacroStep};

    // -----------------------------------------------------------------------
    // detect builder
    // -----------------------------------------------------------------------

    #[test]
    fn detect_connected_device() {
        let r = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(XINPUT),
            active_slot_marker: "1".to_owned(),
            active_slot_marker_verified: true,
            vendor_id: "2dc8".to_owned(),
            product_id: "310b".to_owned(),
            sysfs_path: "/sys/bus/usb/devices/1-1".to_owned(),
            firmware_version: "1.04".to_owned(),
            message: "Supported device detected.".to_owned(),
        };
        let (payload, code) = build_detect_payload(&r);
        assert_eq!(code, 0);
        assert_eq!(payload["supported_device_connected"], true);
        assert_eq!(payload["mode"], "xinput");
        assert_eq!(payload["exit_code"], 0);
        assert_eq!(payload["active_slot_marker"], "1");
        assert_eq!(payload["active_slot_marker_verified"], true);
        assert_eq!(payload["firmware_version"], "1.04");
    }

    #[test]
    fn detect_no_device() {
        let r = DeviceReadiness {
            supported_device_connected: false,
            mode: None,
            ..DeviceReadiness::default()
        };
        let (payload, code) = build_detect_payload(&r);
        assert_eq!(code, 1);
        assert_eq!(payload["supported_device_connected"], false);
        assert_eq!(payload["mode"], "unknown");
        assert_eq!(payload["firmware_version"], "unknown");
        assert_eq!(payload["exit_code"], 1);
    }

    // -----------------------------------------------------------------------
    // read builder
    // -----------------------------------------------------------------------

    fn make_summary(
        id: &str,
        name: &str,
        mode: Mode,
        slot: u8,
    ) -> controller_core::model::CanonicalProfileSummary {
        use controller_core::device::ProtocolCodec as _;
        use controller_core::devices::pro3::Pro3;
        use controller_core::model::{CanonicalProfile, CanonicalProfileSummary};
        CanonicalProfileSummary {
            id: id.to_owned(),
            name: name.to_owned(),
            mode,
            source_slot: slot,
            source_profile_index: slot - 1,
            canonical: CanonicalProfile {
                id: id.to_owned(),
                name: name.to_owned(),
                button_mappings: Vec::new(),
                ..Pro3.default_profile(mode)
            },
        }
    }

    #[test]
    fn read_success_no_error_category() {
        let out = DetectAndReadResult {
            success: true,
            message: "Profiles read successfully.".to_owned(),
            error_category: ErrorCategory::None,
            mode: Some(XINPUT),
            product_id: "310b".to_owned(),
            active_slot_marker: "1".to_owned(),
            active_slot_marker_verified: true,
            profiles: vec![
                make_summary("id1", "Profile 1", XINPUT, 1),
                make_summary("", "", XINPUT, 2),
            ],
            raw_blobs: vec![],
        };
        let (payload, code) = build_read_payload(&out);
        assert_eq!(code, 0);
        assert_eq!(payload["success"], true);
        assert_eq!(payload["profile_count"], 2);
        assert!(
            payload.get("error_category").is_none(),
            "error_category must be absent on success"
        );
        let profiles = payload["profiles"].as_array().unwrap();
        assert_eq!(profiles[0]["status"], "ok");
        assert_eq!(profiles[1]["status"], "ok");
    }

    #[test]
    fn read_failure_has_error_category() {
        let out = DetectAndReadResult {
            success: false,
            message: "no device".to_owned(),
            error_category: ErrorCategory::ConnectionFailure,
            mode: None,
            product_id: String::new(),
            active_slot_marker: String::new(),
            active_slot_marker_verified: false,
            profiles: vec![],
            raw_blobs: vec![],
        };
        let (payload, code) = build_read_payload(&out);
        assert_eq!(code, 1);
        assert_eq!(payload["success"], false);
        assert_eq!(payload["error_category"], "connection_failure");
        assert_eq!(payload["mode"], "unknown");
    }

    // -----------------------------------------------------------------------
    // read-macro ok builder
    // -----------------------------------------------------------------------

    fn make_macro_def(macro_slot: u8) -> MacroDefinition {
        MacroDefinition {
            name: "TestMacro".to_owned(),
            mode: XINPUT,
            trigger: "l1".to_owned(),
            repeat_count: 1,
            interval_ms: 50,
            steps: vec![MacroStep::default(), MacroStep::default(), MacroStep::default()],
            macro_slot: Some(macro_slot),
        }
    }

    #[test]
    fn read_macro_ok_four_entry_array() {
        let res = MacroReadResult { macros: vec![make_macro_def(0)] };
        let (payload, code) = build_read_macro_ok_payload(XINPUT, 1, &res, 4);
        assert_eq!(code, 0);
        let macros = payload["macros"].as_array().unwrap();
        assert_eq!(macros.len(), 4, "must always emit 4 macro entries");
        assert_eq!(macros[0]["active"], true);
        assert_eq!(macros[0]["trigger"], "l1");
        assert_eq!(macros[0]["name"], "TestMacro");
        assert_eq!(macros[0]["step_count"], 3);
        assert_eq!(macros[1]["active"], false);
        assert!(macros[1].get("trigger").is_none(), "inactive entry must not have trigger field");
        assert_eq!(macros[2]["active"], false);
        assert_eq!(macros[3]["active"], false);
        let msg = payload["message"].as_str().unwrap();
        assert_eq!(msg, "Read 1 active macro(s) from xinput slot 1");
    }

    // -----------------------------------------------------------------------
    // read-macro error builder
    // -----------------------------------------------------------------------

    #[test]
    fn read_macro_err_no_active_profile_is_a_validation_failure() {
        let err = Error::Validation("no active profile in slot 1".to_owned());
        let (payload, code) = build_read_macro_err_payload(XINPUT, 1, &err);
        assert_eq!(code, 4, "an empty slot exits 4, as set and upload do");
        assert_eq!(payload["success"], false);
        assert_eq!(payload["error_category"], "validation_failure");
    }

    #[test]
    fn read_macro_err_connection_failure_is_exit_1() {
        let err = Error::NoDevice;
        let (payload, code) = build_read_macro_err_payload(XINPUT, 1, &err);
        assert_eq!(code, 1);
        assert_eq!(payload["error_category"], "connection_failure");
    }

    #[test]
    fn read_macro_err_timeout_is_exit_3() {
        let err = Error::Timeout;
        let (payload, code) = build_read_macro_err_payload(SWITCH, 2, &err);
        assert_eq!(code, 3);
        assert_eq!(payload["error_category"], "timeout");
    }

    #[test]
    fn mode_label_unknown_for_none() {
        assert_eq!(mode_label(None), "unknown");
        assert_eq!(mode_label(Some(XINPUT)), "xinput");
        assert_eq!(mode_label(Some(SWITCH)), "switch");
        assert_eq!(mode_label(Some(DINPUT)), "dinput");
    }

    #[test]
    fn error_category_labels() {
        assert_eq!(error_category_label(ErrorCategory::None), "none");
        assert_eq!(error_category_label(ErrorCategory::ConnectionFailure), "connection_failure");
        assert_eq!(error_category_label(ErrorCategory::Timeout), "timeout");
        assert_eq!(error_category_label(ErrorCategory::ExportFailure), "export_failure");
        assert_eq!(error_category_label(ErrorCategory::ValidationFailure), "validation_failure");
        assert_eq!(error_category_label(ErrorCategory::WriteFailure), "write_failure");
    }
}
