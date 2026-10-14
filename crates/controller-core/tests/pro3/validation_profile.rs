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
fn a_button_in_two_macro_refs_fails_semantic() {
    let mut p = make_valid_xinput_profile();
    p["macro_refs"] = json!([
        { "trigger_buttons": ["r1", "l1"], "path": "macros/macro1.json" },
        { "trigger": "l1", "path": "macros/macro2.json" }
    ]);
    let r = validate_profile(&Pro3, &p).unwrap();
    assert!(!r.valid);
    // Exact message + path (this rule passes schema and reaches semantics).
    assert!(r.errors.iter().any(|e| e.path == "/macro_refs/1/trigger_buttons"
        && e.reason == "'l1' starts two macros. A button starts at most one macro."));
}

#[test]
fn macro_refs_take_a_button_list_or_the_legacy_trigger_but_not_both() {
    let mut p = make_valid_xinput_profile();
    p["macro_refs"] = json!([
        { "trigger_buttons": [], "path": "macros/none.json" },
        { "trigger_buttons": ["rp", "lp"], "path": "macros/two.json" },
        { "trigger": "l1", "path": "macros/legacy.json" }
    ]);
    assert!(validate_profile(&Pro3, &p).unwrap().valid);
    let profile: controller_core::model::CanonicalProfile =
        serde_json::from_value(p.clone()).unwrap();
    assert_eq!(profile.macro_refs[2].trigger_buttons, ["l1"]);
    assert_eq!(
        serde_json::to_value(&profile.macro_refs[2]).unwrap(),
        json!({ "trigger_buttons": ["l1"], "path": "macros/legacy.json" })
    );
    for refs in [
        json!([{ "trigger": "l1", "trigger_buttons": ["l1"], "path": "m.json" }]),
        json!([{ "path": "m.json" }]),
        json!([{ "trigger_buttons": ["l1", "l1"], "path": "m.json" }]),
    ] {
        p["macro_refs"] = refs.clone();
        assert!(!validate_profile(&Pro3, &p).unwrap().valid, "{refs}");
    }
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

// --- embedded macros ----------------------------------------------------------

/// Two embedded macros for an `XInput` profile, in their canonical JSON: one presses
/// buttons, the other glides the left stick and R2.
fn two_macros() -> Value {
    json!([
        {
            "mode": "xinput", "name": "Tap", "trigger_buttons": ["rp"],
            "repeat": { "count": 1, "interval_ms": 0 },
            "steps": [{ "duration_ms": 50,
                        "actions": { "buttons": { "press": ["bottom face"], "release": [] } } }],
            "macro_slot": 0
        },
        {
            "mode": "xinput", "name": "Sweep", "trigger_buttons": ["lp", "l4"],
            "repeat": { "count": 2, "interval_ms": 100 },
            "steps": [
                { "duration_ms": 100, "glide": ["left_stick", "right_trigger"],
                  "actions": { "buttons": { "press": [], "release": [] } } },
                { "duration_ms": 100,
                  "actions": { "buttons": { "press": [], "release": [] },
                               "left_stick": { "x": 255, "y": 127 },
                               "triggers": { "left": 0, "right": 255 } } }
            ],
            "macro_slot": 2
        }
    ])
}

/// The reasons `validate_profile` gives for `p`, by path.
fn errors(p: &Value) -> Vec<(String, String)> {
    let r = validate_profile(&Pro3, p).unwrap();
    r.errors.into_iter().map(|e| (e.path, e.reason)).collect()
}

#[test]
fn embedded_macros_round_trip() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    assert_eq!(errors(&p), []);
    let profile: CanonicalProfile = serde_json::from_value(p.clone()).unwrap();
    let macros = profile.macros.as_ref().unwrap();
    assert_eq!(macros.len(), 2);
    assert_eq!(macros[1].macro_slot, Some(2));
    assert_eq!(macros[1].trigger_buttons, ["lp", "l4"]);
    assert_eq!(macros[1].steps[0].glide.len(), 2);
    assert_eq!(serde_json::to_value(&profile).unwrap(), p);
}

