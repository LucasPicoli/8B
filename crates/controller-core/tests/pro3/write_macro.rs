//! `put_macro` and `delete_macro` against `MockDevice`: the vendor write order, the
//! confirm policy, the refusals before any write, each rollback stage and the read-back
//! check.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::path::Path;

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, XINPUT};
use controller_core::error::{Error, ErrorCategory};
use controller_core::model::macros::{macro_from_json, macro_to_json};
use controller_core::model::{
    MacroDefinition, MacroSlot, MacroStep, ProfileReadResult, Slot, WriteResult,
};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::mock::{MockCall, MockDevice, MockOp};
use controller_core::transport::DeviceIo;
use serde_json::Value;

const BLOB_SIZE: usize = 0x092C;

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

fn macro_slot(n: u8) -> MacroSlot {
    MacroSlot::new(n).unwrap()
}

fn steps(n: u8) -> Vec<MacroStep> {
    (0..n)
        .map(|i| MacroStep {
            duration_ms: 20,
            pressed_buttons: vec!["bottom face".into()],
            left_stick_x: i,
            ..MacroStep::default()
        })
        .collect()
}

fn def(name: &str, trigger: &str, n: u8) -> MacroDefinition {
    MacroDefinition {
        name: name.into(),
        mode: XINPUT,
        trigger: trigger.into(),
        repeat_count: 1,
        interval_ms: 0,
        steps: steps(n),
        macro_slot: None,
    }
}

/// The old macro in macro slot 0 of `XInput` slot 1, the one a replace overwrites.
fn old() -> MacroDefinition {
    MacroDefinition { macro_slot: Some(0), ..def("Old", "l1", 5) }
}

/// The macro the tests put.
fn new_macro() -> Value {
    macro_to_json(&def("New", "r4", 4), "8bitdo-pro3")
}

/// An `XInput` bank: slot 1 holds a profile with `Old` in macro slot 0 and `Keep` in
/// macro slot 1. Slot 2 is empty.
fn bank() -> Vec<u8> {
    let profile = Pro3.default_profile(XINPUT);
    let blob = Pro3.compile_profile(
        &profile,
        slot(1),
        &[],
        &[old(), MacroDefinition { macro_slot: Some(1), ..def("Keep", "r1", 2) }],
    );
    blob.unwrap()
}

/// A pad that holds [`bank`] and the old macro's steps, and keeps what it is sent.
fn pad() -> MockDevice {
    let stream = Pro3.encode_macro_steps(&old().steps, XINPUT).unwrap();
    MockDevice::new()
        .with_profiles(ProfileReadResult {
            raw_blobs: vec![bank(), vec![0; BLOB_SIZE], vec![0; BLOB_SIZE]],
            ..ProfileReadResult::default()
        })
        .with_macro_stream(XINPUT, slot(1), macro_slot(0), stream)
        .keeping_writes()
}

fn put(dev: &MockDevice, m: u8, policy: &ConfirmPolicy, dir: &Path) -> WriteResult {
    ProfileWriteOrchestrator::new(dev, &Pro3, dir).put_macro(
        &new_macro(),
        XINPUT,
        slot(1),
        macro_slot(m),
        policy,
    )
}

fn delete(dev: &MockDevice, s: u8, m: u8, dir: &Path) -> WriteResult {
    ProfileWriteOrchestrator::new(dev, &Pro3, dir).delete_macro(
        XINPUT,
        slot(s),
        macro_slot(m),
        &ConfirmPolicy::Force,
    )
}

/// The bank a put of the new macro into macro slot `m` sends.
fn new_bank(m: u8) -> Vec<u8> {
    let new = macro_from_json(&new_macro()).unwrap();
    Pro3.set_macro(&bank(), slot(1), macro_slot(m), Some(&new)).unwrap()
}

fn new_page(m: u8) -> MockCall {
    let stream = Pro3.encode_macro_steps(&def("New", "r4", 4).steps, XINPUT).unwrap();
    MockCall::WriteMacroStream {
        mode: XINPUT,
        profile_slot: slot(1),
        macro_slot: macro_slot(m),
        stream,
    }
}

fn old_page() -> MockCall {
    let stream = Pro3.encode_macro_steps(&old().steps, XINPUT).unwrap();
    MockCall::WriteMacroStream {
        mode: XINPUT,
        profile_slot: slot(1),
        macro_slot: macro_slot(0),
        stream,
    }
}

const fn bank_call(blob: Vec<u8>) -> MockCall {
    MockCall::WriteFullProfile { mode: XINPUT, blob }
}

