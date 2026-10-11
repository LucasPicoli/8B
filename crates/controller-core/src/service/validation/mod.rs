//! Two-phase validation (JSON Schema + semantic rules).
//!
//! Validators are pure: they consume canonical JSON (`serde_json::Value`) and
//! perform no I/O. Macro-ref file-existence resolution is deferred to the
//! orchestrator layer.

pub mod macros;
pub mod profile;

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

use crate::description::{Button, ControllerDescription};
use crate::error::{Error, Result};

pub use macros::validate_macro;
pub use profile::{validate_all_profiles, validate_profile};

/// A single validation failure: a JSON-Pointer `path` and a human `reason`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidationError {
    /// JSON Pointer to the offending location (e.g. `/steps/0/actions/buttons`).
    pub path: String,
    /// Human-readable failure reason.
    pub reason: String,
}

/// Per-profile validation outcome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProfileValidationResult {
    /// The profile's `id` (empty string if absent).
    pub profile_id: String,
    /// Whether the profile passed both phases.
    pub valid: bool,
    /// All accumulated errors (empty iff `valid`).
    pub errors: Vec<ValidationError>,
}

/// Batch validation outcome.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidationSummary {
    /// One result per input profile, in order.
    pub results: Vec<ProfileValidationResult>,
    /// `true` iff every profile is valid.
    pub all_valid: bool,
}

/// The embedded macro schema (Draft 2020-12), compiled once.
// Used by macro_validator().
const MACRO_SCHEMA_JSON: &str = include_str!("../../../../../schemas/macro-v1.schema.json");
/// The embedded profile schema (Draft 2020-12), compiled once.
// Used by profile_validator().
const PROFILE_SCHEMA_JSON: &str = include_str!("../../../../../schemas/profile-v1.schema.json");

/// Compiles the macro schema for `description` and the profile `device` string of its
/// model: the schema file accepts any non-empty button name, and this fills the trigger
/// and step button lists from the flags `can_start_macro` and `can_be_in_macro_step`,
/// and pins `device` to `device`.
///
/// Compiled on each call: only macro validation uses it, and that is rare.
///
/// # Errors
/// Returns [`Error::Decode`] if the embedded schema fails to parse or compile, or if
/// the description has no button for one of the two lists.
pub(crate) fn macro_validator(
    description: &ControllerDescription,
    device: &str,
) -> Result<Validator> {
    let mut schema: Value = serde_json::from_str(MACRO_SCHEMA_JSON)
        .map_err(|e| Error::Decode(format!("macro schema compile failed: {e}")))?;
    let names = |pick: fn(&Button) -> bool| -> Value {
        description.buttons.iter().filter(|b| pick(b)).map(|b| b.id.as_str()).collect()
    };
    for (def, list) in [
        ("MacroTrigger", names(|b| b.can_start_macro)),
        ("StepButton", names(|b| b.can_be_in_macro_step)),
    ] {
        if let Some(def) =
            schema.pointer_mut(&format!("/$defs/{def}")).and_then(Value::as_object_mut)
        {
            def.insert("enum".to_owned(), list);
        }
    }
    if let Some(field) = schema.pointer_mut("/properties/device").and_then(Value::as_object_mut) {
        field.insert("const".to_owned(), Value::from(device));
    }
    jsonschema::draft202012::new(&schema)
        .map_err(|e| Error::Decode(format!("macro schema compile failed: {e}")))
}

/// Lazily compiles the profile schema validator (process-wide, once).
///
/// # Errors
/// Returns [`Error::Decode`] if the embedded schema fails to parse or compile.
pub(crate) fn profile_validator() -> Result<&'static Validator> {
    static V: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
    compiled(&V, PROFILE_SCHEMA_JSON, "profile")
}

/// Compile-once helper shared by both validators.
fn compiled<'a>(
    cell: &'a OnceLock<std::result::Result<Validator, String>>,
    schema_json: &str,
    label: &str,
) -> Result<&'a Validator> {
    cell.get_or_init(|| {
        let schema: Value = serde_json::from_str(schema_json).map_err(|e| e.to_string())?;
        jsonschema::draft202012::new(&schema).map_err(|e| e.to_string())
    })
    .as_ref()
    .map_err(|e| Error::Decode(format!("{label} schema compile failed: {e}")))
}

/// Runs the schema phase, collecting **all** violations as `{path, reason}`.
///
/// `path` is the error's `instance_path` JSON Pointer (root → `"/"`); `reason`
/// is the `jsonschema` crate's message (schema messages are not asserted
/// verbatim — only semantic messages are byte-significant).
// Called from service::validation::macros and ::profile.
pub(crate) fn schema_errors(validator: &Validator, value: &Value) -> Vec<ValidationError> {
    validator
        .iter_errors(value)
        .map(|e| {
            let raw = e.instance_path().as_str();
            ValidationError {
                path: if raw.is_empty() { "/".to_owned() } else { raw.to_owned() },
                reason: e.to_string(),
            }
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn both_embedded_schemas_compile() {
        // Guards the fancy-regex lookahead pattern + internal-$ref resolution
        // without the file/http resolvers (default-features = false).
        assert!(
            macro_validator(crate::devices::pro3::description().unwrap(), "8bitdo-pro3").is_ok()
        );
        assert!(profile_validator().is_ok());
    }

    #[test]
    fn schema_errors_empty_for_valid_macro() {
        let v =
            macro_validator(crate::devices::pro3::description().unwrap(), "8bitdo-pro3").unwrap();
        let macro_json = json!({
            "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
            "name": "TestMacro", "trigger": "rp",
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [ { "duration_ms": 30, "actions": { "buttons": { "press": ["bottom face"], "release": [] } } } ]
        });
        assert_eq!(schema_errors(&v, &macro_json), []);
    }

    #[test]
    fn schema_errors_reports_press_path_for_invalid_step_button() {
        let v =
            macro_validator(crate::devices::pro3::description().unwrap(), "8bitdo-pro3").unwrap();
        let macro_json = json!({
            "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
            "name": "TestMacro", "trigger": "rp",
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [ { "duration_ms": 30, "actions": { "buttons": { "press": ["turbo"], "release": [] } } } ]
        });
        let errors = schema_errors(&v, &macro_json);
        assert_ne!(errors, []);
        assert!(errors.iter().any(|e| e.path.contains("press")));
    }
}
