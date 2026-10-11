//! Hardware macro write test: requires a physical 8BitDo Pro 3 on USB. In each mode it
//! puts a neighbour macro into macro slot 0 of slot 2, then puts, replaces with a shorter
//! macro, and deletes a macro in macro slot 2, all through `ProfileWriteOrchestrator`.
//! After each write only the target descriptor and the CRC of that bank change, the
//! other banks stay byte-identical (the pad's own `cur_slot` byte left out), and the
//! neighbour's steps still read back. The banks and the macro pages it wrote end as they
//! began. Run with:
//!   `cargo test -p controller-core --features hardware --test pro3 hardware_macro_write -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use std::ops::Range;

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, MODES};
use controller_core::model::macros::macro_to_json;
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Mode, Slot};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

use crate::hw::same_bank;
use crate::seed;

/// The profile slot the test writes macros into.
const SLOT: u8 = 2;
/// The macro slot of the neighbour that must survive every write.
const NEIGHBOUR: u8 = 0;
/// The macro slot the test puts, replaces and deletes.
const TARGET: u8 = 2;
/// Section 4 starts here; each slot's record is 216 bytes with an 8-byte header.
const SECTION4: usize = 0x068C;
const SLOT_STRIDE: usize = 216;
const SLOT_HEADER: usize = 8;
/// Bytes per macro descriptor.
const DESCRIPTOR_LEN: usize = 52;
/// The blob CRC.
const CRC: Range<usize> = 0x0C..0x10;
/// The header byte the pad fills in itself (`cur_slot`).
const CUR_SLOT: usize = 0x12;
/// Bytes per macro step on the wire, and the chunk a page is written in.
const STEP_LEN: usize = 10;
const CHUNK_LEN: usize = 32;

fn slot() -> Slot {
    Slot::new(SLOT).unwrap()
}

fn macro_slot(n: u8) -> MacroSlot {
    MacroSlot::new(n).unwrap()
}

/// Where the descriptor of macro slot `m` of [`SLOT`] sits in a bank.
fn descriptor(m: u8) -> Range<usize> {
    let at = SECTION4
        + (usize::from(SLOT) - 1) * SLOT_STRIDE
        + SLOT_HEADER
        + usize::from(m) * DESCRIPTOR_LEN;
    at..at + DESCRIPTOR_LEN
}

/// `n` steps that press a button, move the left stick and pull L2 fully, so they decode
/// the same in every mode.
fn steps(n: u8) -> Vec<MacroStep> {
    let mut out: Vec<MacroStep> = (0..n - 1)
        .map(|i| MacroStep {
            duration_ms: 40,
            pressed_buttons: vec![if i % 2 == 0 { "bottom face" } else { "right face" }.into()],
            left_stick_x: 20 * i,
            trigger_left: 255,
            ..MacroStep::default()
        })
        .collect();
    out.push(MacroStep::default());
    out
}

fn macro_def(mode: Mode, name: &str, trigger: &str, n: u8) -> MacroDefinition {
    MacroDefinition {
        name: name.into(),
        mode,
        trigger: trigger.into(),
        repeat_count: 1,
        interval_ms: 0,
        steps: steps(n),
        macro_slot: None,
    }
}

fn neighbour(mode: Mode) -> MacroDefinition {
    macro_def(mode, "Neighbour", "l1", 3)
}

/// The bytes a macro of `steps` steps takes in its page, padded as a stream is.
const fn page_span(steps: usize) -> usize {
    (steps * STEP_LEN).div_ceil(CHUNK_LEN) * CHUNK_LEN
}

/// The written span of each page the test writes that a macro of the pad uses now, so
/// it can go back after the test.
fn saved_pages(dev: &HidrawDevice) -> Vec<(Mode, u8, Vec<u8>)> {
    let read = dev.read_all_profiles().unwrap();
    let mut pages = Vec::new();
    for (mode, bank) in MODES.iter().zip(&read.raw_blobs) {
        for held in Pro3.decode_macro_metadata(bank, slot()).unwrap() {
            let m = held.macro_slot.unwrap();
            let span = page_span(held.steps.len());
            if ![NEIGHBOUR, TARGET].contains(&m) || span == 0 {
                continue;
            }
            let read = dev.read_macro_stream(*mode, slot(), macro_slot(m), span.div_ceil(STEP_LEN));
            let mut page = read.unwrap();
            page.truncate(span);
            pages.push((*mode, m, page));
        }
    }
    pages
}

