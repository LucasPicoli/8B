//! Macro fixtures: encode-golden + decode round-trip + schema validation.
//! `build_macro_fixtures()` is the single source of truth (models); the
//! `regenerate` test (ignored) writes committed `.json` + `.steps.bin`.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::macros::macro_to_canonical_json;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Mode};

const DIR: &str = "../../fixtures/pro3/macros";

struct Fixture {
    stem: &'static str,
    def: MacroDefinition,
}

fn step(dur: u16, press: &[&str]) -> MacroStep {
    MacroStep {
        duration_ms: dur,
        pressed_buttons: press.iter().map(|s| (*s).to_owned()).collect(),
        ..MacroStep::default()
    }
}

#[allow(clippy::too_many_lines)]
fn build_macro_fixtures() -> Vec<Fixture> {
    // The 14 DIGITAL step buttons. l2/r2 are intentionally excluded: their
    // STEP_BUTTONS masks (0x4000/0x8000) are the L2/R2 trigger-routing bits, so
    // they are analog-only and never round-trip as digital button names. Their
    // analog form is covered by the x-s3-m2-sticks-triggers fixture.
    let digital_buttons = [
        "right face",
        "bottom face",
        "top face",
        "left face",
        "l1",
        "r1",
        "l3",
        "r3",
        "select/back",
        "start/menu",
        "d-pad up",
        "d-pad down",
        "d-pad left",
        "d-pad right",
    ];
    vec![
        Fixture {
            stem: "x-s1-m0-buttons",
            def: MacroDefinition {
                name: "Buttons".into(),
                mode: Mode::XInput,
                trigger: "rp".into(),
                repeat_count: 1,
                interval_ms: 0,
                macro_slot: Some(0),
                steps: vec![
                    step(50, &["bottom face"]),
                    step(100, &["top face", "r1"]),
                    step(0, &["d-pad up", "l1"]),
                ],
            },
        },
        Fixture {
            stem: "x-s2-m1-allbuttons",
            def: MacroDefinition {
                name: "AllButtons".into(),
                mode: Mode::XInput,
                trigger: "l1".into(),
                repeat_count: 2,
                interval_ms: 200,
                macro_slot: Some(1),
                steps: vec![step(30, &digital_buttons)],
            },
        },
        Fixture {
            stem: "x-s3-m2-sticks-triggers",
            def: MacroDefinition {
                name: "SticksTrig".into(),
                mode: Mode::XInput,
                trigger: "r4".into(),
                repeat_count: 1,
                interval_ms: 0,
                macro_slot: Some(2),
                steps: vec![
                    MacroStep {
                        duration_ms: 100,
                        left_stick_x: 200,
                        left_stick_y: 30,
                        trigger_left: 255,
                        trigger_right: 128,
                        ..MacroStep::default()
                    },
                    MacroStep {
                        duration_ms: 0,
                        right_stick_x: 64,
                        right_stick_y: 192,
                        ..MacroStep::default()
                    },
                ],
            },
        },
        Fixture {
            stem: "x-s1-m3-repeat",
            def: MacroDefinition {
                name: "Repeat255".into(),
                mode: Mode::XInput,
                trigger: "turbo".into(),
                repeat_count: 255,
                interval_ms: 1000,
                macro_slot: Some(3),
                steps: vec![step(40, &["l1"]), step(40, &["r1"])],
            },
        },
        Fixture {
            stem: "s-s1-m0-switch-routing",
            def: MacroDefinition {
                name: "SwitchL2".into(),
                mode: Mode::Switch,
                trigger: "l2".into(),
                repeat_count: 1,
                interval_ms: 0,
                macro_slot: Some(0),
                steps: vec![MacroStep {
                    duration_ms: 60,
                    trigger_left: 255,
                    trigger_right: 0,
                    ..MacroStep::default()
                }],
            },
        },
        Fixture {
            stem: "s-s2-m1-continuous",
            def: MacroDefinition {
                name: "Continuous".into(),
                mode: Mode::Switch,
                trigger: "r2".into(),
                repeat_count: u32::MAX,
                interval_ms: 16,
                macro_slot: Some(1),
                steps: vec![step(30, &["right face"]), step(30, &[])],
            },
        },
        Fixture {
            stem: "s-s3-m2-maxsteps",
            def: MacroDefinition {
                name: "MaxSteps".into(),
                mode: Mode::Switch,
                trigger: "select/back".into(),
                repeat_count: 1,
                interval_ms: 0,
                macro_slot: Some(2),
                steps: (0..255).map(|_| step(10, &["bottom face"])).collect(),
            },
        },
        Fixture {
            stem: "s-s2-m3-triggervariety",
            def: MacroDefinition {
                name: "TrigVariety".into(),
                mode: Mode::Switch,
                trigger: "start/menu".into(),
                repeat_count: 3,
                interval_ms: 100,
                macro_slot: Some(3),
                steps: vec![
                    step(25, &["d-pad left"]),
                    MacroStep {
                        duration_ms: 25,
                        left_stick_x: 10,
                        left_stick_y: 240,
                        trigger_left: 200,
                        ..MacroStep::default()
                    },
                ],
            },
        },
    ]
}

