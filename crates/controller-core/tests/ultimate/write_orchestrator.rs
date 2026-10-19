//! The Ultimate on `MockDevice`: both images decode, a write is read back and kept, a
//! read-back that differs is rolled back, and a macro goes into the image itself.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::{d, ultimate};
use controller_core::devices::ultimate::macros::decode_macros;
use controller_core::devices::ultimate::profile::DEVICE;
use controller_core::devices::ultimate::tables::{MACROS_LEN, MACROS_OFFSET};
use controller_core::devices::ultimate::{SWITCH, XINPUT};
use controller_core::error::ErrorCategory;
use controller_core::model::macros::macro_to_json;
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroSlot, MacroStep, Mode, ProfileReadResult,
    RawProfilePayload, Slot,
};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::mock::{MockCall, MockDevice, MockOp};

fn image(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../fixtures/ultimate/{name}.bin")).unwrap()
}

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

/// A pad that holds the first `XInput` read and the saved Switch image.
fn device() -> MockDevice {
    let raw_blobs = vec![image("xinput-read"), image("switch-saved")];
    MockDevice::new()
        .with_model(ultimate())
        .with_profiles(ProfileReadResult { raw_blobs, ..Default::default() })
}

fn decode(blob: &[u8], mode: Mode, n: u8) -> controller_core::Result<CanonicalProfile> {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: n,
        source_profile_index: n - 1,
        mode_hint: mode,
    };
    ultimate().map_profile(&raw).map(|s| s.canonical)
}

fn target<'a>(profile: &'a CanonicalProfile, source: &str) -> &'a str {
    let mapping = profile.button_mappings.iter().find(|m| m.source == source).unwrap();
    mapping.target.as_str()
}

fn ops(dev: &MockDevice) -> Vec<MockOp> {
    dev.calls().iter().map(MockCall::op).collect()
}

fn writes(dev: &MockDevice) -> Vec<Vec<u8>> {
    let written = dev.calls().into_iter().filter_map(|c| match c {
        MockCall::WriteFullProfile { blob, .. } => Some(blob),
        _ => None,
    });
    written.collect()
}

#[test]
fn both_images_decode_and_an_image_of_another_mode_is_refused() {
    let xinput = image("xinput-read");
    let switch = image("switch-saved");
    assert_eq!(target(&decode(&xinput, XINPUT, 1).unwrap(), "star"), "auto turbo");
    assert_eq!(target(&decode(&switch, SWITCH, 1).unwrap(), "star"), "screenshot");
    let refused = decode(&xinput, SWITCH, 1).unwrap_err().to_string();
    assert!(refused.contains("does not match the switch mode"), "{refused}");
    assert!(decode(&switch, XINPUT, 1).is_err());
}

#[test]
fn a_remap_is_written_read_back_and_kept() {
    let dev = device().keeping_writes();
    let dir = tempfile::tempdir().unwrap();
    let r = ProfileWriteOrchestrator::new(&dev, ultimate(), dir.path()).remap_button(
        XINPUT,
        slot(1),
        "p1",
        "turbo",
    );

    assert!(r.success, "{}", r.message);
    let expected =
        [MockOp::BeginWrite, MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply];
    assert_eq!(ops(&dev), [&expected[..], &[MockOp::EndWrite]].concat());
    let written = writes(&dev).remove(0);
    let before = image("xinput-read");
    assert_eq!(target(&decode(&written, XINPUT, 1).unwrap(), "p1"), "turbo");
    assert_eq!(target(&decode(&written, XINPUT, 1).unwrap(), "star"), "auto turbo");
    // The two slots not in use go back byte for byte.
    for n in [2, 3] {
        assert!(!ultimate().slot_active(&written, slot(n)).unwrap());
    }
    let s = MACROS_OFFSET + MACROS_LEN;
    assert_eq!(written[s..], before[s..], "slots 2 and 3 of the macro sections");
}

#[test]
fn a_read_back_that_differs_is_rolled_back() {
    // The mock keeps no write, so the read-back still holds the old slot 1.
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let r = ProfileWriteOrchestrator::new(&dev, ultimate(), dir.path()).remap_button(
        XINPUT,
        slot(1),
        "p1",
        "turbo",
    );

    assert!(!r.success);
    assert_eq!(r.error_category, ErrorCategory::WriteFailure);
    assert!(r.rollback_attempted && r.rollback_succeeded, "{}", r.message);
    assert!(r.message.starts_with("Read-back check failed"), "{}", r.message);
    assert_eq!(
        ops(&dev),
        [
            MockOp::BeginWrite,
            MockOp::SlotSelect,
            MockOp::WriteFullProfile,
            MockOp::Apply,
            MockOp::WriteFullProfile,
            MockOp::Apply,
            MockOp::EndWrite
        ]
    );
    assert_eq!(writes(&dev)[1], image("xinput-read"), "the rollback writes the image as read");
}

#[test]
fn a_macro_goes_into_the_image_without_a_step_page() {
    let dev = device().keeping_writes();
    let dir = tempfile::tempdir().unwrap();
    let step = MacroStep { duration_ms: 100, ..MacroStep::default() };
    let def = MacroDefinition {
        name: String::new(),
        mode: XINPUT,
        trigger_buttons: vec!["p2".to_owned()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        steps: vec![
            MacroStep { pressed_buttons: vec!["right face".to_owned()], ..step.clone() },
            MacroStep { left_stick_x: 255, left_stick_y: 0, ..step },
        ],
        macro_slot: Some(0),
    };
    let r = ProfileWriteOrchestrator::new(&dev, ultimate(), dir.path()).put_macro(
        &macro_to_json(&def, DEVICE),
        XINPUT,
        slot(1),
        MacroSlot::new(0).unwrap(),
        &ConfirmPolicy::Force,
    );

    assert!(r.success, "{}", r.message);
    assert!(!ops(&dev).contains(&MockOp::WriteMacroStream));
    let held = decode_macros(d(), &writes(&dev).remove(0), slot(1)).unwrap();
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].trigger_buttons, ["p2"]);
    assert_eq!(held[0].steps.len(), 2);
    assert_eq!((held[0].steps[1].left_stick_x, held[0].steps[1].left_stick_y), (255, 0));
}
