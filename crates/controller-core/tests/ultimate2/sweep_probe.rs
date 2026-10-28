//! Probe: which macro pages of the Ultimate 2 keep a long macro across mode switches?
//! In `DInput` slot 3, a macro 1 past 307 steps is erased at the next `DInput` start-up
//! (`step_probe::record_probe`). This writes four macros into each of slots 1 to 3 of
//! both modes, 24 pages: [`STEPS`] steps each, or the lower limit the description sets
//! for a macro slot. Then it reads every page after each start-up of [`TRIPS`] trips and prints
//! which held. `ULTIMATE_2_PROBE_STEPS` and `ULTIMATE_2_PROBE_TRIPS` set other numbers.
//! Run once, at the desk, with the 8B window closed:
//!   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
//!    --nocapture --exact sweep_probe::probe`
//!
//! It first saves each mode's bank and the raw bytes of its 12 pages into [`saved_dir`],
//! and refuses to run when a saved file is already there. At the end it writes every
//! page and bank back. If a run stops halfway, `sweep_probe::restore` puts back the
//! mode `ULTIMATE_2_SWEEP_MODE` names (`xinput` or `dinput`). Nothing retries or resets
//! USB.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::path::PathBuf;

use crate::guided::{backup_dir, model, pad_in, Pad};
use crate::hw;
use crate::step_probe::{full_macro, put, PAGE_STEPS, STEPS, STEP_LEN};
use controller_core::devices::pro3::{DINPUT, XINPUT};
use controller_core::model::{MacroDefinition, MacroSlot, Mode, Slot};
use controller_core::transport::DeviceIo as _;

/// How many switches to `XInput` and back the probe watches.
const TRIPS: u32 = 2;
/// One trigger per macro slot. The check's profile leaves each of them as it is.
const TRIGGERS: [&str; 4] = ["left face", "d-pad up", "d-pad down", "d-pad left"];
/// The profile slots and macro slots the probe writes.
const SLOTS: [u8; 3] = [1, 2, 3];
const MACRO_SLOTS: [u8; 4] = [0, 1, 2, 3];
/// A macro page, and the chunk a page write is a whole number of.
const PAGE_LEN: usize = 4096;
const CHUNK_LEN: usize = 32;

/// Where the probe saves the banks and pages it changes.
fn saved_dir() -> PathBuf {
    backup_dir().join("ultimate-2-sweep")
}

fn bank_file(mode: Mode) -> PathBuf {
    saved_dir().join(format!("{mode}-bank.bin"))
}

fn page_file(mode: Mode, slot: u8, n: u8) -> PathBuf {
    saved_dir().join(format!("{mode}-slot{slot}-macro{}.bin", n + 1))
}

/// Every page as (profile slot, macro slot).
fn pages() -> impl Iterator<Item = (u8, u8)> {
    SLOTS.into_iter().flat_map(|s| MACRO_SLOTS.into_iter().map(move |n| (s, n)))
}

/// The page of macro slot `n` of `mode`'s `slot`, but for its last 6 bytes.
fn read_page(pad: &Pad, mode: Mode, slot: u8, n: u8) -> Vec<u8> {
    let (slot, n) = (Slot::new(slot).unwrap(), MacroSlot::new(n).unwrap());
    pad.dev.read_macro_stream(mode, slot, n, PAGE_STEPS).unwrap()
}

/// Writes `bytes` to `path`, refusing a file that exists.
fn save_file(path: &PathBuf, bytes: &[u8]) {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .unwrap_or_else(|e| panic!("{}: {e}; move the old sweep away", path.display()));
    file.write_all(bytes).and_then(|()| file.sync_all()).unwrap();
}

/// Saves `mode`'s bank and its 12 pages.
fn save(pad: &Pad, mode: Mode) {
    std::fs::create_dir_all(saved_dir()).unwrap();
    save_file(&bank_file(mode), &pad.bank(mode));
    for (slot, n) in pages() {
        save_file(&page_file(mode, slot, n), &read_page(pad, mode, slot, n));
    }
    println!("saved {mode}'s bank and pages to {}", saved_dir().display());
}

/// Writes `page`, as read, back to its macro slot: erased, then its bytes up to the
/// last one that is not `0xFF`, in whole chunks.
fn put_page(pad: &Pad, mode: Mode, slot: u8, n: u8, page: &[u8]) {
    let used = page.iter().rposition(|&b| b != 0xFF).map_or(0, |last| last + 1);
    let mut stream = page.to_vec();
    stream.resize(PAGE_LEN, 0xFF);
    stream.truncate(used.div_ceil(CHUNK_LEN).max(1) * CHUNK_LEN);
    let (slot, n) = (Slot::new(slot).unwrap(), MacroSlot::new(n).unwrap());
    let back = pad.dev.begin_write().unwrap();
    let sent = pad.dev.write_macro_stream(mode, slot, n, &stream);
    pad.dev.end_write(back).unwrap();
    sent.unwrap();
}