fn restore_pages(dev: &HidrawDevice, pages: &[(Mode, u8, Vec<u8>)]) {
    let back_to = dev.begin_write().unwrap();
    let sent = (|| {
        for (mode, m, page) in pages {
            dev.write_macro_stream(*mode, slot(), macro_slot(*m), page)?;
            dev.send_apply(*mode)?;
        }
        Ok::<_, controller_core::Error>(())
    })();
    if let Some(mode) = back_to {
        dev.end_write(mode).unwrap();
    }
    sent.unwrap();
}

#[test]
#[ignore = "writes and removes macros on slot 2 of every mode of an attached 8BitDo Pro 3"]
#[serial]
fn put_replace_and_delete_a_macro_in_every_mode() {
    let dev = seed::pad();
    let pages = saved_pages(&dev);
    let seed = seed::seed(&dev);
    let dir = tempfile::tempdir().unwrap();
    let orchestrator = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    for (bank, &mode) in MODES.iter().enumerate() {
        let put = |def: &MacroDefinition, m: u8| {
            let json = macro_to_json(def, "8bitdo-pro3");
            let r =
                orchestrator.put_macro(&json, mode, slot(), macro_slot(m), &ConfirmPolicy::Force);
            assert!(r.success, "{mode} put {}: {}", def.name, r.message);
        };
        put(&neighbour(mode), NEIGHBOUR);

        let long = macro_def(mode, "Subject", "r4", 6);
        let before = dev.read_all_profiles().unwrap().raw_blobs;
        put(&long, TARGET);
        check(&dev, &before, bank, mode, Some(&long));

        let short = macro_def(mode, "Shorter", "r4", 2);
        let before = dev.read_all_profiles().unwrap().raw_blobs;
        put(&short, TARGET);
        check(&dev, &before, bank, mode, Some(&short));

        let before = dev.read_all_profiles().unwrap().raw_blobs;
        let r = orchestrator.delete_macro(mode, slot(), macro_slot(TARGET), &ConfirmPolicy::Force);
        assert!(r.success, "{mode} delete: {}", r.message);
        check(&dev, &before, bank, mode, None);
    }
    restore_pages(&dev, &pages);
    drop(seed);
}

/// After one write to `bank`: the other banks are as `before`, only the target
/// descriptor and the CRC changed in `bank`, the target holds `want`, and the
/// neighbour's steps still read back.
fn check(
    dev: &HidrawDevice,
    before: &[Vec<u8>],
    bank: usize,
    mode: Mode,
    want: Option<&MacroDefinition>,
) {
    let now = dev.read_all_profiles().unwrap().raw_blobs;
    for (i, (was, is)) in before.iter().zip(&now).enumerate() {
        if i != bank {
            assert!(same_bank(was, is), "{mode}: bank {i} changed");
        }
    }
    let target = descriptor(TARGET);
    let (was, is) = (&before[bank], &now[bank]);
    let outside: Vec<usize> = (0..was.len())
        .filter(|&j| was[j] != is[j] && !target.contains(&j) && !CRC.contains(&j) && j != CUR_SLOT)
        .collect();
    assert!(
        outside.is_empty(),
        "{mode}: bytes outside the target descriptor changed: {outside:x?}"
    );

    let held = Pro3.decode_macro_metadata(is, slot()).unwrap();
    let find = |m: u8| held.iter().find(|d| d.macro_slot == Some(m));
    let read_steps = |m: u8, n: usize| {
        let stream = dev.read_macro_stream(mode, slot(), macro_slot(m), n).unwrap();
        Pro3.decode_macro_steps(&stream, n, mode).unwrap()
    };
    match want {
        Some(def) => {
            let got = find(TARGET).expect("the target holds a macro");
            assert_eq!((&got.name, got.steps.len()), (&def.name, def.steps.len()), "{mode}");
            assert_eq!(read_steps(TARGET, def.steps.len()), def.steps, "{mode}: target steps");
        }
        None => assert!(find(TARGET).is_none(), "{mode}: the target is empty"),
    }
    let n = neighbour(mode);
    assert_eq!(find(NEIGHBOUR).map(|d| d.name.as_str()), Some("Neighbour"), "{mode}");
    assert_eq!(read_steps(NEIGHBOUR, n.steps.len()), n.steps, "{mode}: neighbour steps");
}
