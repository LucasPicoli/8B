//! Hardware test of `write_slots`: requires a physical 8BitDo Pro 3 on USB. Every test
//! seeds the pad with the fixture profiles, renames some of them, and puts the banks
//! back as it found them, byte for byte.
//! Run with:
//!   `cargo test -p controller-core --features hardware --test pro3 hardware_batch -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::time::Instant;

use controller_core::devices::pro3::{Pro3, DINPUT, MODES, SWITCH, XINPUT};
use controller_core::model::{Mode, ProfileReadResult, Slot};
use controller_core::orchestrator::{ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;
use serde_json::Value;
use serial_test::serial;

use crate::hw::Banks;
use crate::seed;

fn bank_index(mode: Mode) -> usize {
    MODES.iter().position(|&m| m == mode).unwrap()
}

/// A job that renames the profile held in `slot` of `mode`.
fn rename(r: &Banks<'_>, mode: Mode, slot: u8, name: &str) -> WriteJob {
    let held = r
        .fixtures
        .profiles
        .iter()
        .find(|p| p.mode == mode && p.source_slot == slot)
        .unwrap_or_else(|| panic!("no profile in {mode} slot {slot}"));
    let mut profile = held.canonical.clone();
    name.clone_into(&mut profile.name);
    profile.macro_refs.clear();
    WriteJob {
        mode,
        slot: Slot::new(slot).unwrap(),
        op: WriteOp::Upload {
            profile: serde_json::to_value(&profile).unwrap(),
            drop_macros: Vec::new(),
        },
    }
}

fn name_of(read: &ProfileReadResult, mode: Mode, slot: u8) -> String {
    read.profiles
        .iter()
        .find(|p| p.mode == mode && p.source_slot == slot)
        .map(|p| p.name.clone())
        .unwrap()
}

fn profile_json(read: &ProfileReadResult, mode: Mode, slot: u8) -> Value {
    let p = read.profiles.iter().find(|p| p.mode == mode && p.source_slot == slot).unwrap();
    serde_json::to_value(&p.canonical).unwrap()
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn two_slots_of_one_bank_land_in_one_write() {
    let dev = seed::pad();
    let dir = tempfile::tempdir().unwrap();
    let orch = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    let r = seed::seed(&dev);
    let jobs = [rename(&r, XINPUT, 1, "bt-A"), rename(&r, XINPUT, 3, "bt-B")];

    let t = Instant::now();
    let results = orch.write_slots(&jobs, &ConfirmPolicy::Force);
    println!("batch of 2 slots, one bank: {:?}", t.elapsed());
    for res in &results {
        assert!(res.success, "{}", res.message);
    }

    let now = dev.read_all_profiles().unwrap();
    assert_eq!(name_of(&now, XINPUT, 1), "bt-A");
    assert_eq!(name_of(&now, XINPUT, 3), "bt-B");
    assert_eq!(
        profile_json(&now, XINPUT, 2),
        profile_json(&r.fixtures, XINPUT, 2),
        "the slot the batch did not name changed"
    );
    // The other banks were not touched.
    for mode in [SWITCH, DINPUT] {
        assert_eq!(now.raw_blobs[bank_index(mode)], r.fixtures.raw_blobs[bank_index(mode)]);
    }
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn slots_of_two_banks_land_in_one_session() {
    let dev = seed::pad();
    let dir = tempfile::tempdir().unwrap();
    let orch = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    let r = seed::seed(&dev);
    let jobs = [rename(&r, XINPUT, 2, "bt-X"), rename(&r, DINPUT, 2, "bt-D")];

    let t = Instant::now();
    let results = orch.write_slots(&jobs, &ConfirmPolicy::Force);
    println!("batch of 2 slots, two banks: {:?}", t.elapsed());
    for res in &results {
        assert!(res.success, "{}", res.message);
    }

    let now = dev.read_all_profiles().unwrap();
    assert_eq!(name_of(&now, XINPUT, 2), "bt-X");
    assert_eq!(name_of(&now, DINPUT, 2), "bt-D");
    assert_eq!(now.raw_blobs[bank_index(SWITCH)], r.fixtures.raw_blobs[bank_index(SWITCH)]);
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn batch_is_faster_than_one_write_per_slot() {
    let dev = seed::pad();
    let dir = tempfile::tempdir().unwrap();
    let orch = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    let r = seed::seed(&dev);
    let names = ["bt-1", "bt-2", "bt-3"];
    let jobs: Vec<WriteJob> =
        (1..=3u8).map(|n| rename(&r, XINPUT, n, names[usize::from(n) - 1])).collect();

    let t = Instant::now();
    for job in &jobs {
        let WriteOp::Upload { profile, .. } = &job.op else { panic!("an upload") };
        let res = orch.upload_profile(profile, job.mode, job.slot, &ConfirmPolicy::Force);
        assert!(res.success, "{}", res.message);
    }
    let one_by_one = t.elapsed();
    r.reseed();

    let t = Instant::now();
    for res in orch.write_slots(&jobs, &ConfirmPolicy::Force) {
        assert!(res.success, "{}", res.message);
    }
    let batched = t.elapsed();
    let now = dev.read_all_profiles().unwrap();
    for (n, name) in (1..=3u8).zip(names) {
        assert_eq!(name_of(&now, XINPUT, n), name);
    }

    println!("3 slots, one write each: {one_by_one:?}\n3 slots, one batch:      {batched:?}");
    assert!(batched < one_by_one);
}