/// Writes `mode`'s saved pages and bank back, and checks them. The pad must be in `mode`.
fn restore_mode(pad: &Pad, mode: Mode) {
    for (slot, n) in pages() {
        let page = std::fs::read(page_file(mode, slot, n)).unwrap();
        put_page(pad, mode, slot, n, &page);
        assert_eq!(read_page(pad, mode, slot, n), page, "{mode} slot {slot} macro {}", n + 1);
    }
    let bank = std::fs::read(bank_file(mode)).unwrap();
    hw::write_bank(&pad.dev, mode, &bank);
    assert!(hw::same_bank(&pad.bank(mode), &bank), "{mode}: the bank differs from the saved one");
    println!("{mode}: every page and the bank are back as saved");
}

/// The steps of the probe macro in macro slot `n` of `mode`'s `slot`: `steps`, or the
/// description's lower limit there.
fn steps_of(mode: Mode, slot: u8, n: u8, steps: usize) -> usize {
    let max = model().description().unwrap().macro_steps_max(mode, slot, n);
    steps.min(usize::try_from(max).unwrap())
}

/// The four probe macros of `mode`'s `slot`, one per macro slot.
fn sweep_macros(mode: Mode, slot: u8, steps: usize) -> Vec<MacroDefinition> {
    MACRO_SLOTS
        .into_iter()
        .map(|n| MacroDefinition {
            name: format!("Sweep {}", n + 1),
            trigger_buttons: vec![TRIGGERS[usize::from(n)].into()],
            macro_slot: Some(n),
            ..full_macro(mode, steps_of(mode, slot, n, steps))
        })
        .collect()
}

/// Prints, per written mode and slot, whether each macro's steps read as written.
fn look(pad: &Pad, written: &[Mode], steps: usize, when: &str) {
    println!("{when}:");
    for &mode in written {
        for slot in SLOTS {
            let states: Vec<String> = MACRO_SLOTS
                .into_iter()
                .map(|n| {
                    let def = full_macro(mode, steps_of(mode, slot, n, steps));
                    let stream = model().encode_macro_steps(&def.steps, mode).unwrap();
                    let used = def.steps.len() * STEP_LEN;
                    let page = read_page(pad, mode, slot, n);
                    let state = if page[..used] == stream[..used] {
                        "ok"
                    } else if page[..used].iter().all(|&b| b == 0xFF) {
                        "ERASED"
                    } else {
                        "CHANGED"
                    };
                    format!("macro {} {state}", n + 1)
                })
                .collect();
            println!("  {mode} slot {slot}: {}", states.join(", "));
        }
    }
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; writes slots 1 to 3 of both modes, then restores"]
fn probe() {
    let steps = std::env::var("ULTIMATE_2_PROBE_STEPS").map_or(STEPS, |n| n.parse().unwrap());
    let trips = std::env::var("ULTIMATE_2_PROBE_TRIPS").map_or(TRIPS, |n| n.parse().unwrap());
    println!(
        "each macro has {steps} steps, DInput slot 3 macro 1 {}",
        steps_of(DINPUT, 3, 0, steps)
    );
    let mut written = Vec::new();
    let mut pad = None;
    for mode in [XINPUT, DINPUT] {
        drop(pad);
        let now = pad_in(mode);
        save(&now, mode);
        // `put` reads every macro back as written.
        for slot in SLOTS {
            put(&now, mode, Slot::new(slot).unwrap(), &sweep_macros(mode, slot, steps));
        }
        println!("wrote and read back {mode}'s 12 macros");
        written.push(mode);
        pad = Some(now);
    }
    for trip in 1..=trips {
        for mode in [XINPUT, DINPUT] {
            drop(pad);
            let now = pad_in(mode);
            look(&now, &written, steps, &format!("trip {trip}, {mode} start-up"));
            pad = Some(now);
        }
    }

    // The pad is in DInput now.
    let now = pad.unwrap();
    restore_mode(&now, DINPUT);
    drop(now);
    restore_mode(&pad_in(XINPUT), XINPUT);
    println!(
        "\nThe saved files stay in {}: move them away before another run.",
        saved_dir().display()
    );
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; writes the pages and bank of one mode"]
fn restore() {
    let name = std::env::var("ULTIMATE_2_SWEEP_MODE").expect("set ULTIMATE_2_SWEEP_MODE");
    let mode = [XINPUT, DINPUT].into_iter().find(|m| m.to_string() == name).unwrap();
    restore_mode(&pad_in(mode), mode);
}
