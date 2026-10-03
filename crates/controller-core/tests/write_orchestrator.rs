//! `ProfileWriteOrchestrator` against `MockDevice`: the whole write pipeline, rollback
//! included, plus the guarantee that every write keeps the controller's macros.
#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use controller_core::detect::is_slot_active;
use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::error::{Error, ErrorCategory};
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroSlot, MacroStep, Mode, ProfileReadResult,
    RawProfilePayload, Slot, Triggers, WriteResult,
};
use controller_core::orchestrator::{
    ProfileWriteOrchestrator, StickPatch, TriggerPatch, REFUSE_DINPUT_WRITES,
};
use controller_core::protocol::crc16::crc16_modbus;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::mock::{MockCall, MockDevice, MockOp};
use serde_json::{json, Value};

const FIXTURES: &str = "../../fixtures/pro3/remap";
const BLOB_SIZE: usize = 0x092C;
const SECTION4: usize = 0x068C;
const SECTION4_STRIDE: usize = 216;
const DESCRIPTORS: usize = 8;
const DESCRIPTOR_SIZE: usize = 52;
/// Offset of the CRC field in a blob.
const CRC: usize = 0x000C;

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

fn fixture(stem: &str) -> Value {
    serde_json::from_slice(&std::fs::read(format!("{FIXTURES}/{stem}.profile.json")).unwrap())
        .unwrap()
}

fn macro_def(name: &str, trigger: &str, macro_slot: u8) -> MacroDefinition {
    MacroDefinition {
        name: name.into(),
        mode: Mode::XInput,
        trigger: trigger.into(),
        repeat_count: 1,
        interval_ms: 10,
        steps: vec![MacroStep::default(); 2],
        macro_slot: Some(macro_slot),
    }
}

fn section4(blob: &[u8], slot_index: usize) -> &[u8] {
    let at = SECTION4 + slot_index * SECTION4_STRIDE;
    &blob[at..at + SECTION4_STRIDE]
}

/// A controller readback: slots 1 and 2 hold profiles with macros, slot 3 is empty but
/// still carries a stale macro descriptor (what a deactivated slot leaves behind). Slot 1's
/// first descriptor has bytes the canonical macro model cannot carry.
fn base_blob(mode: Mode) -> Vec<u8> {
    let (first, second) = match mode {
        Mode::Switch => ("switch-slot1", "switch-slot2"),
        _ => ("xinput-slot1", "xinput-slot2"),
    };
    let profile =
        |stem: &str| -> CanonicalProfile { serde_json::from_value(fixture(stem)).unwrap() };
    let mut blob = Pro3
        .compile_profile(&profile(first), slot(1), &[], &[macro_def("Alpha", "l1", 0)])
        .unwrap();
    blob = Pro3
        .compile_profile(&profile(second), slot(2), &blob, &[macro_def("Beta", "r1", 1)])
        .unwrap();
    let first_descriptor = SECTION4 + DESCRIPTORS;
    blob[first_descriptor + 32] = 1; // gamepad mode byte the decoder reads as Switch
    blob[first_descriptor + 33] = 0xAB; // byte the encoder always writes as 0
    let stale = Pro3
        .encode_macro_metadata(&macro_def("Stale", "l2", 2), MacroSlot::new(2).unwrap())
        .unwrap();
    let at = SECTION4 + 2 * SECTION4_STRIDE + DESCRIPTORS + 2 * DESCRIPTOR_SIZE;
    blob[at..at + DESCRIPTOR_SIZE].copy_from_slice(&stale);
    blob
}

fn device(mode: Mode, blob: &[u8]) -> MockDevice {
    let blobs = if mode == Mode::Switch {
        vec![vec![0; BLOB_SIZE], blob.to_vec()]
    } else {
        vec![blob.to_vec(), vec![0; BLOB_SIZE]]
    };
    MockDevice::new()
        .with_profiles(mode, ProfileReadResult { raw_blobs: blobs, ..Default::default() })
}

fn orch<'a>(dev: &'a MockDevice, dir: &'a Path) -> ProfileWriteOrchestrator<'a> {
    ProfileWriteOrchestrator::new(dev, &Pro3, dir)
}