#[test]
fn macro_steps_encode_match_committed_bin() {
    for f in build_macro_fixtures() {
        let bytes = Pro3.encode_macro_steps(&f.def.steps, f.def.mode).unwrap();
        let committed = std::fs::read(format!("{DIR}/{}.steps.bin", f.stem)).unwrap();
        assert_eq!(bytes, committed, "{}: step encode drifted from committed bin", f.stem);
    }
}

#[test]
fn macro_steps_round_trip_to_committed_json() {
    for f in build_macro_fixtures() {
        // Decode the committed bytes back and rebuild canonical JSON; compare to committed JSON.
        let bytes = std::fs::read(format!("{DIR}/{}.steps.bin", f.stem)).unwrap();
        let count = f.def.steps.len();
        let mut def = f.def.clone();
        def.steps = if count == 0 {
            vec![]
        } else {
            Pro3.decode_macro_steps(&bytes, count, f.def.mode).unwrap()
        };
        let got = macro_to_canonical_json(&def);
        let expected: serde_json::Value =
            serde_json::from_slice(&std::fs::read(format!("{DIR}/{}.json", f.stem)).unwrap())
                .unwrap();
        assert_eq!(got, expected, "{}: decoded JSON != committed JSON", f.stem);
    }
}

#[test]
fn macro_metadata_descriptor_is_52_bytes_with_correct_step_count() {
    for f in build_macro_fixtures() {
        let slot = MacroSlot::new(f.def.macro_slot.unwrap()).unwrap();
        let desc = Pro3.encode_macro_metadata(&f.def, slot).unwrap();
        assert_eq!(desc.len(), 52, "{}: metadata descriptor size", f.stem);
        // max_steps LE16 @ 34 equals the step count.
        assert_eq!(
            u16::from_le_bytes([desc[34], desc[35]]),
            u16::try_from(f.def.steps.len()).unwrap()
        );
    }
}

/// Regenerate committed `.json` + `.steps.bin`. Ignored in normal runs.
/// Run: `cargo test -p controller-core --test fixture_macros regenerate -- --ignored`
#[test]
#[ignore = "writes committed fixture artifacts; run manually after model/encoder changes"]
fn regenerate() {
    for f in build_macro_fixtures() {
        let bytes = Pro3.encode_macro_steps(&f.def.steps, f.def.mode).unwrap();
        std::fs::write(format!("{DIR}/{}.steps.bin", f.stem), &bytes).unwrap();
        // The committed JSON is the round-tripped (DECODED) form, i.e. what a device
        // would actually report. Switch analog trigger values are 1-bit lossy (encode
        // stores only keys bit14/15; decode reconstructs a canonical pressed value), so
        // the decoded form is the honest golden. For XInput this equals the original.
        let mut rt = f.def.clone();
        if !rt.steps.is_empty() {
            rt.steps = Pro3.decode_macro_steps(&bytes, f.def.steps.len(), f.def.mode).unwrap();
        }
        let mut json = serde_json::to_vec_pretty(&macro_to_canonical_json(&rt)).unwrap();
        json.push(b'\n');
        std::fs::write(format!("{DIR}/{}.json", f.stem), json).unwrap();
    }
}

/// Plan 2b tie-in: every committed macro fixture is a valid macro per the
/// validation service (schema + semantic). Guards the catalog against drift
/// that would slip past the byte/round-trip checks but fail validation.
#[test]
fn every_macro_fixture_passes_validation() {
    for f in build_macro_fixtures() {
        let json: serde_json::Value =
            serde_json::from_slice(&std::fs::read(format!("{DIR}/{}.json", f.stem)).unwrap())
                .unwrap();
        let errors = controller_core::service::validation::validate_macro(&json).unwrap();
        assert!(errors.is_empty(), "{}: {:?}", f.stem, errors);
    }
}
