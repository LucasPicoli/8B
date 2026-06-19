//! Two-phase validation (JSON Schema + ported semantic rules).
//!
//! Faithful port of `profile_validation_service.*` and `macro_validation_service.*`.
//! Validators are pure: they consume canonical JSON (`serde_json::Value`) and
//! perform no I/O. Macro-ref file-existence resolution is deferred to the
//! orchestrator layer (Plan 2d), exactly as the C++ services defer it.

pub mod macros;
pub mod profile;

use std::sync::OnceLock;

use jsonschema::Validator;
use serde_json::Value;

use crate::error::{Error, Result};

// NOTE: the `pub use macros::validate_macro;` and
// `pub use profile::{validate_all_profiles, validate_profile};` re-exports are
// added by Tasks 2 and 3, when those functions exist. Task 1 ships only the
// shared types + schema infra below; `macros`/`profile` are empty stubs.

/// A single validation failure: a JSON-Pointer `path` and a human `reason`.
/// Mirrors the C++ `core::ValidationError`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidationError {
    /// JSON Pointer to the offending location (e.g. `/steps/0/actions/buttons`).
    pub path: String,
    /// Human-readable failure reason.
    pub reason: String,
}

/// Per-profile validation outcome. Mirrors the C++ `ProfileValidationResult`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ProfileValidationResult {
    /// The profile's `id` (empty string if absent).
    pub profile_id: String,
    /// Whether the profile passed both phases.
    pub valid: bool,
    /// All accumulated errors (empty iff `valid`).
    pub errors: Vec<ValidationError>,
}

/// Batch validation outcome. Mirrors the C++ `ValidationSummary`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ValidationSummary {
    /// One result per input profile, in order.
    pub results: Vec<ProfileValidationResult>,
    /// `true` iff every profile is valid.
    pub all_valid: bool,
}

/// The embedded macro schema (Draft 2020-12), compiled once.
// Used by macro_validator(); callers added by Tasks 2 & 3.
#[allow(dead_code)]
const MACRO_SCHEMA_JSON: &str = include_str!("../../../../../schemas/macro-v1.schema.json");
/// The embedded profile schema (Draft 2020-12), compiled once.
// Used by profile_validator(); callers added by Tasks 2 & 3.
#[allow(dead_code)]
const PROFILE_SCHEMA_JSON: &str = include_str!("../../../../../schemas/profile-v1.schema.json");

/// Lazily compiles the macro schema validator (process-wide, once).
///
/// # Errors
/// Returns [`Error::Decode`] if the embedded schema fails to parse or compile
/// (a build-time invariant; never expected at runtime).
// Called from service::validation::macros (Task 2).
#[allow(dead_code)]
pub(crate) fn macro_validator() -> Result<&'static Validator> {
    static V: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
    compiled(&V, MACRO_SCHEMA_JSON, "macro")
}

/// Lazily compiles the profile schema validator (process-wide, once).
///
/// # Errors
/// Returns [`Error::Decode`] if the embedded schema fails to parse or compile.
// Called from service::validation::profile (Task 3).
#[allow(dead_code)]
pub(crate) fn profile_validator() -> Result<&'static Validator> {
    static V: OnceLock<std::result::Result<Validator, String>> = OnceLock::new();
    compiled(&V, PROFILE_SCHEMA_JSON, "profile")
}

/// Compile-once helper shared by both validators.
#[allow(dead_code)]
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
// Called from service::validation::macros and ::profile (Tasks 2 & 3).
#[allow(dead_code)]
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
        assert!(macro_validator().is_ok());
        assert!(profile_validator().is_ok());
    }

    #[test]
    fn schema_errors_empty_for_valid_macro() {
        let v = macro_validator().unwrap();
        let macro_json = json!({
            "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
            "name": "TestMacro", "trigger": "rp",
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [ { "duration_ms": 30, "actions": { "buttons": { "press": ["bottom face"], "release": [] } } } ]
        });
        assert!(schema_errors(v, &macro_json).is_empty());
    }

    #[test]
    fn schema_errors_reports_press_path_for_invalid_step_button() {
        let v = macro_validator().unwrap();
        let macro_json = json!({
            "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
            "name": "TestMacro", "trigger": "rp",
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [ { "duration_ms": 30, "actions": { "buttons": { "press": ["turbo"], "release": [] } } } ]
        });
        let errors = schema_errors(v, &macro_json);
        assert!(!errors.is_empty());
        assert!(errors.iter().any(|e| e.path.contains("press")));
    }
}
