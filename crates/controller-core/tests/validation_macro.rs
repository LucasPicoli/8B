//! Ports `macro_validation_service_test.cpp` (+ key `macro_schema_test.cpp` cases).
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::service::validation::validate_macro;
use serde_json::{json, Value};

/// Minimal valid macro JSON — mirrors C++ `makeValidMacro()`.
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

/// Replace a step's press/release arrays — mirrors C++ `setStepButtons()`.
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
fn valid_macro_passes_validation() {
    assert!(validate_macro(&make_valid_macro()).unwrap().is_empty());
}

// --- Schema-caught failures (C++ asserts only non-empty) ---------------------

#[test]
fn zero_steps_fails_schema() {
    let mut m = make_valid_macro();
    m["steps"] = json!([]);
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn home_guide_trigger_fails() {
    let mut m = make_valid_macro();
    m["trigger"] = json!("home/guide");
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn turbo_in_step_fails_and_mentions_press_or_turbo() {
    let m = set_step_buttons(make_valid_macro(), 0, json!(["turbo"]), json!([]));
    let errors = validate_macro(&m).unwrap();
    assert!(!errors.is_empty());
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
    m["name"] = json!("1234567890123456"); // 16 chars
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn name_empty_fails() {
    let mut m = make_valid_macro();
    m["name"] = json!("");
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn repeat_count_zero_fails() {
    let mut m = make_valid_macro();
    m["repeat"] = json!({ "count": 0, "interval_ms": 0 });
    assert!(!validate_macro(&m).unwrap().is_empty());
}

// --- Genuinely-semantic (passes schema; exact message significant) ----------

#[test]
fn button_in_both_press_and_release_fails() {
    let m = set_step_buttons(make_valid_macro(), 0, json!(["bottom face"]), json!(["bottom face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(!errors.is_empty());
    assert!(errors.iter().any(|e| e.reason.contains("both press and release")));
    // Exact ported message + path.
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

// --- Schema-specific cases ported from macro_schema_test.cpp -----------------

#[test]
fn additional_properties_fails_schema() {
    let mut m = make_valid_macro();
    m["extra_field"] = json!("not allowed");
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn duplicate_button_in_press_fails_schema() {
    // uniqueItems on the press array (caught by schema, short-circuits semantic).
    let m =
        set_step_buttons(make_valid_macro(), 0, json!(["bottom face", "bottom face"]), json!([]));
    assert!(!validate_macro(&m).unwrap().is_empty());
}

#[test]
fn schema_failure_short_circuits_semantics() {
    // Schema-invalid (unknown macro trigger) AND seeded with a press/release overlap
    // that the semantic phase WOULD flag if it ran. Schema short-circuits, so the
    // semantic overlap message must be absent.
    let mut m = make_valid_macro();
    m["trigger"] = json!("home/guide"); // not in the macro-trigger enum -> schema rejects
    let m = set_step_buttons(m, 0, json!(["bottom face"]), json!(["bottom face"]));
    let errors = validate_macro(&m).unwrap();
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|e| !e.reason.contains("appears in both press and release")));
}
