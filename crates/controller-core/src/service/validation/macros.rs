//! Macro validation (schema + semantic), for any model with macro slots.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::description::{ControllerDescription, MacroStick, TriggerKind, TriggerSide};
use crate::device::Model;
use crate::error::{Error, Result};
use crate::model::glide::{axes, expanded_len};
use crate::model::macros::macro_from_json;
use crate::model::{GlideControl, MacroDefinition, Mode};
use crate::service::validation::{macro_validator, schema_errors, ValidationError};
use crate::Reason;

/// Validates a canonical macro JSON object for `model` (schema first, then semantics).
///
/// The schema phase takes its trigger and step button lists from the `can_start_macro`
/// and `can_be_in_macro_step` flags of the model's description, and its `device` from
/// the model's profiles.
///
/// Returns an empty `Vec` iff the macro is valid. A controller with no macro slots
/// accepts no macro: that is one error at `/`. Schema failures short-circuit (the
/// semantic phase does not run); semantic failures accumulate.
///
/// # Errors
/// Returns [`Error::Decode`](crate::error::Error::Decode) only if the embedded
/// macro schema fails to compile or the embedded description is malformed (build-time
/// invariants; never expected at runtime).
pub fn validate_macro(model: &dyn Model, macro_json: &Value) -> Result<Vec<ValidationError>> {
    let description = model.description()?;
    let device = description
        .modes
        .first()
        .map_or_else(String::new, |mode| model.default_profile(mode.id).device);
    check(description, &device, macro_json)
}

/// [`validate_macro`] against `description` and `device`.
fn check(
    description: &ControllerDescription,
    device: &str,
    macro_json: &Value,
) -> Result<Vec<ValidationError>> {
    if description.macro_slot_count == 0 {
        return Ok(vec![ValidationError {
            path: "/".to_owned(),
            reason: Reason::NoMacroSlots { display_name: description.display_name.clone() },
        }]);
    }
    // The name check says it in plain words, so the schema's own name errors go. A model
    // without macro names leaves the schema to refuse one.
    let mut errors = match description.limits.macro_name_length {
        Some(names) => {
            let max_name = usize::try_from(names.max)
                .map_err(|_| Error::Decode("macro name limit is negative".into()))?;
            name_errors(description, macro_json, max_name)
        }
        None => Vec::new(),
    };
    let has_names = description.limits.macro_name_length.is_some();
    let schema =
        schema_errors(&macro_validator(description, device)?, macro_json, Reason::MacroSchema);
    let schema: Vec<_> = schema.into_iter().filter(|e| !has_names || e.path != "/name").collect();
    if !schema.is_empty() {
        errors.extend(schema);
        return Ok(errors);
    }
    let def = macro_from_json(macro_json)?;
    errors.extend(semantic_errors(macro_json, def.mode));
    errors.extend(glide_errors(description, &def));
    Ok(errors)
}

/// The error when the name is empty or longer than `max_name` UTF-16 code units, the
/// unit the descriptor stores. The schema counts code points, so only this check sees a
/// surrogate pair as two.
fn name_errors(
    description: &ControllerDescription,
    macro_json: &Value,
    max_name: usize,
) -> Vec<ValidationError> {
    let name = macro_json.get("name").and_then(Value::as_str).unwrap_or_default();
    let len = name.encode_utf16().count();
    let reason = if name.is_empty() {
        Reason::NameEmpty { max: max_name }
    } else if len > max_name {
        Reason::NameTooLong { len, short_name: description.short_name.clone(), max: max_name }
    } else {
        return Vec::new();
    };
    vec![ValidationError { path: "/name".to_owned(), reason }]
}