#[test]
fn no_macros_key_differs_from_an_empty_list() {
    let mut p = make_valid_xinput_profile();
    let absent: CanonicalProfile = serde_json::from_value(p.clone()).unwrap();
    assert_eq!(absent.macros, None);
    assert!(serde_json::to_value(&absent).unwrap().get("macros").is_none());
    p["macros"] = json!([]);
    assert_eq!(errors(&p), []);
    let empty: CanonicalProfile = serde_json::from_value(p.clone()).unwrap();
    assert_eq!(empty.macros, Some(vec![]));
    assert_eq!(serde_json::to_value(&empty).unwrap(), p);
}

#[test]
fn macro_refs_may_be_absent() {
    let mut p = make_valid_xinput_profile();
    p.as_object_mut().unwrap().remove("macro_refs");
    assert_eq!(errors(&p), []);
}

#[test]
fn a_bad_embedded_macro_names_its_entry() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][1]["name"] = json!("");
    let errs = errors(&p);
    assert!(errs.iter().any(|(path, _)| path.starts_with("/macros/1/")), "{errs:?}");
    p["macros"] = two_macros();
    p["macros"][0]["steps"][0]["actions"]["buttons"]["press"] = json!(["warp"]);
    let errs = errors(&p);
    assert!(errs.iter().any(|(path, _)| path.starts_with("/macros/0/steps/0")), "{errs:?}");
}

#[test]
fn an_embedded_macro_needs_a_macro_slot_the_pad_has() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][0].as_object_mut().unwrap().remove("macro_slot");
    assert!(!validate_profile(&Pro3, &p).unwrap().valid);
    for slot in [json!(4), json!(-1), json!("0")] {
        p["macros"] = two_macros();
        p["macros"][0]["macro_slot"] = slot.clone();
        assert!(!validate_profile(&Pro3, &p).unwrap().valid, "{slot}");
    }
}

#[test]
fn two_embedded_macros_cannot_share_a_macro_slot() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][1]["macro_slot"] = json!(0);
    assert_eq!(
        errors(&p),
        [("/macros/1/macro_slot".to_owned(), "Two macros are in macro slot 0.".to_owned())]
    );
}

#[test]
fn an_embedded_macro_has_the_profiles_mode() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][0]["mode"] = json!("dinput");
    assert_eq!(
        errors(&p),
        [(
            "/macros/0/mode".to_owned(),
            "This macro is for DInput mode; the profile is for XInput mode.".to_owned()
        )]
    );
}

#[test]
fn a_button_starts_one_embedded_macro() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][1]["trigger_buttons"] = json!(["rp"]);
    assert_eq!(
        errors(&p),
        [(
            "/macros/1/trigger_buttons".to_owned(),
            "'rp' starts two macros. A button starts at most one macro.".to_owned()
        )]
    );
}

#[test]
fn macros_and_macro_refs_together_are_refused() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macro_refs"] = json!([{ "trigger_buttons": ["r1"], "path": "m.json" }]);
    assert_eq!(
        errors(&p),
        [(
            "/macro_refs".to_owned(),
            "A profile holds its macros in macros or in macro_refs, not both.".to_owned()
        )]
    );
}

/// [`two_macros`] for a `mode` profile. Switch triggers are on or off, so there the
/// second macro glides only the left stick.
fn two_macros_in(mode: &str) -> Value {
    let mut macros = two_macros();
    for entry in macros.as_array_mut().unwrap() {
        entry["mode"] = json!(mode);
    }
    if mode == "switch" {
        macros[1]["steps"][0]["glide"] = json!(["left_stick"]);
    }
    macros
}

