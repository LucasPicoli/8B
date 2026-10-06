//! `ProfileWriteOrchestrator::write_slots` against `MockDevice`: many slots, one read,
//! one write per changed bank.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;

use controller_core::detect::is_slot_active;
use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::error::{Error, ErrorCategory};
use controller_core::model::{
    CanonicalProfile, Mode, ProfileReadResult, RawProfilePayload, Slot, WriteResult,
};
use controller_core::orchestrator::{ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::mock::{MockCall, MockDevice, MockOp};
use serde_json::{json, Value};

const FIXTURES: &str = "../../fixtures/pro3/remap";
const BLOB_SIZE: usize = 0x092C;

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

fn fixture(stem: &str) -> Value {
    serde_json::from_slice(&std::fs::read(format!("{FIXTURES}/{stem}.profile.json")).unwrap())
        .unwrap()
}

/// A bank with slots 1 and 2 filled from the fixtures of `stem`.
fn bank(stem: &str) -> Vec<u8> {
    let profile = |n: u8| -> CanonicalProfile {
        serde_json::from_value(fixture(&format!("{stem}-slot{n}"))).unwrap()
    };
    let blob = Pro3.compile_profile(&profile(1), slot(1), &[], &[]).unwrap();
    Pro3.compile_profile(&profile(2), slot(2), &blob, &[]).unwrap()
}

/// A controller with filled `XInput` and Switch banks and empty others.
fn device() -> MockDevice {
    let blobs = Mode::ALL.iter().map(|m| match m {
        Mode::XInput => bank("xinput"),
        Mode::Switch => bank("switch"),
        Mode::DInput => vec![0; BLOB_SIZE],
    });
    MockDevice::new()
        .with_profiles(ProfileReadResult { raw_blobs: blobs.collect(), ..Default::default() })
}

fn orch<'a>(dev: &'a MockDevice, dir: &'a Path) -> ProfileWriteOrchestrator<'a> {
    ProfileWriteOrchestrator::new(dev, &Pro3, dir)
}

fn upload(mode: Mode, n: u8, name: &str) -> WriteJob {
    let mut profile =
        fixture(&format!("{}-slot1", if mode == Mode::Switch { "switch" } else { "xinput" }));
    profile["name"] = json!(name);
    profile["macro_refs"] = json!([]);
    WriteJob { mode, slot: slot(n), op: WriteOp::Upload { profile, drop_macros: Vec::new() } }
}

fn clear(mode: Mode, n: u8) -> WriteJob {
    WriteJob { mode, slot: slot(n), op: WriteOp::Clear }
}

fn ops(dev: &MockDevice) -> Vec<MockOp> {
    dev.calls().iter().map(MockCall::op).collect()
}

fn written(dev: &MockDevice) -> Vec<(Mode, Vec<u8>)> {
    dev.calls()
        .into_iter()
        .filter_map(|c| match c {
            MockCall::WriteFullProfile { mode, blob } => Some((mode, blob)),
            _ => None,
        })
        .collect()
}

fn name_in(blob: &[u8], mode: Mode, n: u8) -> String {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: n,
        source_profile_index: n - 1,
        mode_hint: mode,
    };
    Pro3.map_profile(&raw).unwrap().canonical.name
}

fn all_ok(results: &[WriteResult]) {
    for r in results {
        assert!(r.success, "{}", r.message);
    }
}

#[test]
fn slots_of_one_mode_go_out_in_one_write() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let jobs = [upload(Mode::XInput, 2, "Two"), upload(Mode::XInput, 3, "Three")];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    all_ok(&results);
    assert_eq!(results.len(), 2);
    assert_eq!((results[1].mode, results[1].slot), (Mode::XInput, 3));
    assert_eq!(
        ops(&dev),
        [MockOp::BeginWrite, MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]
    );
    let (_, blob) = written(&dev).remove(0);
    assert_eq!(name_in(&blob, Mode::XInput, 2), "Two");
    assert_eq!(name_in(&blob, Mode::XInput, 3), "Three");
    assert_eq!(name_in(&blob, Mode::XInput, 1), name_in(&bank("xinput"), Mode::XInput, 1));
}

#[test]
fn a_batch_of_one_writes_what_the_single_upload_writes() {
    let job = upload(Mode::XInput, 2, "Solo");
    let WriteOp::Upload { profile, .. } = &job.op else { panic!("an upload") };
    let dir = tempfile::tempdir().unwrap();

    let single = device();
    let r = orch(&single, dir.path()).upload_profile(
        profile,
        Mode::XInput,
        slot(2),
        &ConfirmPolicy::Force,
    );
    assert!(r.success, "{}", r.message);
    let batch = device();
    all_ok(&orch(&batch, dir.path()).write_slots(&[job], &ConfirmPolicy::Force));

    assert_eq!(batch.calls(), single.calls());
}

#[test]
fn each_changed_mode_gets_its_own_select_write_and_apply_inside_one_flip() {
    let dev = device().with_flip(Mode::Switch);
    let dir = tempfile::tempdir().unwrap();
    let jobs = [
        upload(Mode::XInput, 3, "X3"),
        upload(Mode::Switch, 3, "S3"),
        upload(Mode::XInput, 1, "X1"),
    ];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    all_ok(&results);
    assert_eq!(
        dev.calls().iter().map(MockCall::op).collect::<Vec<_>>(),
        [
            MockOp::BeginWrite,
            MockOp::SlotSelect,
            MockOp::WriteFullProfile,
            MockOp::Apply,
            MockOp::SlotSelect,
            MockOp::WriteFullProfile,
            MockOp::Apply,
            MockOp::EndWrite,
        ]
    );
    assert_eq!(dev.calls().last(), Some(&MockCall::EndWrite(Mode::Switch)));
    let blobs = written(&dev);
    assert_eq!(blobs.iter().map(|(m, _)| *m).collect::<Vec<_>>(), [Mode::XInput, Mode::Switch]);
    assert_eq!(name_in(&blobs[0].1, Mode::XInput, 3), "X3");
    assert_eq!(name_in(&blobs[0].1, Mode::XInput, 1), "X1");
    assert_eq!(name_in(&blobs[1].1, Mode::Switch, 3), "S3");
}