/// The glide errors of `def`, and the error when its expanded steps do not fit the
/// controller. `def` passed the schema.
fn glide_errors(
    description: &ControllerDescription,
    def: &MacroDefinition,
) -> Vec<ValidationError> {
    let mut errors = Vec::new();
    let on_off = description.mode(def.mode).is_some_and(|m| m.trigger_kind == TriggerKind::OnOff);
    let mode = description.mode_label(def.mode);
    let steps = def.steps.iter().zip(def.steps.iter().skip(1).map(Some).chain([None]));
    for (i, (step, next)) in steps.enumerate() {
        let n = i + 1; // the Macros tab numbers steps from 1; `path` stays 0-based
        let mut error = |reason: Reason| {
            errors.push(ValidationError { path: format!("/steps/{i}/glide"), reason });
        };
        for &control in &step.glide {
            let name = control_label(description, def.mode, control);
            let is_trigger =
                matches!(control, GlideControl::LeftTrigger | GlideControl::RightTrigger);
            match next {
                None => error(Reason::GlideNoNextStep { step: n, name }),
                _ if is_trigger && on_off => {
                    error(Reason::GlideOnOffTrigger { step: n, name, mode: mode.clone() });
                }
                Some(next) if axes(step, control) == axes(next, control) => {
                    error(Reason::GlideSameValue { step: n, name });
                }
                _ => {}
            }
        }
    }
    let limits = &description.limits;
    let max = usize::try_from(limits.macro_steps.max).unwrap_or_default();
    let needed = expanded_len(&def.steps, limits.glide_step_ms);
    if needed > max {
        let reason = if needed == def.steps.len() {
            Reason::TooManySteps { needed, short_name: description.short_name.clone(), max }
        } else {
            Reason::TooManyGlideSteps { needed, max }
        };
        errors.push(ValidationError { path: "/steps".to_owned(), reason });
    }
    errors
}

/// What the user calls `control` in `mode`: the title of its stick's settings frame
/// (`Left stick`) or the label of its trigger button (`LT`). The schema id when the
/// description names neither.
fn control_label(description: &ControllerDescription, mode: Mode, control: GlideControl) -> String {
    let (stick, side) = match control {
        GlideControl::LeftStick => (Some(MacroStick::LeftStick), None),
        GlideControl::RightStick => (Some(MacroStick::RightStick), None),
        GlideControl::LeftTrigger => (None, Some(TriggerSide::Left)),
        GlideControl::RightTrigger => (None, Some(TriggerSide::Right)),
    };
    let label = stick
        .and_then(|stick| {
            description
                .pages(mode)
                .flat_map(|p| &p.frames)
                .find(|f| f.sliders.iter().any(|s| s.macro_stick == Some(stick)))
                .map(|f| f.title.clone())
        })
        .or_else(|| {
            let button = description.buttons.iter().find(|b| b.step_trigger_value == side)?;
            button.labels.get(&mode).cloned()
        });
    label.unwrap_or_else(|| control.as_str().to_owned())
}

