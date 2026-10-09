//! Profile validation: schema and semantic rules. Schemas are embedded
//! (`include_str!`), so there is no schema-not-loaded case.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec as _;
use controller_core::devices::pro3::{Pro3, XINPUT};
use controller_core::model::{CanonicalProfile, CanonicalProfileSummary};
use controller_core::service::validation::{validate_all_profiles, validate_profile};
use serde_json::{json, Value};

/// Minimal valid xinput profile JSON (empty `button_mappings` is schema-valid; mappings added only where tested).
fn make_valid_xinput_profile() -> Value {
    json!({
        "id": "xinput-slot-1-index-0", "name": "TestProfile", "version": 1,
        "kind": "8bitdo.pro3.profile", "device": "8bitdo-pro3", "mode": "xinput",
        "sticks": {
            "left_min_pct": 0, "left_max_pct": 100, "right_min_pct": 0, "right_max_pct": 100,
            "invert_left_x": false, "invert_left_y": false, "invert_right_x": false,
            "invert_right_y": false, "swap_sticks": false, "swap_dpad_with_left_stick": false
        },
        "triggers": {
            "left_min_pct": 0, "left_max_pct": 100, "right_min_pct": 0, "right_max_pct": 100,
            "swap_triggers": false
        },
        "vibration": { "left_level": 5, "right_level": 5 },
        "button_mappings": [], "macro_refs": []
    })
}

/// Minimal valid switch profile JSON.
fn make_valid_switch_profile() -> Value {
    let mut p = make_valid_xinput_profile();
    p["id"] = json!("switch-slot-1-index-0");
    p["mode"] = json!("switch");
    p["triggers"] =
        json!({ "left_threshold_pct": 0, "right_threshold_pct": 0, "swap_triggers": false });
    p
}

#[test]
fn valid_xinput_profile_passes() {
    let r = validate_profile(&Pro3, &make_valid_xinput_profile()).unwrap();
    assert!(r.valid, "errors: {:?}", r.errors);
    assert_eq!(r.errors, []);
    assert_eq!(r.profile_id, "xinput-slot-1-index-0");
}

#[test]
fn valid_switch_profile_passes() {
    let r = validate_profile(&Pro3, &make_valid_switch_profile()).unwrap();
    assert!(r.valid, "errors: {:?}", r.errors);
}

#[test]
fn missing_required_field_fails_schema() {
    let mut p = make_valid_xinput_profile();
    p.as_object_mut().unwrap().remove("name");
    let r = validate_profile(&Pro3, &p).unwrap();
    assert!(!r.valid);
    assert!(r.errors.iter().any(|e| e.reason.to_lowercase().contains("name")));
}

#[test]
fn invalid_mode_fails_schema() {
    let mut p = make_valid_xinput_profile();
    p["mode"] = json!("gameboy");
    assert!(!validate_profile(&Pro3, &p).unwrap().valid);
}

#[test]
fn invalid_button_target_fails_schema() {
    let mut p = make_valid_xinput_profile();
    p["button_mappings"] = json!([{ "source": "right face", "target": "nonexistent_button" }]);
    assert!(!validate_profile(&Pro3, &p).unwrap().valid);
}

#[test]
fn duplicate_macro_triggers_fails_semantic() {
    let mut p = make_valid_xinput_profile();
    p["macro_refs"] = json!([
        { "trigger": "l1", "path": "macros/macro1.json" },
        { "trigger": "l1", "path": "macros/macro2.json" }
    ]);
    let r = validate_profile(&Pro3, &p).unwrap();
    assert!(!r.valid);
    // Exact message + path (this rule passes schema and reaches semantics).
    assert!(r.errors.iter().any(|e| e.path == "/macro_refs/1/trigger"
        && e.reason == "Duplicate macro trigger 'l1'. Each trigger must be unique."));
}

#[test]
fn analog_trigger_min_max_order_passes() {
    let mut p = make_valid_xinput_profile();
    p["triggers"] = json!({
        "left_min_pct": 80, "left_max_pct": 50, "right_min_pct": 0, "right_max_pct": 100,
        "swap_triggers": false
    });
    assert!(validate_profile(&Pro3, &p).unwrap().valid);
}

