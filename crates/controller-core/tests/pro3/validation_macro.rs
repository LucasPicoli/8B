//! Pro 3 macro validation: semantic rules and the macro schema.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::devices::pro3::Pro3;
use controller_core::service::validation::ValidationError;
use serde_json::{json, Value};

/// Validates against the Pro 3 description.
fn validate_macro(macro_json: &Value) -> controller_core::Result<Vec<ValidationError>> {
    controller_core::service::validation::validate_macro(&Pro3, macro_json)
}

/// Minimal valid macro JSON.
fn make_valid_macro() -> Value {
    json!({
        "version": 1, "device": "8bitdo-pro3", "mode": "xinput",
        "name": "TestMacro", "trigger": "rp",
        "repeat": { "count": 1, "interval_ms": 0 },
        "steps": [
            { "duration_ms": 30, "actions": { "buttons": { "press": ["bottom face"], "release": [] } } }
        ]
    })
}

/// Replace a step's press/release arrays.
/// Direct `IndexMut` assignment unambiguously *moves* `press`/`release` into the
/// tree (no `json!` borrow ambiguity, no `needless_pass_by_value`).
fn set_step_buttons(mut macro_json: Value, step: usize, press: Value, release: Value) -> Value {
    macro_json["steps"][step]["actions"]["buttons"]["press"] = press;
    macro_json["steps"][step]["actions"]["buttons"]["release"] = release;
    macro_json
}

fn a_valid_step() -> Value {
    json!({ "duration_ms": 30, "actions": { "buttons": { "press": ["right face"], "release": [] } } })
}

// --- Positive ---------------------------------------------------------------

#[test]
fn trigger_buttons_are_a_list_of_distinct_macro_trigger_buttons() {
    let with = |buttons: Value| {
        let mut m = make_valid_macro();
        m.as_object_mut().unwrap().remove("trigger");
        m["trigger_buttons"] = buttons;
        m
    };
    assert_eq!(validate_macro(&with(json!([]))).unwrap(), [], "no button");
    assert_eq!(validate_macro(&with(json!(["rp", "lp"]))).unwrap(), [], "two buttons");
    assert_ne!(validate_macro(&with(json!(["rp", "rp"]))).unwrap(), [], "a duplicate");
    assert_ne!(validate_macro(&with(json!(["home/guide"]))).unwrap(), [], "no can_start_macro");
    let mut both = make_valid_macro();
    both["trigger_buttons"] = json!(["rp"]);
    assert_ne!(validate_macro(&both).unwrap(), [], "trigger and trigger_buttons");
    let mut neither = make_valid_macro();
    neither.as_object_mut().unwrap().remove("trigger");
    assert_ne!(validate_macro(&neither).unwrap(), [], "no trigger at all");
}

#[test]
fn valid_macro_passes_validation() {
    assert_eq!(validate_macro(&make_valid_macro()).unwrap(), []);
}

// --- Schema-caught failures ------------------------------------------------

#[test]
fn an_unknown_trigger_or_step_button_fails_at_its_path() {
    let mut m = make_valid_macro();
    m["trigger"] = json!("nope");
    let paths: Vec<_> = validate_macro(&m).unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["/trigger"]);

    let m = set_step_buttons(make_valid_macro(), 0, json!(["nope"]), json!(["l4"]));
    let paths: Vec<_> = validate_macro(&m).unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["/steps/0/actions/buttons/press/0", "/steps/0/actions/buttons/release/0"]);
}

#[test]
fn a_macro_for_another_device_fails_at_device() {
    let mut m = make_valid_macro();
    m["device"] = json!("8bitdo-ultimate");
    let paths: Vec<_> = validate_macro(&m).unwrap().into_iter().map(|e| e.path).collect();
    assert_eq!(paths, ["/device"]);
}

