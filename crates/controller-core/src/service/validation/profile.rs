//! Profile validation (model schema, then semantic rules). Ports
//! `profile_validation_service.cpp`, with the settings rules read from the model's
//! description instead of code.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{ProfileValidationResult, ValidationError, ValidationSummary};
use crate::description::{ControllerDescription, DISABLED_OUTPUT, UNRECOGNISED_OUTPUT};
use crate::device::Model;
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, CanonicalProfileSummary};

/// Validates a canonical profile JSON object for `model`: the model's own schema
/// first, then the semantic rules.
///
/// `result.profile_id` is the JSON's `id` (empty if absent); `result.valid` is
/// `true` iff both phases produced no errors. Schema failures short-circuit.
///
/// # Errors
/// Returns [`Error::Decode`] only if an embedded schema or description fails to load
/// (a build-time invariant; never expected at runtime).
pub fn validate_profile(
    model: &dyn Model,
    profile_json: &Value,
) -> Result<ProfileValidationResult> {
    let profile_id = profile_json.get("id").and_then(Value::as_str).unwrap_or_default().to_owned();

    let schema = model.profile_schema_errors(profile_json)?;
    if !schema.is_empty() {
        return Ok(ProfileValidationResult { profile_id, valid: false, errors: schema });
    }

    let semantic = semantic_errors(model, model.description()?, profile_json);
    Ok(ProfileValidationResult { profile_id, valid: semantic.is_empty(), errors: semantic })
}

/// Validates a batch, mirroring the C++ `validateAll`: `all_valid` is `false`
/// if any profile is invalid; `results` preserves input order.
///
/// # Errors
/// Returns [`Error::Decode`] if a profile fails to serialize to JSON or an embedded
/// schema fails to load.
pub fn validate_all_profiles(
    model: &dyn Model,
    profiles: &[CanonicalProfileSummary],
) -> Result<ValidationSummary> {
    let mut results = Vec::with_capacity(profiles.len());
    let mut all_valid = true;
    for summary in profiles {
        let value = serde_json::to_value(&summary.canonical)
            .map_err(|e| Error::Decode(format!("profile serialize failed: {e}")))?;
        let result = validate_profile(model, &value)?;
        all_valid &= result.valid;
        results.push(result);
    }
    Ok(ValidationSummary { results, all_valid })
}

/// One error at `path`.
fn error(path: impl Into<String>, reason: impl Into<String>) -> ValidationError {
    ValidationError { path: path.into(), reason: reason.into() }
}

/// The last segment of a JSON pointer: `left_min_pct` for `/sticks/left_min_pct`.
fn name(field: &str) -> &str {
    field.rsplit('/').next().unwrap_or(field)
}

/// Accumulates every semantic error (port of `validateSemantics`).
///
/// The profile must be for this model and one of its modes, hold every setting its
/// mode declares with a value that fits, and nothing else. No flag may be on together
/// with a flag it excludes, such as the Pro 3's D-pad swap with swap sticks. Mappings
/// name buttons and outputs the model has, and each macro trigger is used once.
/// Analog trigger min/max ordering and gap are deliberately NOT enforced
/// (firmware-accepted; decoder auto-fixes), and macro-ref file existence is
/// deferred to the orchestrator layer.
fn semantic_errors(
    model: &dyn Model,
    description: &ControllerDescription,
    profile_json: &Value,
) -> Vec<ValidationError> {
    let profile: CanonicalProfile = match serde_json::from_value(profile_json.clone()) {
        Ok(p) => p,
        Err(e) => return vec![error("/", format!("This is not a profile: {e}."))],
    };
    let mut errors = Vec::new();
    let mode = profile.mode;
    let Some(mode_entry) = description.mode(mode) else {
        return vec![error("/mode", format!("This controller has no {mode} mode."))];
    };
    let default = model.default_profile(mode);
    for (path, got, want) in
        [("/device", &profile.device, &default.device), ("/kind", &profile.kind, &default.kind)]
    {
        if got != want {
            let shown = &description.display_name;
            errors.push(error(
                path,
                format!("This profile is for '{got}', not the {shown} ('{want}')."),
            ));
        }
    }

    let mut groups = BTreeSet::new();
    for page in description.pages(mode) {
        for field in page.fields() {
            groups.insert(field.trim_start_matches('/').split('/').next().unwrap_or_default());
            let value = profile.setting(field);
            if let Some(number) = description.number(mode, field) {
                let (lo, hi) = (i64::from(number.min), i64::from(number.max));
                if !value.and_then(Value::as_i64).is_some_and(|v| (lo..=hi).contains(&v)) {
                    errors.push(error(field, format!("{} must be {lo} to {hi}.", name(field))));
                }
            } else if !value.is_some_and(Value::is_boolean) {
                errors.push(error(field, format!("{} must be true or false.", name(field))));
            }
        }
    }
    for group in profile.settings.keys().filter(|g| !groups.contains(g.as_str())) {
        errors.push(error(
            format!("/{group}"),
            format!("This controller has no '{group}' settings in {mode} mode."),
        ));
    }

    let on = |field: &str| profile.setting(field).and_then(Value::as_bool) == Some(true);
    for flag in description.flags(mode).filter(|f| on(&f.field)) {
        let clashes: Vec<&str> = flag.excludes.iter().filter(|e| on(e)).map(|e| name(e)).collect();
        let Some((last, rest)) = clashes.split_last() else { continue };
        let joined = if rest.is_empty() {
            (*last).to_owned()
        } else {
            format!("{} and {last}", rest.join(", "))
        };
        let reason = format!("{} cannot be on together with {joined}.", name(&flag.field));
        errors.push(error(flag.field.clone(), reason));
    }

    for (i, mapping) in profile.button_mappings.iter().enumerate() {
        if description.button(&mapping.source).is_none() {
            let reason = format!("'{}' is not a button of this controller.", mapping.source);
            errors.push(error(format!("/button_mappings/{i}/source"), reason));
        }
        let target = mapping.target.as_str();
        let known = [DISABLED_OUTPUT, UNRECOGNISED_OUTPUT].contains(&target)
            || description.button(target).is_some()
            || mode_entry.extra_outputs.iter().any(|o| o.id == target);
        if !known {
            let reason = format!("'{target}' is not an output in {mode} mode.");
            errors.push(error(format!("/button_mappings/{i}/target"), reason));
        }
    }

    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (i, entry) in profile.macro_refs.iter().enumerate() {
        if !seen.insert(entry.trigger.as_str()) {
            errors.push(error(
                format!("/macro_refs/{i}/trigger"),
                format!(
                    "Duplicate macro trigger '{}'. Each trigger must be unique.",
                    entry.trigger
                ),
            ));
        }
    }

    errors
}
