//! Macro validation (schema + semantic). Faithful port of
//! `macro_validation_service.cpp`.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{macro_validator, schema_errors, ValidationError};
use crate::devices::pro3::tables;
use crate::error::Result;

/// Button names valid as macro triggers but NOT as step actions.
/// Port of `macro_validation_service.cpp::invalidStepButtons()`.
const INVALID_STEP_BUTTONS: [&str; 6] = ["turbo", "lp", "rp", "l4", "r4", "home/guide"];

/// Validates a canonical macro JSON object (schema first, then semantics).
///
/// Returns an empty `Vec` iff the macro is valid. Schema failures short-circuit
/// (the semantic phase does not run); semantic failures accumulate.
///
/// # Errors
/// Returns [`Error::Decode`](crate::error::Error::Decode) only if the embedded
/// macro schema fails to compile (a build-time invariant; never expected at
/// runtime).
pub fn validate_macro(macro_json: &Value) -> Result<Vec<ValidationError>> {
    let schema = schema_errors(macro_validator()?, macro_json);
    if !schema.is_empty() {
        return Ok(schema);
    }
    Ok(semantic_errors(macro_json))
}

/// Accumulates every semantic error (port of `validateSemantics`).
fn semantic_errors(macro_json: &Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // 1. Trigger eligibility — must map to a non-zero key_map.
    let trigger = macro_json.get("trigger").and_then(Value::as_str).unwrap_or_default();
    if !tables::TRIGGERS.iter().any(|t| t.name == trigger && t.key_map != 0) {
        errors.push(ValidationError {
            path: "/trigger".to_owned(),
            reason: format!("'{trigger}' is not a valid macro trigger."),
        });
    }

    // 2. Name length — 1..=15 UTF-16 code units (mirrors QString::size()).
    let name = macro_json.get("name").and_then(Value::as_str).unwrap_or_default();
    let name_len = name.encode_utf16().count();
    if name.is_empty() || name_len > 15 {
        errors.push(ValidationError {
            path: "/name".to_owned(),
            reason: format!("Name must be 1-15 characters (got {name_len})."),
        });
    }

    // 3. repeat.count >= 1.
    let count = macro_json
        .get("repeat")
        .and_then(|r| r.get("count"))
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if count < 1 {
        errors.push(ValidationError {
            path: "/repeat/count".to_owned(),
            reason: format!("repeat.count must be >= 1 (got {count})."),
        });
    }

    // 4. Per-step checks.
    if let Some(steps) = macro_json.get("steps").and_then(Value::as_array) {
        for (i, step) in steps.iter().enumerate() {
            let buttons = step.get("actions").and_then(|a| a.get("buttons"));
            let press = buttons.and_then(|b| b.get("press")).and_then(Value::as_array);
            let release = buttons.and_then(|b| b.get("release")).and_then(Value::as_array);

            // 4a/4b. Invalid step-action buttons in press / release.
            for (kind, arr) in [("press", press), ("release", release)] {
                let Some(arr) = arr else { continue };
                for (j, btn) in arr.iter().enumerate() {
                    let btn = btn.as_str().unwrap_or_default();
                    if INVALID_STEP_BUTTONS.contains(&btn) {
                        errors.push(ValidationError {
                            path: format!("/steps/{i}/actions/buttons/{kind}/{j}"),
                            reason: format!("Step {i}: '{btn}' is not a valid step-action button."),
                        });
                    }
                }
            }

            // 4c. A button cannot appear in both press and release.
            let press_set: BTreeSet<&str> =
                press.into_iter().flatten().filter_map(Value::as_str).collect();
            for btn in release.into_iter().flatten().filter_map(Value::as_str) {
                if press_set.contains(btn) {
                    errors.push(ValidationError {
                        path: format!("/steps/{i}/actions/buttons"),
                        reason: format!("Step {i}: '{btn}' appears in both press and release."),
                    });
                }
            }
        }
    }

    errors
}
