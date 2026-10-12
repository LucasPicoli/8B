//! Macro edits in `ProfileWriteOrchestrator::write_slots` jobs against `MockDevice`:
//! one flip for every bank, each bank's pages after it, a put that brings its own
//! profile, and a failed bank after a written one.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
use controller_core::error::Error;
use controller_core::model::{
    MacroDefinition, MacroSlot, MacroStep, Mode, ProfileReadResult, Slot, WriteResult,
};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
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

fn def(name: &str, mode: Mode, trigger: &str, n: u8) -> MacroDefinition {
    let step = |i| MacroStep {
        duration_ms: 20,
        pressed_buttons: vec!["bottom face".into()],
        left_stick_x: i,
        ..MacroStep::default()
    };
    MacroDefinition {
        name: name.into(),
        mode,
        trigger_buttons: vec![trigger.into()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        steps: (0..n).map(step).collect(),
        macro_slot: None,
    }
}

/// An `XInput` bank: slot 1 holds a profile with `Old` (`l1`) in macro slot 0 and
/// `Keep` (`r1`) in macro slot 1.
fn xinput_bank() -> Vec<u8> {
    let macros = [
        MacroDefinition { macro_slot: Some(0), ..def("Old", XINPUT, "l1", 5) },
        MacroDefinition { macro_slot: Some(1), ..def("Keep", XINPUT, "r1", 2) },
    ];
    Pro3.compile_profile(&Pro3.default_profile(XINPUT), slot(1), &[], &macros).unwrap()
}

/// A pad with [`xinput_bank`], empty Switch and `DInput` banks, the steps of its two
/// macros, and a flip back to Switch, that keeps what it is sent.
fn pad() -> MockDevice {
    let stream = |n| Pro3.encode_macro_steps(&def("", XINPUT, "l1", n).steps, XINPUT).unwrap();
    MockDevice::new()
        .with_profiles(ProfileReadResult {
            raw_blobs: vec![xinput_bank(), vec![0; BLOB_SIZE], vec![0; BLOB_SIZE]],
            ..ProfileReadResult::default()
        })
        .with_macro_stream(XINPUT, slot(1), macro_slot(0), stream(5))
        .with_macro_stream(XINPUT, slot(1), macro_slot(1), stream(2))
        .with_flip(SWITCH)
        .keeping_writes()
}

fn put(m: u8, definition: MacroDefinition) -> MacroEdit {
    MacroEdit::Put { macro_slot: macro_slot(m), definition }
}

/// A job on `slot` of `mode` that makes `edits`, with `profile` or keeping the slot's.
fn job(mode: Mode, n: u8, profile: Option<Value>, edits: Vec<MacroEdit>) -> WriteJob {
    let op = WriteOp::Upload { profile, drop_macros: Vec::new(), macro_edits: edits };
    WriteJob { mode, slot: slot(n), op }
}

/// The default profile of `mode` as an upload.
fn profile(mode: Mode) -> Value {
    let mut profile = serde_json::to_value(Pro3.default_profile(mode)).unwrap();
    profile["macro_refs"] = serde_json::json!([]);
    profile["id"] = format!("{mode}-new").into();
    profile["name"] = "New".into();
    profile
}

fn write(dev: &MockDevice, jobs: &[WriteJob]) -> Vec<WriteResult> {
    let dir = tempfile::tempdir().unwrap();
    ProfileWriteOrchestrator::new(dev, &Pro3, dir.path()).write_slots(jobs, &ConfirmPolicy::Force)
}

/// The macro slot, name and buttons of each macro the pad holds in `slot` of `mode`.
fn held(dev: &MockDevice, mode: Mode, n: u8) -> Vec<(Option<u8>, String, Vec<String>)> {
    let read = dev.read_all_profiles().unwrap();
    let i = [XINPUT, SWITCH, DINPUT].iter().position(|&m| m == mode).unwrap();
    let macros = Pro3.decode_macro_metadata(&read.raw_blobs[i], slot(n)).unwrap();
    macros.into_iter().map(|m| (m.macro_slot, m.name, m.trigger_buttons)).collect()
}

fn page(mode: Mode, n: u8, m: u8, def: &MacroDefinition) -> MockCall {
    let stream = Pro3.encode_macro_steps(&def.steps, mode).unwrap();
    MockCall::WriteMacroStream { mode, profile_slot: slot(n), macro_slot: macro_slot(m), stream }
}

fn ops(dev: &MockDevice) -> Vec<MockOp> {
    dev.calls().iter().map(MockCall::op).collect()
}

#[test]
fn macro_edits_in_two_modes_go_out_in_one_flip_each_page_after_its_bank() {
    use MockOp::{Apply, BeginWrite, EndWrite, SlotSelect, WriteFullProfile, WriteMacroStream};
    let dev = pad();
    let (x, d) = (def("New", XINPUT, "r4", 4), def("Dee", DINPUT, "r4", 3));
    let jobs = [
        job(XINPUT, 1, None, vec![put(2, x.clone())]),
        job(DINPUT, 2, Some(profile(DINPUT)), vec![put(0, d.clone())]),
    ];
    let results = write(&dev, &jobs);
    assert!(results[0].success, "{}", results[0].message);
    assert!(results[1].success, "{}", results[1].message);
    assert_eq!(results[0].message, "Macro written.");
    assert_eq!(results[1].message, "Profile uploaded successfully.");

    let calls = dev.calls();
    assert_eq!(calls[4], page(XINPUT, 1, 2, &x));
    assert_eq!(calls[9], page(DINPUT, 2, 0, &d));
    let bank = [SlotSelect, WriteFullProfile, Apply, WriteMacroStream, Apply];
    let expected: Vec<MockOp> = [&[BeginWrite][..], &bank, &bank, &[EndWrite]].concat();
    assert_eq!(ops(&dev), expected);

    let names = |held: Vec<(Option<u8>, String, Vec<String>)>| -> Vec<String> {
        held.into_iter().map(|(_, n, _)| n).collect()
    };
    assert_eq!(names(held(&dev, XINPUT, 1)), ["Old", "Keep", "New"]);
    assert_eq!(names(held(&dev, DINPUT, 2)), ["Dee"]);
}

#[test]
fn a_put_into_an_empty_slot_needs_the_profile_in_the_same_job() {
    let dev = pad();
    let edit = || vec![put(0, def("New", XINPUT, "r4", 2))];
    let refused = write(&dev, &[job(XINPUT, 2, None, edit())]);
    assert_eq!(refused[0].message, "Write a profile to slot 2 first.");
    assert_eq!(dev.calls(), []);

    let r = write(&dev, &[job(XINPUT, 2, Some(profile(XINPUT)), edit())]);
    assert!(r[0].success, "{}", r[0].message);
    assert_eq!(held(&dev, XINPUT, 2), [(Some(0), "New".into(), vec!["r4".into()])]);
    assert_eq!(held(&dev, XINPUT, 1).len(), 2, "slot 1 keeps its macros");
}

#[test]
fn buttons_swap_between_two_macros_of_one_job() {
    let dev = pad();
    let swapped = vec![
        put(0, def("Old", XINPUT, "r1", 5)),
        put(1, def("Keep", XINPUT, "l1", 2)),
        MacroEdit::Delete { macro_slot: macro_slot(3) },
    ];
    let r = write(&dev, &[job(XINPUT, 1, None, swapped)]);
    assert!(r[0].success, "{}", r[0].message);
    assert_eq!(r[0].message, "Macros written.");
    let buttons: Vec<Vec<String>> = held(&dev, XINPUT, 1).into_iter().map(|h| h.2).collect();
    assert_eq!(buttons, [vec!["r1".to_owned()], vec!["l1".to_owned()]]);

    let twice = vec![put(2, def("A", XINPUT, "r4", 1)), put(2, def("B", XINPUT, "l4", 1))];
    let r = write(&dev, &[job(XINPUT, 1, None, twice)]);
    assert_eq!(r[0].message, "The job edits macro slot 2 twice.");
    let clash = vec![put(2, def("A", XINPUT, "r4", 1)), put(3, def("B", XINPUT, "r4", 1))];
    let r = write(&dev, &[job(XINPUT, 1, None, clash)]);
    assert!(r[0].message.contains("'r4' already starts the macro"), "{}", r[0].message);
}

#[test]
fn a_failed_page_in_the_second_bank_leaves_the_first_bank_written() {
    let dev = pad().fail_nth(MockOp::WriteMacroStream, 1, Error::write("nak"));
    let jobs = [
        job(XINPUT, 1, None, vec![put(2, def("New", XINPUT, "r4", 4))]),
        job(DINPUT, 2, Some(profile(DINPUT)), vec![put(0, def("Dee", DINPUT, "r4", 3))]),
    ];
    let results = write(&dev, &jobs);
    assert!(results[0].success, "{}", results[0].message);
    assert!(!results[1].success);
    // The job is one rollback unit: its profile goes with its macro.
    assert!(results[1].rollback_succeeded, "{}", results[1].message);
    assert_eq!(held(&dev, XINPUT, 1).len(), 3);
    assert_eq!(held(&dev, DINPUT, 2), []);
    assert_eq!(dev.calls().last(), Some(&MockCall::EndWrite(SWITCH)));
}

#[test]
fn a_lost_old_macro_is_named_in_the_result() {
    let dev = pad().fail_nth(MockOp::WriteMacroStream, 0, Error::write("nak")).fail_nth(
        MockOp::WriteMacroStream,
        1,
        Error::write("nak again"),
    );
    let r = write(&dev, &[job(XINPUT, 1, None, vec![put(0, def("New", XINPUT, "r4", 4))])]);
    assert!(!r[0].success && r[0].rollback_attempted && !r[0].rollback_succeeded);
    assert_eq!(r[0].macro_backup_paths.len(), 1);
    assert!(r[0].message.contains(&r[0].macro_backup_paths[0]), "{}", r[0].message);
}

#[test]
fn a_profile_over_an_occupied_slot_asks_once_and_names_the_macros_it_loses() {
    use std::sync::{Arc, Mutex};
    let dev = pad();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&asked);
    let policy = ConfirmPolicy::Ask(Box::new(move |w| {
        seen.lock().unwrap().push(w.to_owned());
        true
    }));
    let delete = MacroEdit::Delete { macro_slot: macro_slot(1) };
    let edits =
        vec![put(0, def("New", XINPUT, "l1", 3)), delete, put(2, def("Add", XINPUT, "r4", 2))];
    let dir = tempfile::tempdir().unwrap();
    let r = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path())
        .write_slots(&[job(XINPUT, 1, Some(profile(XINPUT)), edits)], &policy);
    assert!(r[0].success, "{}", r[0].message);
    let asked = asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert!(asked[0].contains("'Old' in macro slot 0"), "{}", asked[0]);
    assert!(asked[0].contains("'Keep' in macro slot 1"), "{}", asked[0]);

    // Without --force the refusal names them too, and nothing goes out.
    let dev = pad();
    let edits = vec![MacroEdit::Delete { macro_slot: macro_slot(0) }];
    let r = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path())
        .write_slots(&[job(XINPUT, 1, Some(profile(XINPUT)), edits)], &ConfirmPolicy::Abort);
    assert!(!r[0].success);
    assert!(r[0].message.contains("'Old' in macro slot 0"), "{}", r[0].message);
    assert!(!r[0].message.contains("'Keep'"), "{}", r[0].message);
    assert!(!ops(&dev).contains(&MockOp::WriteFullProfile));
}