#[test]
fn zero_steps_fails_schema() {
    let mut m = make_valid_macro();
    m["steps"] = json!([]);
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn home_guide_trigger_fails() {
    let mut m = make_valid_macro();
    m["trigger"] = json!("home/guide");
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn turbo_in_step_fails_and_mentions_press_or_turbo() {
    let m = set_step_buttons(make_valid_macro(), 0, json!(["turbo"]), json!([]));
    let errors = validate_macro(&m).unwrap();
    assert_ne!(errors, []);
    assert!(errors
        .iter()
        .any(|e| e.reason.to_lowercase().contains("turbo") || e.path.contains("press")));
}

#[test]
fn paddle_buttons_in_step_fail_schema() {
    for btn in ["lp", "rp", "l4", "r4"] {
        let m = set_step_buttons(make_valid_macro(), 0, json!([btn]), json!([]));
        assert!(!validate_macro(&m).unwrap().is_empty(), "{btn} should fail");
    }
}

#[test]
fn name_too_long_fails() {
    let mut m = make_valid_macro();
    m["name"] = json!("12345678901234567"); // 17 chars
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn name_empty_fails() {
    let mut m = make_valid_macro();
    m["name"] = json!("");
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn repeat_count_zero_fails() {
    let mut m = make_valid_macro();
    m["repeat"] = json!({ "count": 0, "interval_ms": 0 });
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn dinput_mode_passes_validation() {
    let mut m = make_valid_macro();
    m["mode"] = json!("dinput");
    assert_eq!(validate_macro(&m).unwrap(), []);
}

#[test]
fn name_of_16_units_passes_validation() {
    let mut m = make_valid_macro();
    m["name"] = json!("macro1dinputTEST");
    assert_eq!(validate_macro(&m).unwrap(), []);
}

#[test]
fn name_limit_counts_utf16_units_not_code_points() {
    // 9 code points pass the schema, but each takes two units: 18 > 16.
    let mut m = make_valid_macro();
    m["name"] = json!("😀".repeat(9));
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "/name");
    assert_eq!(errors[0].reason, "The name has 18 characters; the Pro 3 takes at most 16.");
}

#[test]
fn an_empty_or_long_name_gets_one_plain_reason_not_the_schema_text() {
    let mut m = make_valid_macro();
    m["name"] = json!("");
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "/name");
    assert_eq!(errors[0].reason, "Give the macro a name of 1 to 16 characters.");
    m["name"] = json!("A".repeat(17));
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].reason, "The name has 17 characters; the Pro 3 takes at most 16.");
}

#[test]
fn a_name_error_comes_first_beside_the_other_schema_errors() {
    let mut m = make_valid_macro();
    m["name"] = json!("");
    m["mode"] = json!("nope");
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors[0].path, "/name");
    assert!(errors.iter().skip(1).any(|e| e.path == "/mode"), "{errors:?}");
}

// --- Genuinely-semantic (passes schema; exact message significant) ----------

#[test]
fn button_in_both_press_and_release_fails() {
    let m = set_step_buttons(make_valid_macro(), 0, json!(["bottom face"]), json!(["bottom face"]));
    let errors = validate_macro(&m).unwrap();
    assert_ne!(errors, []);
    assert!(errors.iter().any(|e| e.reason.contains("both press and release")));
    // Exact message + path.
    assert!(errors.iter().any(|e| e.path == "/steps/0/actions/buttons"
        && e.reason == "Step 1: 'bottom face' appears in both press and release."));
}

