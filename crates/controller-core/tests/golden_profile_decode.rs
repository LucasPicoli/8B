//! Golden-vector tests for the Pro 3 profile decoder.
//!
//! Each test decodes a real-hardware blob fixture and compares the canonical
//! profile (as `serde_json::Value`, so key order is irrelevant) against the
//! JSON exported by the C++ tool from the same blob.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{Mode, RawProfilePayload};

fn decode_slot(blob_path: &str, slot: u8, index: u8, mode: Mode) -> serde_json::Value {
    let blob = std::fs::read(blob_path).unwrap();
    let raw = RawProfilePayload {
        payload: blob,
        source_slot: slot,
        source_profile_index: index,
        mode_hint: mode,
    };
    serde_json::to_value(Pro3.map_profile(&raw).unwrap().canonical).unwrap()
}

fn golden(path: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn xinput_slot1_decodes_to_golden_json() {
    // PRIMARY: remaps, disabled paddles, stick inversion, custom ranges.
    assert_eq!(
        decode_slot("../../fixtures/pro3/xinput.blob", 1, 0, Mode::XInput),
        golden("../../fixtures/pro3/xinput-slot1.profile.json"),
    );
}

#[test]
fn xinput_slot2_decodes_to_golden_json() {
    assert_eq!(
        decode_slot("../../fixtures/pro3/xinput.blob", 2, 1, Mode::XInput),
        golden("../../fixtures/pro3/xinput-slot2.profile.json"),
    );
}

#[test]
fn switch_slot1_decodes_to_golden_json() {
    assert_eq!(
        decode_slot("../../fixtures/pro3/switch.blob", 1, 0, Mode::Switch),
        golden("../../fixtures/pro3/switch-slot1.profile.json"),
    );
}
