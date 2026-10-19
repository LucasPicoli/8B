//! Profile validation (model schema, then semantic rules), with the settings rules read
//! from the model's description instead of code.

use std::collections::BTreeSet;

use serde_json::Value;

use super::{validate_macro, ProfileValidationResult, ValidationError, ValidationSummary};
use crate::description::{ControllerDescription, DISABLED_OUTPUT, UNRECOGNISED_OUTPUT};
use crate::device::Model;
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, CanonicalProfileSummary};
use crate::Reason;

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

    let embedded = embedded_macro_errors(model, profile_json)?;
    if !embedded.is_empty() {
        return Ok(ProfileValidationResult { profile_id, valid: false, errors: embedded });
    }

    let semantic = semantic_errors(model, model.description()?, profile_json);
    Ok(ProfileValidationResult { profile_id, valid: semantic.is_empty(), errors: semantic })
}

/// Validates a batch: `all_valid` is `false`
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

/// The errors [`validate_macro`] finds in each `macros` entry, at `/macros/{i}`. An
/// entry is checked as a macro file: it takes the profile's `version` and `device` when
/// it has none, and drops its `macro_slot`.
fn embedded_macro_errors(model: &dyn Model, profile_json: &Value) -> Result<Vec<ValidationError>> {
    let entries = profile_json.get("macros").and_then(Value::as_array);
    let mut errors = Vec::new();
    for (i, entry) in entries.into_iter().flatten().enumerate() {
        let mut file = entry.clone();
        if let Some(map) = file.as_object_mut() {
            map.remove("macro_slot");
            for key in ["version", "device"] {
                if let Some(value) = profile_json.get(key) {
                    map.entry(key).or_insert_with(|| value.clone());
                }
            }
        }
        for e in validate_macro(model, &file)? {
            let at = if e.path == "/" { "" } else { e.path.as_str() };
            let reason = Reason::InMacro { index: i, reason: Box::new(e.reason) };
            errors.push(error(format!("/macros/{i}{at}"), reason));
        }
    }
    Ok(errors)
}

/// One error at `path`.
fn error(path: impl Into<String>, reason: Reason) -> ValidationError {
    ValidationError { path: path.into(), reason }
}

/// Accumulates every semantic error.
///
/// The profile must be for this model and one of its modes, hold every setting its
/// mode declares with a value that fits, and nothing else. No flag may be on together
/// with a flag it excludes, such as the Pro 3's D-pad swap with swap sticks. Mappings
/// name buttons and outputs the model has, and the macros pass [`macro_errors`].
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
        Err(e) => return vec![error("/", Reason::NotAProfile(e.to_string()))],
    };
    let mut errors = Vec::new();
    let mode = profile.mode;
    let Some(mode_entry) = description.mode(mode) else {
        return vec![error("/mode", Reason::NoMode(mode))];
    };
    let default = model.default_profile(mode);
    for (path, got, want) in
        [("/device", &profile.device, &default.device), ("/kind", &profile.kind, &default.kind)]
    {
        if got != want {
            let shown = description.display_name.clone();
            let reason = Reason::WrongDevice { got: got.clone(), shown, want: want.clone() };
            errors.push(error(path, reason));
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
                    let reason = Reason::OutOfRange { field: field.to_owned(), mode, lo, hi };
                    errors.push(error(field, reason));
                }
            } else if !value.is_some_and(Value::is_boolean) {
                errors.push(error(field, Reason::NotBool { field: field.to_owned(), mode }));
            }
        }
    }
    for group in profile.settings.keys().filter(|g| !groups.contains(g.as_str())) {
        errors
            .push(error(format!("/{group}"), Reason::UnknownGroup { group: group.clone(), mode }));
    }

    let on = |field: &str| profile.setting(field).and_then(Value::as_bool) == Some(true);
    for flag in description.flags(mode).filter(|f| on(&f.field)) {
        let clashes: Vec<String> = flag.excludes.iter().filter(|e| on(e)).cloned().collect();
        if clashes.is_empty() {
            continue;
        }
        let reason = Reason::Excludes { flag: flag.field.clone(), clashes, mode };
        errors.push(error(flag.field.clone(), reason));
    }

    for (i, mapping) in profile.button_mappings.iter().enumerate() {
        if description.button(&mapping.source).is_none() {
            let reason = Reason::UnknownSource { source: mapping.source.clone() };
            errors.push(error(format!("/button_mappings/{i}/source"), reason));
        }
        let target = mapping.target.as_str();
        let extra = mode_entry.extra_outputs.iter().find(|o| o.id == target);
        let known = [DISABLED_OUTPUT, UNRECOGNISED_OUTPUT].contains(&target)
            || description.button(target).is_some()
            || extra.is_some();
        if !known {
            let reason = Reason::UnknownTarget { target: target.to_owned(), mode };
            errors.push(error(format!("/button_mappings/{i}/target"), reason));
        } else if extra.is_some_and(|o| !o.allows(&mapping.source)) {
            let (source, target) = (mapping.source.clone(), target.to_owned());
            let reason = Reason::TargetNotOnButton { source, target, mode };
            errors.push(error(format!("/button_mappings/{i}/target"), reason));
        }
    }

    errors.extend(macro_errors(description, &profile));
    errors
}