#[test]
fn error_message_includes_step_index() {
    let mut m = make_valid_macro();
    // `Value::Array` (not `json!([...])`) sidesteps the array-of-`Value` move question.
    m["steps"] = Value::Array(vec![m["steps"][0].clone(), a_valid_step(), a_valid_step()]);
    let m = set_step_buttons(m, 2, json!(["left face"]), json!(["left face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(errors.iter().any(|e| e.reason.contains("Step 3")));
}

#[test]
fn multiple_semantic_errors_reported() {
    let mut m = make_valid_macro();
    m["steps"] = Value::Array(vec![m["steps"][0].clone(), a_valid_step()]);
    let m = set_step_buttons(m, 0, json!(["bottom face"]), json!(["bottom face"]));
    let m = set_step_buttons(m, 1, json!(["right face"]), json!(["right face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(errors.len() >= 2);
    assert!(errors.iter().any(|e| e.reason.contains("Step 1")));
    assert!(errors.iter().any(|e| e.reason.contains("Step 2")));
}

// --- Schema-specific cases --------------------------------------------------

#[test]
fn additional_properties_fails_schema() {
    let mut m = make_valid_macro();
    m["extra_field"] = json!("not allowed");
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn duplicate_button_in_press_fails_schema() {
    // uniqueItems on the press array (caught by schema, short-circuits semantic).
    let m =
        set_step_buttons(make_valid_macro(), 0, json!(["bottom face", "bottom face"]), json!([]));
    assert_ne!(validate_macro(&m).unwrap(), []);
}

#[test]
fn schema_failure_short_circuits_semantics() {
    // Schema-invalid (empty macro trigger) AND seeded with a press/release overlap
    // that the semantic phase WOULD flag if it ran. Schema short-circuits, so the
    // semantic overlap message must be absent.
    let mut m = make_valid_macro();
    m["trigger"] = json!(""); // a name needs 1+ characters -> schema rejects
    let m = set_step_buttons(m, 0, json!(["bottom face"]), json!(["bottom face"]));
    let errors = validate_macro(&m).unwrap();
    assert_ne!(errors, []);
    assert!(errors.iter().all(|e| !e.reason.contains("appears in both press and release")));
}

// --- Glides -----------------------------------------------------------------

/// Two steps: the first glides `controls`, from stick 0 / trigger 0 to `end`.
fn a_glide(mode: &str, controls: &Value, end: &Value) -> Value {
    let mut m = make_valid_macro();
    m["mode"] = mode.into();
    m["steps"] = json!([
        { "duration_ms": 100, "glide": controls, "actions": { "left_stick": { "x": 0, "y": 127 } } },
        { "duration_ms": 0, "actions": end }
    ]);
    m
}

#[test]
fn a_glide_to_a_different_value_passes() {
    let end =
        json!({ "left_stick": { "x": 255, "y": 127 }, "triggers": { "left": 0, "right": 90 } });
    let m = a_glide("xinput", &json!(["left_stick", "right_trigger"]), &end);
    assert_eq!(validate_macro(&m).unwrap(), []);
}

#[test]
fn a_glide_needs_a_next_step_and_a_value_that_moves() {
    let end = json!({ "left_stick": { "x": 255, "y": 127 } });
    let mut last = a_glide("xinput", &json!([]), &end);
    last["steps"][1]["glide"] = json!(["left_stick"]);
    let errors = validate_macro(&last).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(errors[0].path, "/steps/1/glide");
    assert!(errors[0].reason.contains("Left stick cannot glide, there is no next step"));

    let same = a_glide(
        "xinput",
        &json!(["left_stick", "right_stick"]),
        &json!({ "left_stick": { "x": 0, "y": 127 } }),
    );
    let errors = validate_macro(&same).unwrap();
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].reason.contains("Left stick has the same value"), "{}", errors[0].reason);
    assert!(errors[1].reason.contains("Right stick has the same value"), "{}", errors[1].reason);
}

#[test]
fn a_trigger_cannot_glide_in_an_on_off_mode() {
    let end = json!({ "triggers": { "left": 255, "right": 0 } });
    let m = a_glide("switch", &json!(["left_trigger"]), &end);
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].reason.contains("ZL cannot glide in Switch mode"), "{}", errors[0].reason);
    assert_eq!(validate_macro(&a_glide("xinput", &json!(["left_trigger"]), &end)).unwrap(), []);
}

#[test]
fn an_unknown_glide_control_or_a_repeat_fails_the_schema() {
    let end = json!({ "left_stick": { "x": 255, "y": 127 } });
    assert_ne!(validate_macro(&a_glide("xinput", &json!(["dpad"]), &end)).unwrap(), []);
    assert_ne!(
        validate_macro(&a_glide("xinput", &json!(["left_stick", "left_stick"]), &end)).unwrap(),
        []
    );
}

#[test]
fn the_step_count_includes_the_steps_a_glide_becomes() {
    let plain = || json!({ "duration_ms": 1, "actions": {} });
    let mut m = make_valid_macro();
    m["steps"] = json!(vec![plain(); 408]);
    assert_eq!(validate_macro(&m).unwrap(), [], "408 steps fit");
    m["steps"] = json!(vec![plain(); 409]);
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].path, "/steps");
    assert_eq!(
        errors[0].reason,
        "The macro has 409 steps; the Pro 3 holds at most 408. Remove steps."
    );

    // 398 plain steps, a 200 ms glide of 10 short steps, then its end step: 409.
    let mut steps = vec![plain(); 398];
    steps.push(json!({ "duration_ms": 200, "glide": ["left_stick"], "actions": { "left_stick": { "x": 0, "y": 127 } } }));
    steps.push(json!({ "duration_ms": 0, "actions": { "left_stick": { "x": 200, "y": 127 } } }));
    m["steps"] = json!(steps);
    let errors = validate_macro(&m).unwrap();
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert_eq!(
        errors[0].reason,
        "With its glides, this macro needs 409 steps on the controller, and the controller holds 408. Shorten a glide or remove steps."
    );
}