#[test]
fn embedded_macros_round_trip_in_every_mode() {
    let mut dinput = make_valid_xinput_profile();
    dinput["mode"] = json!("dinput");
    for (mode, mut p) in [
        ("xinput", make_valid_xinput_profile()),
        ("switch", make_valid_switch_profile()),
        ("dinput", dinput),
    ] {
        p["macros"] = two_macros_in(mode);
        assert_eq!(errors(&p), [], "{mode}");
        let profile: CanonicalProfile = serde_json::from_value(p.clone()).unwrap();
        assert_eq!(serde_json::to_value(&profile).unwrap(), p, "{mode}");
    }
}

#[test]
fn a_switch_macro_cannot_glide_a_trigger() {
    let mut p = make_valid_switch_profile();
    p["macros"] = two_macros_in("switch");
    p["macros"][1]["steps"][0]["glide"] = json!(["right_trigger"]);
    assert_eq!(
        errors(&p),
        [(
            "/macros/1/steps/0/glide".to_owned(),
            "Step 1: ZR cannot glide in Switch mode, its triggers are on or off.".to_owned()
        )]
    );
}

#[test]
fn a_pasted_macro_file_is_an_entry_and_loses_its_version_and_device() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][0]["version"] = json!(1);
    p["macros"][0]["device"] = json!("8bitdo-pro3");
    assert_eq!(errors(&p), []);
    let written = serde_json::to_value(serde_json::from_value::<CanonicalProfile>(p).unwrap());
    let entry = &written.unwrap()["macros"][0];
    assert_eq!((entry.get("version"), entry.get("device")), (None, None));
}

#[test]
fn a_pasted_macro_file_for_another_device_is_refused() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    p["macros"][0]["device"] = json!("8bitdo-ultimate");
    let errs = errors(&p);
    assert!(errs.iter().any(|(path, _)| path == "/macros/0/device"), "{errs:?}");
}

#[test]
fn an_embedded_macro_may_hold_the_legacy_trigger() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    let entry = p["macros"][0].as_object_mut().unwrap();
    entry.remove("trigger_buttons");
    entry.insert("trigger".to_owned(), json!("rp"));
    assert_eq!(errors(&p), []);
    let profile: CanonicalProfile = serde_json::from_value(p).unwrap();
    assert_eq!(profile.macros.as_ref().unwrap()[0].trigger_buttons, ["rp"]);
    let written = serde_json::to_value(&profile).unwrap();
    assert_eq!(written["macros"][0]["trigger_buttons"], json!(["rp"]));
    assert_eq!(written["macros"][0].get("trigger"), None);
}

#[test]
fn a_macros_list_that_is_not_a_list_of_macros_is_refused() {
    let mut five = two_macros().as_array().unwrap().clone();
    five.extend(two_macros().as_array().unwrap().clone());
    five.push(two_macros()[0].clone());
    for (i, slot) in [0, 1, 2, 3, 0].into_iter().enumerate() {
        five[i]["macro_slot"] = json!(slot);
        five[i]["trigger_buttons"] = json!([]);
    }
    for macros in [json!(five), Value::Null, json!(["Tap"]), json!({ "0": two_macros()[0] })] {
        let mut p = make_valid_xinput_profile();
        p["macros"] = macros.clone();
        assert!(!validate_profile(&Pro3, &p).unwrap().valid, "{macros}");
    }
}

#[test]
fn a_macro_with_no_slot_writes_a_file_that_does_not_validate() {
    let mut p = make_valid_xinput_profile();
    p["macros"] = two_macros();
    let mut profile: CanonicalProfile = serde_json::from_value(p).unwrap();
    profile.macros.as_mut().unwrap()[0].macro_slot = None;
    let written = serde_json::to_value(&profile).unwrap();
    let r = validate_profile(&Pro3, &written).unwrap();
    assert!(r.errors.iter().any(|e| e.path == "/macros/0/macro_slot"), "{:?}", r.errors);
    assert!(serde_json::from_value::<CanonicalProfile>(written).is_err());
}