/// Accumulates every semantic error but the name's, see [`name_errors`]. `mode` is the
/// macro's.
fn semantic_errors(macro_json: &Value, mode: Mode) -> Vec<ValidationError> {
    let mut errors = Vec::new();

    // 1. repeat.count >= 1.
    let count = macro_json
        .get("repeat")
        .and_then(|r| r.get("count"))
        .and_then(Value::as_i64)
        .unwrap_or_default();
    if count < 1 {
        errors.push(ValidationError {
            path: "/repeat/count".to_owned(),
            reason: Reason::RepeatCount { count },
        });
    }

    // 2. Per-step checks.
    if let Some(steps) = macro_json.get("steps").and_then(Value::as_array) {
        for (i, step) in steps.iter().enumerate() {
            let n = i + 1; // the Macros tab numbers steps from 1; `path` stays 0-based
            let buttons = step.get("actions").and_then(|a| a.get("buttons"));
            let press = buttons.and_then(|b| b.get("press")).and_then(Value::as_array);
            let release = buttons.and_then(|b| b.get("release")).and_then(Value::as_array);

            // 2a. A button cannot appear in both press and release.
            let press_set: BTreeSet<&str> =
                press.into_iter().flatten().filter_map(Value::as_str).collect();
            for btn in release.into_iter().flatten().filter_map(Value::as_str) {
                if press_set.contains(btn) {
                    errors.push(ValidationError {
                        path: format!("/steps/{i}/actions/buttons"),
                        reason: Reason::PressAndRelease { step: n, button: btn.to_owned(), mode },
                    });
                }
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
    use crate::devices::test_pad::TestPad;

    fn a_macro() -> Value {
        json!({
            "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
            "name": "TestMacro", "trigger": "rp",
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [ { "duration_ms": 30, "actions": { "buttons": { "press": ["l4"], "release": [] } } } ]
        })
    }

    #[test]
    fn a_controller_without_macro_slots_accepts_no_macro() {
        let errors = validate_macro(&TestPad, &a_macro()).unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "/");
        assert!(errors[0].reason.to_string().contains("no macro slots"), "{}", errors[0].reason);
    }

    #[test]
    fn the_flags_of_the_description_decide_which_buttons_pass() {
        let mut description = crate::devices::pro3::description().unwrap().clone();
        assert_ne!(check(&description, "8bitdo-pro3", &a_macro()).unwrap(), []);
        for b in &mut description.buttons {
            b.can_be_in_macro_step |= b.id == "l4";
        }
        assert_eq!(check(&description, "8bitdo-pro3", &a_macro()).unwrap(), []);
    }

    /// The Pro 3 with the macro shape of a pad that has no macro names, repeats, pauses
    /// or glides, takes one trigger per macro, and has no `DInput` mode.
    fn plain_macros() -> ControllerDescription {
        let mut d = crate::devices::pro3::description().unwrap().clone();
        for b in &mut d.buttons {
            b.can_be_in_macro_step |= b.id == "l4";
        }
        d.modes.retain(|m| m.id != crate::devices::pro3::DINPUT);
        let l = &mut d.limits;
        (l.macro_name_length, l.macro_repeat_count, l.macro_interval_ms) = (None, None, None);
        (l.glide_step_ms, l.triggers_per_macro) = (None, Some(1));
        d
    }

    #[test]
    fn a_model_without_a_macro_feature_takes_no_macro_that_uses_it() {
        let d = plain_macros();
        let mut nameless = a_macro();
        nameless.as_object_mut().unwrap().remove("name");
        assert_eq!(check(&d, "8bitdo-pro3", &nameless).unwrap(), []);
        let mut empty = a_macro();
        empty["name"] = json!("");
        assert_eq!(check(&d, "8bitdo-pro3", &empty).unwrap(), []);
        let cases: [fn(&mut Value); 6] = [
            |m| m["name"] = json!("Named"),
            |m| m["repeat"]["count"] = json!(2),
            |m| m["repeat"]["interval_ms"] = json!(5),
            |m| {
                m.as_object_mut().unwrap().remove("trigger");
                m["trigger_buttons"] = json!(["rp", "lp"]);
            },
            |m| m["steps"][0]["glide"] = json!(["left_stick"]),
            |m| m["mode"] = json!("dinput"),
        ];
        for (i, use_it) in cases.iter().enumerate() {
            let mut m = empty.clone();
            use_it(&mut m);
            assert_ne!(check(&d, "8bitdo-pro3", &m).unwrap(), [], "case {i} should fail");
        }
    }

    #[test]
    fn the_pro_3_still_takes_names_repeats_and_many_triggers() {
        let mut d = crate::devices::pro3::description().unwrap().clone();
        for b in &mut d.buttons {
            b.can_be_in_macro_step |= b.id == "l4";
        }
        let mut m = a_macro();
        m.as_object_mut().unwrap().remove("trigger");
        m["trigger_buttons"] = json!(["rp", "lp"]);
        m["repeat"] = json!({ "count": 3, "interval_ms": 40 });
        m["mode"] = json!("dinput");
        assert_eq!(check(&d, "8bitdo-pro3", &m).unwrap(), []);
        m["name"] = json!("");
        let errors = check(&d, "8bitdo-pro3", &m).unwrap();
        assert!(matches!(
            errors[..],
            [ValidationError { reason: Reason::NameEmpty { max: 16 }, .. }]
        ));
    }
}