fn ops(dev: &MockDevice) -> Vec<MockOp> {
    dev.calls().iter().map(MockCall::op).collect()
}

fn writes(dev: &MockDevice) -> Vec<Vec<u8>> {
    dev.calls()
        .into_iter()
        .filter_map(|c| match c {
            MockCall::WriteFullProfile { blob, .. } => Some(blob),
            _ => None,
        })
        .collect()
}

fn decode(blob: &[u8], mode: Mode, slot_number: u8) -> CanonicalProfile {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: slot_number,
        source_profile_index: slot_number - 1,
        mode_hint: mode,
    };
    Pro3.map_profile(&raw).unwrap().canonical
}

fn crc_ok(blob: &[u8]) -> bool {
    let mut b = blob.to_vec();
    let stored = u16::from_le_bytes([b[CRC], b[CRC + 1]]);
    b[CRC..CRC + 4].fill(0);
    crc16_modbus(&b) == stored
}

fn asking(answer: bool, calls: &Arc<AtomicUsize>) -> ConfirmPolicy {
    let calls = Arc::clone(calls);
    ConfirmPolicy::Ask(Box::new(move |_| {
        calls.fetch_add(1, Ordering::SeqCst);
        answer
    }))
}

fn assert_failed(r: &WriteResult, category: ErrorCategory) {
    assert!(!r.success, "expected failure, got: {}", r.message);
    assert_eq!(r.error_category, category, "{}", r.message);
}

// ---------------------------------------------------------------------------
// Codec: why the macros need a raw copy
// ---------------------------------------------------------------------------

#[test]
fn plain_compile_clears_macros_and_keep_macros_does_not() {
    let base = base_blob(Mode::XInput);
    let profile = decode(&base, Mode::XInput, 1);

    let plain = Pro3.compile_profile(&profile, slot(1), &base, &[]).unwrap();
    assert!(section4(&plain, 0)[DESCRIPTORS..].iter().all(|&b| b == 0), "plain compile clears");

    let kept = Pro3.compile_profile_keep_macros(&profile, slot(1), &base).unwrap();
    assert_eq!(section4(&kept, 0), section4(&base, 0));
    assert!(crc_ok(&kept));
}

#[test]
fn decode_and_re_encode_would_have_changed_the_macro_bytes() {
    let base = base_blob(Mode::XInput);
    let metas = Pro3.decode_macro_metadata(&base, slot(1)).unwrap();
    let re_encoded = Pro3.encode_macro_metadata(&metas[0], MacroSlot::new(0).unwrap()).unwrap();
    let raw = &section4(&base, 0)[DESCRIPTORS..DESCRIPTORS + DESCRIPTOR_SIZE];
    assert_ne!(re_encoded.as_slice(), raw);
}

#[test]
fn keep_macros_with_a_foreign_sized_base_keeps_nothing_and_still_compiles() {
    let profile = decode(&base_blob(Mode::XInput), Mode::XInput, 1);
    let blob = Pro3.compile_profile_keep_macros(&profile, slot(1), &[]).unwrap();
    assert_eq!(blob.len(), BLOB_SIZE);
    assert!(section4(&blob, 0)[DESCRIPTORS..].iter().all(|&b| b == 0));
}

// ---------------------------------------------------------------------------
// Upload
// ---------------------------------------------------------------------------

