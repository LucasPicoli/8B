//! Profile validation (schema + semantic). Faithful port of
//! `profile_validation_service.cpp`.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{
    profile_validator, schema_errors, ProfileValidationResult, ValidationError, ValidationSummary,
};
use crate::error::{Error, Result};
use crate::model::{CanonicalProfileSummary, Sticks};

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

/// The refusal for a profile with the D-pad swap on together with swap sticks, invert
/// left X or invert left Y, naming each flag that clashes. `None` when none does.
#[must_use]
pub fn dpad_swap_clash(sticks: &Sticks) -> Option<String> {
    let clashes = sticks.dpad_swap_clashes();
    let (last, rest) = clashes.split_last()?;
    let joined = if rest.is_empty() {
        (*last).to_owned()
    } else {
        format!("{} and {last}", rest.join(", "))
    };
    Some(format!("swap_dpad_with_left_stick cannot be on together with {joined}."))
}

/// Accumulates every semantic error (port of `validateSemantics`).
///
/// Two rules are reachable post-schema: duplicate `macro_refs` triggers, and the
/// D-pad swap with swap sticks or an inverted left stick (the vendor app's rule).
/// Analog trigger min/max ordering and gap are deliberately NOT enforced
/// (firmware-accepted; decoder auto-fixes), and macro-ref file existence is
/// deferred to the orchestrator layer.
fn semantic_errors(profile_json: &Value) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    let sticks =
        profile_json.get("sticks").and_then(|s| serde_json::from_value::<Sticks>(s.clone()).ok());
    if let Some(reason) = sticks.as_ref().and_then(dpad_swap_clash) {
        errors
            .push(ValidationError { path: "/sticks/swap_dpad_with_left_stick".to_owned(), reason });
    }

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
