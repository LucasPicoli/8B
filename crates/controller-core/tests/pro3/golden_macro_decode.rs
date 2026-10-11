//! Golden-vector tests for the Pro 3 macro decoder.
//!
//! `macro_steps_decode_to_golden_json` walks a golden 32-byte step stream, rebuilds
//! the macro and compares its canonical JSON (as `serde_json::Value`, so key order is
//! irrelevant) against the golden fixture. `macro_metadata_decodes_from_section4` exercises the Section-4
//! descriptor scan against a real 2348-byte profile blob.

// Test module: panic-free lints are relaxed for assertions.
// `indexing_slicing` is allowed because `serde_json::Value` indexing
// (`expected["repeat"]["count"]`) is the idiomatic way to read fixtures.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{macros::macro_to_canonical_json, Pro3, DINPUT, XINPUT};
use controller_core::model::{MacroDefinition, MacroRef, MacroStep, RawProfilePayload, Slot};

#[test]
fn macro_steps_decode_to_golden_json() {
    let stream = std::fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read("../../fixtures/pro3/macro-sample.json").unwrap())
            .unwrap();
    let step_count = expected["steps"].as_array().unwrap().len();
    let steps = Pro3.decode_macro_steps(&stream, step_count, XINPUT).unwrap();
    let def = MacroDefinition {
        name: expected["name"].as_str().unwrap_or("").to_owned(),
        mode: XINPUT,
        trigger: expected["trigger"].as_str().unwrap().to_owned(),
        repeat_count: u32::try_from(expected["repeat"]["count"].as_u64().unwrap()).unwrap(),
        interval_ms: u32::try_from(expected["repeat"]["interval_ms"].as_u64().unwrap()).unwrap(),
        steps,
        macro_slot: Some(0),
    };
    assert_eq!(macro_to_canonical_json(&def), expected);
}

#[test]
fn macro_metadata_decodes_from_section4() {
    let blob = std::fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
    let metas = Pro3.decode_macro_metadata(&blob, Slot::new(1).unwrap()).unwrap();
    assert_eq!(metas.len(), 1);
    let m = &metas[0];
    assert_eq!(m.name, "GoldenMac");
    assert_eq!(m.trigger, "l1");
    assert_eq!(m.repeat_count, 3);
    assert_eq!(m.interval_ms, 100);
    assert_eq!(m.macro_slot, Some(0));
    assert_eq!(m.steps.len(), 3); // max_steps from descriptor
    assert!(m.steps.iter().all(|s| *s == MacroStep::default())); // all default-initialised
}

#[test]
fn profile_read_fills_macro_refs_from_section4() {
    let payload = std::fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
    let read = |mode| {
        let raw = RawProfilePayload {
            payload: payload.clone(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: mode,
        };
        Pro3.map_profile(&raw).unwrap().canonical.macro_refs
    };
    let expected = MacroRef {
        trigger: "l1".to_owned(),
        path: "xinput-slot1-macro0-GoldenMac.json".to_owned(),
    };
    assert_eq!(read(XINPUT), vec![expected]);
    let dinput = read(DINPUT);
    assert_eq!(dinput.len(), 1, "every mode reads its descriptors");
    assert_eq!(dinput[0].path, "dinput-slot1-macro0-GoldenMac.json");
}

#[test]
fn xinput_decodes_vendor_trigger_key_bits() {
    let stream = std::fs::read("../../fixtures/pro3/xinput-vendor-triggers.steps.bin").unwrap();
    let steps = Pro3.decode_macro_steps(&stream, 2, XINPUT).unwrap();
    assert_eq!((steps[0].trigger_left, steps[0].trigger_right), (0, 255), "R2 from keys bit 15");
    assert_eq!((steps[1].trigger_left, steps[1].trigger_right), (255, 0), "L2 from keys bit 14");
    assert!(steps.iter().all(|s| s.pressed_buttons.is_empty()));
}

#[test]
fn xinput_trigger_value_wins_over_key_bits() {
    // keys 0xC000 (L2 and R2 bits), trigger_value (L2 128, R2 0): L2 keeps 128, R2 reads 255.
    let record = [0x0A, 0x00, 0x00, 0xC0, 0x00, 0x80, 0x7F, 0x7F, 0x7F, 0x7F];
    let steps = Pro3.decode_macro_steps(&record, 1, XINPUT).unwrap();
    assert_eq!((steps[0].trigger_left, steps[0].trigger_right), (128, 255));
}
