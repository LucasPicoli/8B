//! Hardware test of macro edits in the slot write job: requires a physical 8BitDo Pro 3
//! on USB. One `write_slots` batch carries a job per mode, each renaming the profile of
//! slot 2 and putting a macro into its macro slot 2, behind one flip. Then in every bank
//! the other slots and slot 2's neighbour macro are unchanged, the profile has its new
//! name and the macro its steps. The banks and the macro pages it wrote end as they
//! began. Run with:
//!   `cargo test -p controller-core --features hardware --test pro3 hardware_batch_macro -- --ignored`
#![allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, MODES};
use controller_core::model::macros::macro_to_json;
use controller_core::model::{Mode, RawProfilePayload, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

use crate::hardware_macro_write::{
    descriptor, macro_def, macro_slot, neighbour, restore_pages, saved_pages, slot, NEIGHBOUR,
    SLOT, TARGET,
};
use crate::seed;

/// The profile of `n` in `bank` of `mode`, read from its bytes.
fn profile_in(bank: &[u8], mode: Mode, n: u8) -> controller_core::model::CanonicalProfile {
    let raw = RawProfilePayload {
        payload: bank.to_vec(),
        source_slot: n,
        source_profile_index: n - 1,
        mode_hint: mode,
    };
    Pro3.map_profile(&raw).unwrap().canonical
}

/// The macros of every slot but [`SLOT`], and slot 2's profile, as `bank` holds them.
fn others(bank: &[u8], mode: Mode) -> Vec<String> {
    (1..=3)
        .filter(|&n| n != SLOT)
        .map(|n| {
            let held = Pro3.decode_macro_metadata(bank, Slot::new(n).unwrap()).unwrap();
            format!("{:?} {:?}", profile_in(bank, mode, n), held)
        })
        .collect()
}

#[test]
#[ignore = "writes a profile and a macro to slot 2 of every mode of an attached 8BitDo Pro 3"]
#[serial]
fn a_profile_edit_and_a_macro_put_go_out_in_one_job_per_mode() {
    let dev = seed::pad();
    let pages = saved_pages(&dev);
    let seed = seed::seed(&dev);
    let dir = tempfile::tempdir().unwrap();
    let orchestrator = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    for &mode in &MODES {
        let json = macro_to_json(&neighbour(mode), "8bitdo-pro3");
        let r = orchestrator.put_macro(
            &json,
            mode,
            slot(),
            macro_slot(NEIGHBOUR),
            &ConfirmPolicy::Force,
        );
        assert!(r.success, "{mode} neighbour: {}", r.message);
    }

    let before = dev.read_all_profiles().unwrap().raw_blobs;
    let jobs: Vec<WriteJob> = MODES
        .iter()
        .zip(&before)
        .map(|(&mode, bank)| {
            let mut profile = profile_in(bank, mode, SLOT);
            profile.name = "Batched".into();
            profile.macro_refs.clear();
            let definition = macro_def(mode, "Subject", "r4", 6);
            WriteJob {
                mode,
                slot: slot(),
                op: WriteOp::Upload {
                    profile: Some(serde_json::to_value(&profile).unwrap()),
                    drop_macros: Vec::new(),
                    macro_edits: vec![MacroEdit::Put {
                        macro_slot: macro_slot(TARGET),
                        definition,
                    }],
                },
            }
        })
        .collect();
    for r in orchestrator.write_slots(&jobs, &ConfirmPolicy::Force) {
        assert!(r.success, "{}: {}", r.mode, r.message);
    }

    let now = dev.read_all_profiles().unwrap().raw_blobs;
    for ((&mode, was), is) in MODES.iter().zip(&before).zip(&now) {
        assert_eq!(others(was, mode), others(is, mode), "{mode}: another slot changed");
        let n = descriptor(NEIGHBOUR);
        assert_eq!(was[n.clone()], is[n], "{mode}: the neighbour's descriptor changed");
        assert_eq!(profile_in(is, mode, SLOT).name, "Batched", "{mode}");
        check_steps(&dev, is, mode);
    }
    restore_pages(&dev, &pages);
    drop(seed);
}

/// The target and the neighbour of `bank` read back with the steps they were given.
fn check_steps(dev: &HidrawDevice, bank: &[u8], mode: Mode) {
    let held = Pro3.decode_macro_metadata(bank, slot()).unwrap();
    for (m, def) in [(TARGET, macro_def(mode, "Subject", "r4", 6)), (NEIGHBOUR, neighbour(mode))] {
        let got = held.iter().find(|d| d.macro_slot == Some(m)).unwrap();
        assert_eq!(got.name, def.name, "{mode}: macro slot {m}");
        let n = def.steps.len();
        let stream = dev.read_macro_stream(mode, slot(), macro_slot(m), n).unwrap();
        assert_eq!(
            Pro3.decode_macro_steps(&stream, n, mode).unwrap(),
            def.steps,
            "{mode}: macro slot {m}"
        );
    }
}