#[test]
fn clear_and_upload_share_a_bank() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let jobs = [clear(Mode::XInput, 1), upload(Mode::XInput, 3, "Three")];

    all_ok(&orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force));

    let (_, blob) = written(&dev).remove(0);
    assert!(!is_slot_active(&blob, slot(1)).unwrap());
    assert!(is_slot_active(&blob, slot(2)).unwrap());
    assert!(is_slot_active(&blob, slot(3)).unwrap());
}

#[test]
fn clearing_only_empty_slots_does_not_touch_the_controller() {
    let dev = device().with_flip(Mode::Switch);
    let dir = tempfile::tempdir().unwrap();

    let results =
        orch(&dev, dir.path()).write_slots(&[clear(Mode::XInput, 3)], &ConfirmPolicy::Force);

    all_ok(&results);
    assert!(results[0].message.contains("already empty"), "{}", results[0].message);
    assert_eq!(dev.calls(), []);
}

#[test]
fn an_invalid_job_stops_the_batch_before_anything_is_sent() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let mut bad = upload(Mode::Switch, 3, "Bad");
    if let WriteOp::Upload { profile, .. } = &mut bad.op {
        profile["sticks"]["left_max_pct"] = json!(500);
    }
    let jobs = [upload(Mode::XInput, 3, "Fine"), bad];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    assert_eq!(results.len(), 2);
    for r in &results {
        assert!(!r.success);
        assert_eq!(r.error_category, ErrorCategory::ValidationFailure);
        assert!(
            r.message.starts_with("Slot 3 in switch mode: Profile validation failed"),
            "{}",
            r.message
        );
    }
    assert_eq!((results[0].mode, results[0].slot), (Mode::XInput, 3));
    assert_eq!(dev.calls(), []);
}

#[test]
fn a_declined_overwrite_stops_the_batch_before_anything_is_sent() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let jobs = [upload(Mode::XInput, 3, "Empty"), upload(Mode::XInput, 2, "Taken")];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Abort);

    assert!(results.iter().all(|r| !r.success && r.error_category == ErrorCategory::None));
    assert!(results[0].message.contains("Use --force"), "{}", results[0].message);
    assert_eq!(dev.calls(), []);
}

#[test]
fn a_slot_named_twice_is_refused() {
    let dev = device();
    let dir = tempfile::tempdir().unwrap();
    let jobs = [upload(Mode::XInput, 3, "A"), upload(Mode::XInput, 3, "B")];
    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);
    assert!(results.iter().all(|r| !r.success));
    assert_eq!(dev.calls(), []);
}

#[test]
fn a_failed_bank_is_rolled_back_and_the_banks_before_it_stay_written() {
    let dev = device().fail_nth(MockOp::WriteFullProfile, 1, Error::write("chunk refused"));
    let dir = tempfile::tempdir().unwrap();
    let jobs = [
        upload(Mode::XInput, 3, "X3"),
        upload(Mode::Switch, 1, "S1"),
        upload(Mode::Switch, 3, "S3"),
    ];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    assert!(results[0].success, "{}", results[0].message);
    for r in &results[1..] {
        assert!(!r.success && r.rollback_succeeded, "{}", r.message);
        assert_eq!(r.error_category, ErrorCategory::WriteFailure);
    }
    assert_eq!(results[2].slot, 3);
    // The rollback sends the Switch bank as it was read.
    let blobs = written(&dev);
    assert_eq!(blobs.len(), 3);
    assert_eq!(blobs[2], (Mode::Switch, bank("switch")));
}

#[test]
fn banks_after_a_failure_are_not_written() {
    let dev = device().fail_nth(MockOp::WriteFullProfile, 0, Error::write("chunk refused"));
    let dir = tempfile::tempdir().unwrap();
    let jobs = [upload(Mode::XInput, 3, "X3"), upload(Mode::Switch, 3, "S3")];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    assert!(!results[0].success && !results[1].success);
    assert_eq!(results[1].message, "Not written: an earlier slot failed.");
    assert_eq!(written(&dev).iter().filter(|(m, _)| *m == Mode::Switch).count(), 0);
}

#[test]
fn a_controller_that_stays_flipped_is_reported_on_every_result() {
    let dev = device().with_flip(Mode::XInput).fail_nth(MockOp::EndWrite, 0, Error::Timeout);
    let dir = tempfile::tempdir().unwrap();
    let jobs = [upload(Mode::XInput, 3, "X3"), upload(Mode::XInput, 2, "X2")];

    let results = orch(&dev, dir.path()).write_slots(&jobs, &ConfirmPolicy::Force);

    for r in &results {
        assert!(r.success && r.message.contains("unplug it and plug it back in"), "{}", r.message);
    }
}

#[test]
fn an_empty_batch_does_nothing() {
    let dev = MockDevice::new();
    let dir = tempfile::tempdir().unwrap();
    assert!(orch(&dev, dir.path()).write_slots(&[], &ConfirmPolicy::Force).is_empty());
    assert_eq!(dev.calls(), []);
}