#[test]
fn upload_into_an_empty_slot_runs_the_full_pipeline() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let dir = tempfile::tempdir().unwrap();
    let json = fixture("xinput-slot2");

    let r =
        orch(&dev, dir.path()).upload_profile(&json, Mode::XInput, slot(3), &ConfirmPolicy::Abort);

    assert!(r.success, "{}", r.message);
    assert_eq!(r.profile_id, json["id"].as_str().unwrap());
    assert_eq!((r.mode, r.slot), (Mode::XInput, 3));
    assert_eq!(ops(&dev), [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]);

    let written = writes(&dev).remove(0);
    assert!(is_slot_active(&written, slot(3)).unwrap());
    let decoded = decode(&written, Mode::XInput, 3);
    let uploaded: CanonicalProfile = serde_json::from_value(json).unwrap();
    assert_eq!(decoded.name, uploaded.name);
    assert_eq!(decoded.sticks, uploaded.sticks);
    assert_eq!(decoded.triggers, uploaded.triggers);
    assert_eq!(decoded.vibration, uploaded.vibration);
    assert_eq!(decoded.button_mappings, uploaded.button_mappings);
    // The other slots are untouched, and the empty slot does not inherit its stale macro.
    assert_eq!(section4(&written, 0), section4(&base, 0));
    assert_eq!(section4(&written, 1), section4(&base, 1));
    assert!(section4(&written, 2)[DESCRIPTORS..].iter().all(|&b| b == 0));
    assert_eq!(decode(&written, Mode::XInput, 1), decode(&base, Mode::XInput, 1));
    assert_eq!(decode(&written, Mode::XInput, 2), decode(&base, Mode::XInput, 2));
}

#[test]
fn upload_rejects_bad_input_before_touching_the_device() {
    let dev = MockDevice::new(); // a readback would fail with ConnectionFailure
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());

    let mut bad = fixture("xinput-slot1");
    bad["sticks"]["left_max_pct"] = json!(500);
    let r = o.upload_profile(&bad, Mode::XInput, slot(1), &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ValidationFailure);
    assert!(r.message.starts_with("Profile validation failed"), "{}", r.message);

    let r = o.upload_profile(&json!({"x": 1}), Mode::XInput, slot(1), &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ValidationFailure);

    let r =
        o.upload_profile(&fixture("switch-slot1"), Mode::XInput, slot(1), &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ValidationFailure);
    assert!(r.message.starts_with("Mode mismatch"), "{}", r.message);

    let mut with_refs = fixture("xinput-slot1");
    with_refs["macro_refs"] = json!([{"trigger": "l1", "path": "m.json"}]);
    let r = o.upload_profile(&with_refs, Mode::XInput, slot(1), &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ValidationFailure);
    assert!(r.message.contains("macro_refs"), "{}", r.message);

    assert!(dev.calls().is_empty());
}

#[test]
fn overwrite_policy_decides_whether_an_occupied_slot_is_written() {
    let base = base_blob(Mode::XInput);
    let json = fixture("xinput-slot2");
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(AtomicUsize::new(0));

    for policy in [ConfirmPolicy::Abort, asking(false, &asked)] {
        let dev = device(Mode::XInput, &base);
        let r = orch(&dev, dir.path()).upload_profile(&json, Mode::XInput, slot(1), &policy);
        assert_failed(&r, ErrorCategory::None);
        assert!(dev.calls().is_empty());
    }
    for policy in [ConfirmPolicy::Force, asking(true, &asked)] {
        let dev = device(Mode::XInput, &base);
        let r = orch(&dev, dir.path()).upload_profile(&json, Mode::XInput, slot(1), &policy);
        assert!(r.success, "{}", r.message);
        assert_eq!(writes(&dev).len(), 1);
    }
    assert_eq!(asked.load(Ordering::SeqCst), 2);

    // An empty slot never asks.
    let dev = device(Mode::XInput, &base);
    let r =
        orch(&dev, dir.path()).upload_profile(&json, Mode::XInput, slot(3), &asking(false, &asked));
    assert!(r.success);
    assert_eq!(asked.load(Ordering::SeqCst), 2);
}

#[test]
fn upload_in_switch_mode_uses_the_switch_blob() {
    let base = base_blob(Mode::Switch);
    let dev = device(Mode::Switch, &base);
    let dir = tempfile::tempdir().unwrap();
    let r = orch(&dev, dir.path()).upload_profile(
        &fixture("switch-slot2"),
        Mode::Switch,
        slot(3),
        &ConfirmPolicy::Abort,
    );
    assert!(r.success, "{}", r.message);
    let written = writes(&dev).remove(0);
    assert_eq!(section4(&written, 0), section4(&base, 0));
}

