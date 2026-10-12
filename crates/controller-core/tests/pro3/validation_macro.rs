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
    assert!(errors.iter().any(|e| e.path == "/name" && e.reason.contains("1-16")));
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
        && e.reason == "Step 0: 'bottom face' appears in both press and release."));
}

#[test]
fn error_message_includes_step_index() {
    let mut m = make_valid_macro();
    // `Value::Array` (not `json!([...])`) sidesteps the array-of-`Value` move question.
    m["steps"] = Value::Array(vec![m["steps"][0].clone(), a_valid_step(), a_valid_step()]);
    let m = set_step_buttons(m, 2, json!(["left face"]), json!(["left face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(errors.iter().any(|e| e.reason.contains("Step 2")));
}

#[test]
fn multiple_semantic_errors_reported() {
    let mut m = make_valid_macro();
    m["steps"] = Value::Array(vec![m["steps"][0].clone(), a_valid_step()]);
    let m = set_step_buttons(m, 0, json!(["bottom face"]), json!(["bottom face"]));
    let m = set_step_buttons(m, 1, json!(["right face"]), json!(["right face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(errors.len() >= 2);
    assert!(errors.iter().any(|e| e.reason.contains("Step 0")));
    assert!(errors.iter().any(|e| e.reason.contains("Step 1")));
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
