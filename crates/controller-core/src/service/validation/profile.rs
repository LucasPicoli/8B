//! Profile validation (schema + semantic). Faithful port of
//! `profile_validation_service.cpp`.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    profile_validator, schema_errors, ProfileValidationResult, ValidationError, ValidationSummary,
};
use crate::error::{Error, Result};
use crate::model::CanonicalProfileSummary;

/// Validates a canonical profile JSON object (schema first, then semantics).
///
/// `result.profile_id` is the JSON's `id` (empty if absent); `result.valid` is
/// `true` iff both phases produced no errors. Schema failures short-circuit.
///
/// # Errors
/// Returns [`Error::Decode`](crate::error::Error::Decode) only if the embedded
/// profile schema fails to compile (a build-time invariant; never expected at runtime).
pub fn validate_profile(profile_json: &Value) -> Result<ProfileValidationResult> {
    let profile_id = profile_json.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();

    let schema = schema_errors(profile_validator()?, profile_json);
    if !schema.is_empty() {
        return Ok(ProfileValidationResult { profile_id, valid: false, errors: schema });
    }

    let semantic = semantic_errors(profile_json);
    Ok(ProfileValidationResult { profile_id, valid: semantic.is_empty(), errors: semantic })
}

/// Validates a batch, mirroring the C++ `validateAll`: `all_valid` is `false`
/// if any profile is invalid; `results` preserves input order.
///
/// # Errors
/// Returns [`Error::Decode`](crate::error::Error::Decode) if a profile fails to
/// serialize to JSON or the embedded schema fails to compile.
pub fn validate_all_profiles(profiles: &[CanonicalProfileSummary]) -> Result<ValidationSummary> {
    let mut results = Vec::with_capacity(profiles.len());
    let mut all_valid = true;
    for summary in profiles {
        let value = serde_json::to_value(&summary.canonical)
            .map_err(|e| Error::Decode(format!("profile serialize failed: {e}")))?;
        let result = validate_profile(&value)?;
        all_valid &= result.valid;
        results.push(result);
    }
    Ok(ValidationSummary { results, all_valid })
}

/// Accumulates every semantic error (port of `validateSemantics`).
///
/// Only one rule is reachable post-schema: duplicate `macro_refs` triggers.
/// Analog trigger min/max ordering and gap are deliberately NOT enforced
/// (firmware-accepted; decoder auto-fixes), and macro-ref file existence is
/// deferred to the orchestrator layer.
fn semantic_errors(profile_json: &Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    if let Some(refs) = profile_json.get("macro_refs").and_then(Value::as_array) {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for (i, entry) in refs.iter().enumerate() {
            let trigger = entry.get("trigger").and_then(Value::as_str).unwrap_or_default();
            if seen.contains(trigger) {
                errors.push(ValidationError {
                    path: format!("/macro_refs/{i}/trigger"),
                    reason: format!(
                        "Duplicate macro trigger '{trigger}'. Each trigger must be unique."
                    ),
                });
            }
            seen.insert(trigger);
        }
    }

    errors
}