// ---------------------------------------------------------------------------
// Macros survive every write
// ---------------------------------------------------------------------------

#[test]
fn every_write_to_an_occupied_slot_keeps_all_macros() {
    let base = base_blob(Mode::XInput);
    let dir = tempfile::tempdir().unwrap();
    let go = |f: &dyn Fn(&ProfileWriteOrchestrator<'_>) -> WriteResult| {
        let dev = device(Mode::XInput, &base);
        let r = f(&orch(&dev, dir.path()));
        assert!(r.success, "{}", r.message);
        let written = writes(&dev).pop().unwrap();
        for idx in 0..3 {
            assert_eq!(section4(&written, idx), section4(&base, idx), "slot index {idx}");
        }
        assert!(crc_ok(&written));
    };
    let f = ConfirmPolicy::Force;

    go(&|o| o.upload_profile(&fixture("xinput-slot2"), Mode::XInput, slot(1), &f));
    go(&|o| o.remap_button(Mode::XInput, slot(1), "l1", "r1", &f));
    go(&|o| {
        o.patch_sticks(
            Mode::XInput,
            slot(1),
            &StickPatch { swap_sticks: Some(true), ..StickPatch::default() },
            &f,
        )
    });
    go(&|o| {
        o.patch_triggers(
            Mode::XInput,
            slot(1),
            &TriggerPatch { swap_triggers: Some(true), ..TriggerPatch::default() },
            &f,
        )
    });
    go(&|o| o.patch_vibration(Mode::XInput, slot(1), 1, 4, &f));
    go(&|o| o.deactivate_slot(Mode::XInput, slot(1), &f));
}

// ---------------------------------------------------------------------------
// Deactivate
// ---------------------------------------------------------------------------

#[test]
fn deactivate_clears_only_the_flag_and_reseals_the_crc() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let dir = tempfile::tempdir().unwrap();

    let r = orch(&dev, dir.path()).deactivate_slot(Mode::XInput, slot(2), &ConfirmPolicy::Force);

    assert!(r.success, "{}", r.message);
    assert_eq!(ops(&dev), [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]);
    let written = writes(&dev).remove(0);
    assert!(is_slot_active(&written, slot(1)).unwrap());
    assert!(!is_slot_active(&written, slot(2)).unwrap());
    assert!(crc_ok(&written));
    let changed: Vec<usize> = (0..BLOB_SIZE).filter(|&i| written[i] != base[i]).collect();
    assert!(
        changed.iter().all(|&i| (4..8).contains(&i) || (CRC..CRC + 2).contains(&i)),
        "unexpected changes at {changed:x?}"
    );
}

#[test]
fn deactivating_an_empty_slot_succeeds_without_writing() {
    let dev = device(Mode::XInput, &base_blob(Mode::XInput));
    let dir = tempfile::tempdir().unwrap();
    let r = orch(&dev, dir.path()).deactivate_slot(Mode::XInput, slot(3), &ConfirmPolicy::Abort);
    assert!(r.success);
    assert!(r.message.contains("already empty"), "{}", r.message);
    assert!(dev.calls().is_empty());
}

// ---------------------------------------------------------------------------
// Remap and patches
// ---------------------------------------------------------------------------

#[test]
fn remap_changes_one_mapping_and_nothing_else() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let dir = tempfile::tempdir().unwrap();

    let r = orch(&dev, dir.path()).remap_button(
        Mode::XInput,
        slot(1),
        "l1",
        "r1",
        &ConfirmPolicy::Force,
    );

    assert!(r.success, "{}", r.message);
    let before = decode(&base, Mode::XInput, 1);
    let after = decode(&writes(&dev).remove(0), Mode::XInput, 1);
    assert!(after.button_mappings.iter().any(|m| m.source == "l1" && m.target == "r1"));
    assert_eq!(
        CanonicalProfile { button_mappings: vec![], ..after.clone() },
        CanonicalProfile { button_mappings: vec![], ..before.clone() }
    );
    // every other mapping is as before
    let others = |p: &CanonicalProfile| -> Vec<_> {
        p.button_mappings.iter().filter(|m| m.source != "l1").cloned().collect()
    };
    assert_eq!(others(&after), others(&before));
}

