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
use controller_core::model::{MacroDefinition, Mode, Slot};

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
    assert!(m.steps.is_empty()); // metadata decode leaves steps empty (filled from the stream)
}