/// The calls of a put of the new macro into macro slot `m` that goes through.
fn put_calls(m: u8) -> Vec<MockCall> {
    vec![
        MockCall::BeginWrite,
        MockCall::SlotSelect(XINPUT),
        bank_call(new_bank(m)),
        MockCall::Apply(XINPUT),
        new_page(m),
        MockCall::Apply(XINPUT),
    ]
}

/// The macro slot and name of each macro the pad now holds in `XInput` slot 1.
fn names(dev: &MockDevice) -> Vec<(Option<u8>, String)> {
    let bank = dev.read_all_profiles().unwrap().raw_blobs.remove(0);
    let held = Pro3.decode_macro_metadata(&bank, slot(1)).unwrap();
    held.into_iter().map(|m| (m.macro_slot, m.name)).collect()
}

#[test]
fn a_put_into_a_free_macro_slot_writes_bank_then_page_and_keeps_the_others() {
    let (dev, dir) = (pad(), tempfile::tempdir().unwrap());
    let r = put(&dev, 2, &ConfirmPolicy::Abort, dir.path());
    assert!(r.success, "{}", r.message);
    assert_eq!(r.message, "Macro written.");
    assert_eq!(dev.calls(), put_calls(2));
    let held = names(&dev);
    assert_eq!(held, [(Some(0), "Old".into()), (Some(1), "Keep".into()), (Some(2), "New".into())]);
}

#[test]
fn a_replace_asks_the_policy_and_only_then_writes() {
    let (dev, dir) = (pad(), tempfile::tempdir().unwrap());
    let r = put(&dev, 0, &ConfirmPolicy::Abort, dir.path());
    assert!(!r.success);
    assert_eq!(r.error_category, ErrorCategory::None);
    assert!(r.message.contains("already holds the macro 'Old'"), "{}", r.message);
    assert_eq!(dev.calls(), []);

    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(r.success, "{}", r.message);
    assert_eq!(dev.calls(), put_calls(0));
    assert_eq!(names(&dev), [(Some(0), "New".into()), (Some(1), "Keep".into())]);
}

#[test]
fn nothing_is_sent_for_an_empty_profile_slot_or_a_bad_macro() {
    let (dev, dir) = (pad(), tempfile::tempdir().unwrap());
    let orchestrator = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    let to = |json: &Value, s: u8| {
        orchestrator.put_macro(json, XINPUT, slot(s), macro_slot(2), &ConfirmPolicy::Force)
    };
    assert_eq!(to(&new_macro(), 2).message, "Write a profile to slot 2 first.");
    assert_eq!(delete(&dev, 2, 0, dir.path()).message, "Write a profile to slot 2 first.");

    let mut switch = new_macro();
    switch["mode"] = "switch".into();
    assert!(to(&switch, 1).message.starts_with("Mode mismatch"));
    let mut bad = new_macro();
    bad["trigger"] = "nope".into();
    assert!(to(&bad, 1).message.starts_with("Macro validation failed: /trigger"));
    let long = macro_to_json(&def("Long", "r4", 0), "8bitdo-pro3");
    let mut long = long;
    long["steps"] = vec![new_macro()["steps"][0].clone(); 409].into();
    // The schema's own step cap refuses it first, until glides drop that cap.
    assert!(!to(&long, 1).success);
    assert_eq!(dev.calls(), []);
}

#[test]
fn a_delete_zeroes_the_descriptor_and_leaves_the_page() {
    let (dev, dir) = (pad(), tempfile::tempdir().unwrap());
    let r = delete(&dev, 1, 0, dir.path());
    assert!(r.success, "{}", r.message);
    assert_eq!(r.message, "Macro deleted.");
    let zeroed = Pro3.set_macro(&bank(), slot(1), macro_slot(0), None).unwrap();
    assert_eq!(
        dev.calls(),
        [
            MockCall::BeginWrite,
            MockCall::SlotSelect(XINPUT),
            bank_call(zeroed),
            MockCall::Apply(XINPUT)
        ]
    );
    assert_eq!(names(&dev), [(Some(1), "Keep".into())]);

    let again = delete(&dev, 1, 3, dir.path());
    assert!(again.success);
    assert_eq!(again.message, "Macro slot 3 is already empty.");
    assert_eq!(dev.calls().len(), 4, "an empty macro slot writes nothing");
}

#[test]
fn a_failed_bank_write_restores_the_bank() {
    let dir = tempfile::tempdir().unwrap();
    let dev = pad().fail_nth(MockOp::WriteFullProfile, 0, Error::write("boom"));
    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && r.rollback_succeeded, "{}", r.message);
    assert!(r.message.contains("Rollback succeeded"), "{}", r.message);
    assert_eq!(
        dev.calls(),
        [
            MockCall::BeginWrite,
            MockCall::SlotSelect(XINPUT),
            bank_call(new_bank(0)),
            bank_call(bank()),
            MockCall::Apply(XINPUT),
        ]
    );
}