#[test]
fn remap_rejects_bad_names_before_touching_the_device() {
    let dev = MockDevice::new();
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());
    for (source, target) in
        [("home/guide", "l1"), ("nope", "l1"), ("l1", "rp"), ("l1", "screenshot")]
    {
        let r = o.remap_button(Mode::XInput, slot(1), source, target, &ConfirmPolicy::Force);
        assert_failed(&r, ErrorCategory::ValidationFailure);
    }
    assert!(dev.calls().is_empty());
}

#[test]
fn patches_on_an_empty_slot_are_refused_without_a_write() {
    let dev = device(Mode::XInput, &base_blob(Mode::XInput));
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());
    let f = ConfirmPolicy::Force;
    let results = [
        o.remap_button(Mode::XInput, slot(3), "l1", "r1", &f),
        o.patch_sticks(Mode::XInput, slot(3), &StickPatch::default(), &f),
        o.patch_triggers(Mode::XInput, slot(3), &TriggerPatch::default(), &f),
        o.patch_vibration(Mode::XInput, slot(3), 1, 1, &f),
    ];
    for r in &results {
        assert_failed(r, ErrorCategory::ValidationFailure);
        assert!(r.message.contains("empty slot"), "{}", r.message);
    }
    assert!(dev.calls().is_empty());
}

#[test]
fn patch_sticks_changes_only_the_set_fields() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let dir = tempfile::tempdir().unwrap();
    let patch = StickPatch {
        left_min_pct: Some(20),
        left_max_pct: Some(80),
        invert_right_y: Some(true),
        swap_sticks: Some(true),
        ..StickPatch::default()
    };

    let r =
        orch(&dev, dir.path()).patch_sticks(Mode::XInput, slot(1), &patch, &ConfirmPolicy::Force);

    assert!(r.success, "{}", r.message);
    let before = decode(&base, Mode::XInput, 1);
    let after = decode(&writes(&dev).remove(0), Mode::XInput, 1);
    let mut expected = before.sticks.clone();
    expected.left_min_pct = 20;
    expected.left_max_pct = 80;
    expected.invert_right_y = true;
    expected.swap_sticks = true;
    assert_eq!(after.sticks, expected);
    assert_eq!(CanonicalProfile { sticks: before.sticks.clone(), ..after }, before);
}

#[test]
fn patch_sticks_rules_run_before_any_write() {
    let dev = device(Mode::XInput, &base_blob(Mode::XInput));
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());
    let f = ConfirmPolicy::Force;

    let out_of_range = StickPatch { left_min_pct: Some(95), ..StickPatch::default() };
    assert_failed(
        &o.patch_sticks(Mode::XInput, slot(1), &out_of_range, &f),
        ErrorCategory::ValidationFailure,
    );

    let conflict = StickPatch {
        swap_dpad_with_left_stick: Some(true),
        invert_left_x: Some(true),
        ..StickPatch::default()
    };
    let r = o.patch_sticks(Mode::XInput, slot(1), &conflict, &f);
    assert_failed(&r, ErrorCategory::ValidationFailure);
    assert!(r.message.contains("D-pad"), "{}", r.message);
    assert!(dev.calls().is_empty());
}