#[test]
fn analog_trigger_gap_small_passes() {
    let mut p = make_valid_xinput_profile();
    p["triggers"] = json!({
        "left_min_pct": 45, "left_max_pct": 50, "right_min_pct": 0, "right_max_pct": 100,
        "swap_triggers": false
    });
    assert!(validate_profile(&Pro3, &p).unwrap().valid);
}

#[test]
fn swap_dpad_with_left_stick_excludes_swap_sticks_and_the_left_inverts() {
    let with = |flags: &[&str]| {
        let mut p = make_valid_xinput_profile();
        p["sticks"]["swap_dpad_with_left_stick"] = json!(true);
        for f in flags {
            p["sticks"][f] = json!(true);
        }
        validate_profile(&Pro3, &p).unwrap()
    };
    assert!(with(&[]).valid, "swap_dpad alone passes");
    assert!(with(&["invert_right_x", "invert_right_y"]).valid, "the right inverts have no rule");
    for flag in ["swap_sticks", "invert_left_x", "invert_left_y"] {
        let r = with(&[flag]);
        assert!(!r.valid, "swap_dpad + {flag} should fail");
        let reason = &r.errors.first().unwrap().reason;
        assert!(reason.contains(flag), "{reason}");
    }
    let r = with(&["swap_sticks", "invert_left_x"]);
    assert_eq!(r.errors.len(), 1, "one message for the pair");
    let reason = &r.errors.first().unwrap().reason;
    assert!(reason.contains("swap_sticks and invert_left_x"), "{reason}");
}

#[test]
fn validate_all_produces_correct_summary() {
    // The typed model cannot express a missing-`name` profile, so the invalid
    // member uses an out-of-range vibration level (schema max is 5).
    let valid = canonical_xinput("xinput-slot-1-index-0");
    let mut bad = canonical_xinput("xinput-slot-2-index-1");
    bad.set_setting("/vibration/left_level", (99).into());

    let summary =
        validate_all_profiles(&Pro3, &[summary_of(valid, 1, 0), summary_of(bad, 2, 1)]).unwrap();

    assert!(!summary.all_valid);
    assert_eq!(summary.results.len(), 2);
    assert!(summary.results[0].valid);
    assert!(!summary.results[1].valid);
}

// --- typed-model helpers for the batch test ---------------------------------

fn canonical_xinput(id: &str) -> CanonicalProfile {
    CanonicalProfile {
        id: id.to_owned(),
        name: "TestProfile".to_owned(),
        button_mappings: Vec::new(),
        ..Pro3.default_profile(XINPUT)
    }
}

#[test]
fn macro_ref_with_nonexistent_path_is_still_valid() {
    // Validation is pure (no file I/O): a macro_ref to a non-existent file stays valid.
    let mut p = make_valid_xinput_profile();
    p["macro_refs"] = json!([{ "trigger": "l1", "path": "macros/does-not-exist.json" }]);
    assert!(validate_profile(&Pro3, &p).unwrap().valid);
}

fn summary_of(canonical: CanonicalProfile, slot: u8, index: u8) -> CanonicalProfileSummary {
    CanonicalProfileSummary {
        id: canonical.id.clone(),
        name: canonical.name.clone(),
        mode: canonical.mode,
        source_slot: slot,
        source_profile_index: index,
        canonical,
    }
}

#[test]
fn unrecognised_target_passes_schema_in_every_mode() {
    for mut p in [make_valid_xinput_profile(), make_valid_switch_profile()] {
        p["button_mappings"] = json!([{ "source": "d-pad left", "target": "unrecognised" }]);
        let r = validate_profile(&Pro3, &p).unwrap();
        assert!(r.valid, "errors: {:?}", r.errors);
    }
}

#[test]
fn paddle_outputs_pass_schema_in_dinput_only() {
    let mut dinput = make_valid_xinput_profile();
    dinput["mode"] = json!("dinput");
    for (mut p, valid) in
        [(dinput, true), (make_valid_xinput_profile(), false), (make_valid_switch_profile(), false)]
    {
        p["button_mappings"] = json!([{ "source": "l1", "target": "rp output" }]);
        let r = validate_profile(&Pro3, &p).unwrap();
        assert_eq!(r.valid, valid, "{}: {:?}", p["mode"], r.errors);
    }
}
