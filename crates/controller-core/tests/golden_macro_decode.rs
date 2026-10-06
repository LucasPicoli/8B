//! Golden-vector tests for the Pro 3 macro decoder.
//!
//! `macro_steps_decode_to_golden_json` walks the 32-byte step stream produced by
//! the C++ encoder, rebuilds the macro and compares its canonical JSON (as
//! `serde_json::Value`, so key order is irrelevant) against the C++-exported
//! fixture. `macro_metadata_decodes_from_section4` exercises the Section-4
//! descriptor scan against a real 2348-byte profile blob.

// Test module: panic-free lints are relaxed for assertions.
// `indexing_slicing` is allowed because `serde_json::Value` indexing
// (`expected["repeat"]["count"]`) is the idiomatic way to read fixtures.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{macros::macro_to_canonical_json, Pro3};
use controller_core::model::{MacroDefinition, MacroRef, MacroStep, Mode, RawProfilePayload, Slot};

#[test]
fn macro_steps_decode_to_golden_json() {
    let stream = std::fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();
    let expected: serde_json::Value =
        serde_json::from_slice(&std::fs::read("../../fixtures/pro3/macro-sample.json").unwrap())
            .unwrap();
    let step_count = expected["steps"].as_array().unwrap().len();
    let steps = Pro3.decode_macro_steps(&stream, step_count, Mode::XInput).unwrap();
    let def = MacroDefinition {
        name: expected["name"].as_str().unwrap_or("").to_owned(),
        mode: Mode::XInput,
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
    assert_eq!(read(Mode::XInput), vec![expected]);
    let dinput = read(Mode::DInput);
    assert_eq!(dinput.len(), 1, "every mode reads its descriptors");
    assert_eq!(dinput[0].path, "dinput-slot1-macro0-GoldenMac.json");
}