#[test]
fn patch_triggers_is_mode_aware() {
    let dir = tempfile::tempdir().unwrap();
    let f = ConfirmPolicy::Force;

    // XInput: min and max.
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let patch =
        TriggerPatch { left_min_pct: Some(10), right_max_pct: Some(90), ..TriggerPatch::default() };
    let r = orch(&dev, dir.path()).patch_triggers(Mode::XInput, slot(1), &patch, &f);
    assert!(r.success, "{}", r.message);
    let Triggers::Analog(before) = decode(&base, Mode::XInput, 1).triggers else {
        panic!("analog")
    };
    let Triggers::Analog(after) = decode(&writes(&dev).remove(0), Mode::XInput, 1).triggers else {
        panic!("analog")
    };
    assert_eq!((after.left_min_pct, after.right_max_pct), (10, 90));
    assert_eq!(
        (after.left_max_pct, after.right_min_pct),
        (before.left_max_pct, before.right_min_pct)
    );

    // Switch: thresholds.
    let base = base_blob(Mode::Switch);
    let dev = device(Mode::Switch, &base);
    let patch = TriggerPatch {
        left_threshold_pct: Some(30),
        swap_triggers: Some(false),
        ..TriggerPatch::default()
    };
    let r = orch(&dev, dir.path()).patch_triggers(Mode::Switch, slot(1), &patch, &f);
    assert!(r.success, "{}", r.message);
    let Triggers::Switch(after) = decode(&writes(&dev).remove(0), Mode::Switch, 1).triggers else {
        panic!("switch")
    };
    assert_eq!((after.left_threshold_pct, after.swap_triggers), (30, false));
}

#[test]
fn patch_triggers_rejects_options_for_the_wrong_mode_and_bad_ranges() {
    let dev = MockDevice::new();
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());
    let f = ConfirmPolicy::Force;
    let analog = TriggerPatch { left_min_pct: Some(10), ..TriggerPatch::default() };
    let threshold = TriggerPatch { left_threshold_pct: Some(10), ..TriggerPatch::default() };
    let too_high = TriggerPatch { left_threshold_pct: Some(95), ..TriggerPatch::default() };
    assert_failed(
        &o.patch_triggers(Mode::Switch, slot(1), &analog, &f),
        ErrorCategory::ValidationFailure,
    );
    assert_failed(
        &o.patch_triggers(Mode::XInput, slot(1), &threshold, &f),
        ErrorCategory::ValidationFailure,
    );
    assert_failed(
        &o.patch_triggers(Mode::Switch, slot(1), &too_high, &f),
        ErrorCategory::ValidationFailure,
    );
    assert!(dev.calls().is_empty());
}

#[test]
fn patch_vibration_sets_both_levels_and_checks_the_range() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base);
    let dir = tempfile::tempdir().unwrap();
    let o = orch(&dev, dir.path());

    assert_failed(
        &o.patch_vibration(Mode::XInput, slot(1), 6, 1, &ConfirmPolicy::Force),
        ErrorCategory::ValidationFailure,
    );
    assert!(dev.calls().is_empty());

    let r = o.patch_vibration(Mode::XInput, slot(1), 1, 4, &ConfirmPolicy::Force);
    assert!(r.success, "{}", r.message);
    let after = decode(&writes(&dev).remove(0), Mode::XInput, 1);
    assert_eq!((after.vibration.left_level, after.vibration.right_level), (1, 4));
}

// ---------------------------------------------------------------------------
// Failures and rollback
// ---------------------------------------------------------------------------

fn write_error() -> Error {
    Error::Write { message: "nak".into(), failed_chunk: Some(4), total_chunks: 53 }
}

fn upload_over_slot1(dev: &MockDevice, dir: &Path) -> WriteResult {
    orch(dev, dir).upload_profile(
        &fixture("xinput-slot2"),
        Mode::XInput,
        slot(1),
        &ConfirmPolicy::Force,
    )
}

#[test]
fn slot_select_failure_stops_before_any_write() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base).fail_nth(MockOp::SlotSelect, 0, Error::Timeout);
    let dir = tempfile::tempdir().unwrap();
    let r = upload_over_slot1(&dev, dir.path());
    assert_failed(&r, ErrorCategory::ConnectionFailure);
    assert!(r.message.starts_with("Slot select failed"), "{}", r.message);
    assert_eq!(ops(&dev), [MockOp::SlotSelect]);
}