#[test]
fn a_failed_erase_on_a_create_restores_the_bank_only() {
    let dir = tempfile::tempdir().unwrap();
    let dev = pad().fail_nth(MockOp::WriteMacroStream, 0, Error::write("erase rejected"));
    let r = put(&dev, 2, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && r.rollback_succeeded, "{}", r.message);
    assert_eq!(r.message, "Write failed. Rollback succeeded. Slot restored to previous state.");
    let mut calls = put_calls(2);
    calls.pop();
    calls.extend([bank_call(bank()), MockCall::Apply(XINPUT)]);
    assert_eq!(dev.calls(), calls);
    assert_eq!(names(&dev), [(Some(0), "Old".into()), (Some(1), "Keep".into())]);
}

#[test]
fn a_failed_middle_chunk_on_a_replace_puts_the_old_steps_back() {
    let dir = tempfile::tempdir().unwrap();
    let chunk = Error::Write { message: "nak".into(), failed_chunk: Some(1), total_chunks: 2 };
    let dev = pad().fail_nth(MockOp::WriteMacroStream, 0, chunk);
    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && r.rollback_succeeded, "{}", r.message);
    assert!(
        r.message.starts_with("Write failed at chunk 2/2. Rollback succeeded"),
        "{}",
        r.message
    );
    let mut calls = put_calls(0);
    calls.pop();
    calls.extend([bank_call(bank()), MockCall::Apply(XINPUT), old_page(), MockCall::Apply(XINPUT)]);
    assert_eq!(dev.calls(), calls);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn a_failed_final_apply_is_reported_without_a_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let dev = pad().fail_nth(MockOp::Apply, 1, Error::Timeout);
    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && !r.rollback_attempted);
    assert!(r.message.starts_with("Write succeeded but APPLY failed"), "{}", r.message);
    assert_eq!(dev.calls(), put_calls(0));
}

#[test]
fn a_failed_stream_rewrite_empties_the_macro_slot_and_saves_the_old_macro() {
    let dir = tempfile::tempdir().unwrap();
    let dev = pad().fail_nth(MockOp::WriteMacroStream, 0, Error::write("nak")).fail_nth(
        MockOp::WriteMacroStream,
        1,
        Error::write("nak again"),
    );
    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && r.rollback_attempted && !r.rollback_succeeded);
    assert!(r.message.contains("so macro slot 0 is now empty"), "{}", r.message);

    let emptied = Pro3.set_macro(&bank(), slot(1), macro_slot(0), None).unwrap();
    let mut calls = put_calls(0);
    calls.pop();
    calls.extend([
        bank_call(bank()),
        MockCall::Apply(XINPUT),
        old_page(),
        bank_call(emptied),
        MockCall::Apply(XINPUT),
    ]);
    assert_eq!(dev.calls(), calls);
    assert_eq!(names(&dev), [(Some(1), "Keep".into())]);

    let saved = std::fs::read_dir(dir.path()).unwrap().next().unwrap().unwrap().path();
    assert!(r.message.contains(&saved.display().to_string()), "{}", r.message);
    let name = saved.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("backup-xinput-slot-1-macro-0-"), "{name}");
    let json: Value = serde_json::from_slice(&std::fs::read(&saved).unwrap()).unwrap();
    let back = macro_from_json(&json).unwrap();
    assert_eq!((back.name.as_str(), back.steps), ("Old", old().steps));
}

#[test]
fn a_read_back_mismatch_fails_the_write_without_a_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let stream = Pro3.encode_macro_steps(&old().steps, XINPUT).unwrap();
    // A pad that takes every packet and keeps none of it.
    let dev = MockDevice::new()
        .with_profiles(ProfileReadResult {
            raw_blobs: vec![bank(), vec![0; BLOB_SIZE], vec![0; BLOB_SIZE]],
            ..ProfileReadResult::default()
        })
        .with_macro_stream(XINPUT, slot(1), macro_slot(0), stream);
    let r = put(&dev, 0, &ConfirmPolicy::Force, dir.path());
    assert!(!r.success && !r.rollback_attempted);
    assert_eq!(r.error_category, ErrorCategory::WriteFailure);
    assert!(r.message.starts_with("Read-back check failed"), "{}", r.message);
    assert_eq!(dev.calls(), put_calls(0));
}
