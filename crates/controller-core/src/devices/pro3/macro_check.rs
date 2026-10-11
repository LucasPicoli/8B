//! Pro 3 macro validation (schema + semantic). The app reads macros but never imports
//! one, so only the tests run it.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::description::ControllerDescription;
use crate::error::{Error, Result};
use crate::service::validation::{macro_validator, schema_errors, ValidationError};

/// Validates a canonical macro JSON object (schema first, then semantics). The schema
/// phase takes its trigger and step button lists from the `can_start_macro` and
/// `can_be_in_macro_step` flags of `description`.
///
/// Returns an empty `Vec` iff the macro is valid. Schema failures short-circuit
/// (the semantic phase does not run); semantic failures accumulate.
///
/// # Errors
/// Returns [`Error::Decode`](crate::error::Error::Decode) only if the embedded
/// macro schema fails to compile, which includes a description with no button
/// that can start a macro or no button that can be in a step.
pub fn validate_macro(
    description: &ControllerDescription,
    macro_json: &Value,
) -> Result<Vec<ValidationError>> {
    let schema = schema_errors(&macro_validator(description)?, macro_json);
    if !schema.is_empty() {
        return Ok(schema);
    }
    let max_name = usize::try_from(description.limits.macro_name_length.max)
        .map_err(|_| Error::Decode("macro name limit is negative".into()))?;
    Ok(semantic_errors(macro_json, max_name))
}

/// Accumulates every semantic error.
fn semantic_errors(macro_json: &Value, max_name: usize) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // 1. Name length — 1..=`max_name` UTF-16 code units, the unit the descriptor stores.
    // The schema counts code points, so only this check sees a surrogate pair as two.
    let name = macro_json.get("name").and_then(Value::as_str).unwrap_or_default();
    let name_len = name.encode_utf16().count();
    if name.is_empty() || name_len > max_name {
        errors.push(ValidationError {
            path: "/name".to_owned(),
            reason: format!("Name must be 1-{max_name} characters (got {name_len})."),
        });
    }

    // 2. repeat.count >= 1.
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

    // 3. Per-step checks.
    if let Some(steps) = macro_json.get("steps").and_then(Value::as_array) {
        for (i, step) in steps.iter().enumerate() {
            let buttons = step.get("actions").and_then(|a| a.get("buttons"));
            let press = buttons.and_then(|b| b.get("press")).and_then(Value::as_array);
            let release = buttons.and_then(|b| b.get("release")).and_then(Value::as_array);

            // 3a. A button cannot appear in both press and release.
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