#[test]
fn failed_write_is_rolled_back_with_the_readback() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base).fail_nth(MockOp::WriteFullProfile, 0, write_error());
    let dir = tempfile::tempdir().unwrap();

    let r = upload_over_slot1(&dev, dir.path());

    assert_failed(&r, ErrorCategory::WriteFailure);
    assert!(r.rollback_attempted && r.rollback_succeeded, "{}", r.message);
    assert!(r.message.contains("chunk 5/53"), "{}", r.message);
    assert_eq!(
        ops(&dev),
        [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::WriteFullProfile, MockOp::Apply]
    );
    let sent = writes(&dev);
    assert_ne!(sent[0], base);
    assert_eq!(sent[1], base, "the rollback writes the original blob back");
    assert_eq!(r.profile_id, "xinput-slot-2-index-1");
}

#[test]
fn failed_rollback_saves_the_backup_file() {
    let base = base_blob(Mode::XInput);
    let dev = device(Mode::XInput, &base)
        .fail_nth(MockOp::WriteFullProfile, 0, write_error())
        .fail_nth(MockOp::WriteFullProfile, 1, write_error());
    let dir = tempfile::tempdir().unwrap();

    let r = upload_over_slot1(&dev, dir.path());

    assert_failed(&r, ErrorCategory::WriteFailure);
    assert!(r.rollback_attempted && !r.rollback_succeeded);
    let path = r.backup_file_path.unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), base);
    assert!(path.starts_with(dir.path().to_str().unwrap()));
}

#[test]
fn failed_write_into_an_empty_slot_needs_no_rollback() {
    let dev = device(Mode::XInput, &base_blob(Mode::XInput)).fail_nth(
        MockOp::WriteFullProfile,
        0,
        write_error(),
    );
    let dir = tempfile::tempdir().unwrap();
    let r = orch(&dev, dir.path()).upload_profile(
        &fixture("xinput-slot2"),
        Mode::XInput,
        slot(3),
        &ConfirmPolicy::Abort,
    );
    assert_failed(&r, ErrorCategory::WriteFailure);
    assert!(!r.rollback_attempted);
    assert_eq!(ops(&dev), [MockOp::SlotSelect, MockOp::WriteFullProfile]);
}

#[test]
fn write_that_never_reached_the_device_is_not_rolled_back() {
    let dev = device(Mode::XInput, &base_blob(Mode::XInput)).fail_nth(
        MockOp::WriteFullProfile,
        0,
        Error::Usb("cannot open".into()),
    );
    let dir = tempfile::tempdir().unwrap();
    let r = upload_over_slot1(&dev, dir.path());
    assert_failed(&r, ErrorCategory::ConnectionFailure);
    assert!(!r.rollback_attempted && r.backup_file_path.is_none());
    assert_eq!(ops(&dev), [MockOp::SlotSelect, MockOp::WriteFullProfile]);
}

#[test]
fn apply_failure_after_a_good_write_is_reported_without_rollback() {
    let dev =
        device(Mode::XInput, &base_blob(Mode::XInput)).fail_nth(MockOp::Apply, 0, Error::Timeout);
    let dir = tempfile::tempdir().unwrap();
    let r = upload_over_slot1(&dev, dir.path());
    assert_failed(&r, ErrorCategory::WriteFailure);
    assert!(r.message.contains("APPLY failed"), "{}", r.message);
    assert!(!r.rollback_attempted);
    assert_eq!(ops(&dev), [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]);
}

#[test]
fn no_device_is_a_connection_failure() {
    let dev = MockDevice::new();
    let dir = tempfile::tempdir().unwrap();
    let r = orch(&dev, dir.path()).deactivate_slot(Mode::XInput, slot(1), &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ConnectionFailure);
}

#[test]
fn dinput_writes_are_refused_while_the_switch_is_on() {
    if !REFUSE_DINPUT_WRITES {
        return; // the dinput table is verified: replace this test with real dinput write tests
    }
    let blob = base_blob(Mode::XInput);
    let dev = MockDevice::new().with_profiles(
        Mode::DInput,
        ProfileReadResult { raw_blobs: vec![blob], ..Default::default() },
    );
    let dir = tempfile::tempdir().unwrap();
    let r =
        orch(&dev, dir.path()).patch_vibration(Mode::DInput, slot(1), 1, 1, &ConfirmPolicy::Force);
    assert_failed(&r, ErrorCategory::ValidationFailure);
    assert!(dev.calls().is_empty());
}