/// The macro errors of `profile`: each embedded macro has the profile's mode and its own
/// macro slot, a profile holds `macros` or non-empty `macro_refs`, and a button starts
/// at most one macro across both lists.
fn macro_errors(
    description: &ControllerDescription,
    profile: &CanonicalProfile,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let mode = profile.mode;
    let embedded = profile.macros.as_deref().unwrap_or_default();
    if profile.macros.is_some() && !profile.macro_refs.is_empty() {
        errors.push(error("/macro_refs", Reason::BothMacroLists));
    }
    let count = description.macro_slot_count;
    let mut slots = BTreeSet::new();
    for (i, def) in embedded.iter().enumerate() {
        if def.mode != mode {
            let (got, want) = (description.mode_label(def.mode), description.mode_label(mode));
            let reason = Reason::MacroModeMismatch { got, want };
            errors.push(error(format!("/macros/{i}/mode"), reason));
        }
        let slot = def.macro_slot.unwrap_or_default();
        if slot >= count {
            errors.push(error(format!("/macros/{i}/macro_slot"), Reason::SlotOutOfRange { count }));
        } else if !slots.insert(slot) {
            errors.push(error(format!("/macros/{i}/macro_slot"), Reason::SlotTwice { slot }));
        }
    }

    let refs = profile.macro_refs.iter().enumerate();
    let refs = refs.map(|(i, r)| (format!("/macro_refs/{i}/trigger_buttons"), &r.trigger_buttons));
    let macros = embedded.iter().enumerate();
    let macros = macros.map(|(i, m)| (format!("/macros/{i}/trigger_buttons"), &m.trigger_buttons));
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for (path, buttons) in refs.chain(macros) {
        for button in buttons {
            if !seen.insert(button.as_str()) {
                let reason = Reason::TwoMacrosOneButton { button: button.clone(), mode };
                errors.push(error(path.clone(), reason));
            }
        }
    }

    errors
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::device::{ControllerSpec as _, ProtocolCodec as _};
    use crate::devices::pro3::{Pro3, SWITCH, XINPUT};
    use crate::devices::test_pad::{TestPad, STANDARD};
    use crate::model::MacroDefinition;

    /// A one-step macro in `mode` and `macro_slot`, started by no button.
    fn tap(mode: crate::model::Mode, macro_slot: u8) -> MacroDefinition {
        MacroDefinition {
            name: "Tap".to_owned(),
            mode,
            trigger_buttons: Vec::new(),
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![crate::model::MacroStep { duration_ms: 50, ..Default::default() }],
            macro_slot: Some(macro_slot),
        }
    }

    #[test]
    fn an_extra_output_on_a_button_it_is_not_for_is_refused() {
        let mut description = Pro3.description().unwrap().clone();
        let switch = description.modes.iter_mut().find(|m| m.id == SWITCH).unwrap();
        switch.extra_outputs[0].buttons = vec!["turbo".to_owned(), "l4".to_owned()];
        let mut profile = Pro3.default_profile(SWITCH);
        for (source, target) in [("l4", "screenshot"), ("r4", "screenshot")] {
            let mapping =
                crate::model::ButtonMapping { source: source.into(), target: target.into() };
            profile.button_mappings.push(mapping);
        }
        let at = profile.button_mappings.len() - 1;
        let json = serde_json::to_value(&profile).unwrap();
        let reason = Reason::TargetNotOnButton {
            source: "r4".into(),
            target: "screenshot".into(),
            mode: SWITCH,
        };
        let errors = semantic_errors(&Pro3, &description, &json);
        assert_eq!(errors, [error(format!("/button_mappings/{at}/target"), reason)]);
        let errors = semantic_errors(&Pro3, Pro3.description().unwrap(), &json);
        assert_eq!(errors, [], "no list: every button takes it");
    }

    /// The Pro 3 schema stops a macro slot above 3, so only this reaches the rule.
    #[test]
    fn a_macro_slot_the_controller_lacks_is_refused() {
        let description = Pro3.description().unwrap();
        let profile = CanonicalProfile {
            macros: Some(vec![tap(XINPUT, 3), tap(XINPUT, 4)]),
            ..Pro3.default_profile(XINPUT)
        };
        let errors = macro_errors(description, &profile);
        assert_eq!(errors, [error("/macros/1/macro_slot", Reason::SlotOutOfRange { count: 4 })]);
    }

    #[test]
    fn two_macros_of_one_profile_cannot_share_a_trigger() {
        let description = Pro3.description().unwrap();
        let started_by = |slot, button: &str| MacroDefinition {
            trigger_buttons: vec![button.to_owned()],
            ..tap(XINPUT, slot)
        };
        let profile = CanonicalProfile {
            macros: Some(vec![started_by(0, "r4"), started_by(1, "r4")]),
            ..Pro3.default_profile(XINPUT)
        };
        let errors = macro_errors(description, &profile);
        let reason = Reason::TwoMacrosOneButton { button: "r4".into(), mode: XINPUT };
        assert_eq!(errors, [error("/macros/1/trigger_buttons", reason)]);
    }

    #[test]
    fn a_controller_with_no_macro_slots_takes_no_embedded_macro() {
        let mut profile = serde_json::to_value(TestPad.default_profile(STANDARD)).unwrap();
        profile["macros"] = json!([]);
        assert_eq!(embedded_macro_errors(&TestPad, &profile).unwrap(), []);
        profile["macros"] = json!([{ "mode": "standard", "macro_slot": 0 }]);
        let errors = embedded_macro_errors(&TestPad, &profile).unwrap();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, "/macros/0");
        assert!(errors[0].reason.to_string().ends_with("has no macro slots."), "{errors:?}");
    }
}
