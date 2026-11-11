//! The GUI check of the Ultimate 2, run by a person at the desk, see
//! `docs/ultimate-2-gui-check.md`: `just ultimate-2-gui-check backup|check|cleared|restore`.
//! The person builds slots 2 and 3 of each mode in the window; `check` reads them back
//! and compares each with its file in `fixtures/ultimate-2/gui-check/`. Nothing here
//! writes to the pad but `restore`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::path::PathBuf;

use crate::fixture::{differences_from, load};
use crate::guided::{backup_dir, model, modes, pad_in, Pad};
use crate::hw;
use crate::{gui_check_file, GUI_CHECK};
use controller_core::model::{Mode, RawProfilePayload, Slot};
use controller_core::service::read::read_macros;
use controller_core::transport::DeviceIo as _;

/// The slot the person keeps: the check must leave it as the backup has it.
const KEPT: u8 = 1;

/// Where the backup of `mode`'s bank goes: the app's backup folder.
fn backup_file(mode: Mode) -> PathBuf {
    backup_dir().join(format!("ultimate-2-gui-check-{mode}.bin"))
}

/// The slots the check builds in `mode`.
fn slots(mode: Mode) -> Vec<u8> {
    GUI_CHECK.iter().filter(|(m, _)| *m == mode.to_string()).map(|&(_, s)| s).collect()
}

/// The backup of `mode`'s bank, as `backup` saved it.
fn backup(mode: Mode) -> Vec<u8> {
    let path = backup_file(mode);
    let saved = std::fs::read(&path).unwrap_or_else(|e| {
        panic!("{}: {e}; run `just ultimate-2-gui-check backup` first", path.display())
    });
    assert_eq!(saved.len(), model().blob_size(), "the {mode} backup is not one bank");
    saved
}

/// Every way slot 1 of `bank` differs from slot 1 of `saved`: its profile, and its
/// macros' names, triggers and lengths. The steps live outside the bank.
fn kept_changes(mode: Mode, bank: &[u8], saved: &[u8]) -> Vec<String> {
    let slot = Slot::new(KEPT).unwrap();
    let decode = |blob: &[u8]| {
        let raw = RawProfilePayload {
            payload: blob.to_vec(),
            source_slot: KEPT,
            source_profile_index: KEPT - 1,
            mode_hint: mode,
        };
        let active = model().slot_active(blob, slot).unwrap();
        let profile = active.then(|| model().map_profile(&raw).unwrap().canonical);
        (profile, model().decode_macro_metadata(blob, slot).unwrap())
    };
    let (now, then) = (decode(bank), decode(saved));
    let mut out = Vec::new();
    if now.0 != then.0 {
        out.push(format!("{mode} slot {KEPT}: the profile changed"));
    }
    if now.1 != then.1 {
        out.push(format!("{mode} slot {KEPT}: the macros changed"));
    }
    out
}

/// The pad in `mode`, and its bank.
fn bank_in(mode: Mode) -> (Pad, Vec<u8>) {
    let pad = pad_in(mode);
    let bank = pad.bank(mode);
    (pad, bank)
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-gui-check backup`"]
fn backup_phase() {
    for mode in modes() {
        let (_, bank) = bank_in(mode);
        // Restoring the bank undoes the check only if its slots had nothing to lose:
        // their macro steps live on flash pages the bank does not hold. An inactive slot
        // plays nothing, so the macros a deactivated slot left behind are not lost.
        for n in slots(mode) {
            let slot = Slot::new(n).unwrap();
            assert!(
                !model().slot_active(&bank, slot).unwrap(),
                "{mode} slot {n} holds a profile: move it, then back up again"
            );
            let left = model().decode_macro_metadata(&bank, slot).unwrap().len();
            if left > 0 {
                println!("{mode} slot {n} is empty; old macros left in it: {left}");
            }
        }
        let path = backup_file(mode);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // `create_new`: a second backup could hold the check's slots, not the pad's own.
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap_or_else(
                |e| panic!("{}: {e}; move the old backup away to back up again", path.display()),
            );
        file.write_all(&bank).and_then(|()| file.sync_all()).unwrap();
        println!("saved the {mode} bank to {}", path.display());
    }
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-gui-check check`"]
fn check_phase() {
    // Every slot is compared, so one run shows all the window got wrong.
    let mut failed = Vec::new();
    for mode in modes() {
        let saved = backup(mode);
        let (pad, bank) = bank_in(mode);
        let read = pad.dev.read_all_profiles().unwrap();
        for n in slots(mode) {
            let (want, want_macros) = load(&gui_check_file(&mode.to_string(), n));
            // The read lists an empty slot too, with an empty id.
            let Some(profile) = read
                .profiles
                .iter()
                .find(|p| p.mode == mode && p.source_slot == n && !p.id.is_empty())
            else {
                println!("{mode} slot {n}: FAILED, the slot is empty");
                failed.push(format!("{mode} slot {n}: empty"));
                continue;
            };
            let macros = read_macros(&pad.dev, mode, Slot::new(n).unwrap()).unwrap().macros;
            let found = differences_from(&want, &want_macros, &profile.canonical, &macros);
            if found.is_empty() {
                println!("{mode} slot {n}: ok, it matches its file");
            } else {
                println!("{mode} slot {n}: FAILED, {} differences", found.len());
                for line in &found {
                    println!("  {line}");
                }
                failed.extend(found.into_iter().map(|l| format!("{mode} slot {n}: {l}")));
            }
        }
        let kept = kept_changes(mode, &bank, &saved);
        if kept.is_empty() {
            println!("{mode} slot {KEPT}: ok, as the backup has it");
        }
        for line in kept {
            println!("{line}: FAILED");
            failed.push(line);
        }
    }
    assert!(failed.is_empty(), "{} differences:\n{}", failed.len(), failed.join("\n"));
    println!("\nEvery slot checked matches its file.");
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-gui-check cleared`"]
fn cleared_phase() {
    let mut failed = Vec::new();
    for mode in modes() {
        let saved = backup(mode);
        let (_, bank) = bank_in(mode);
        for n in slots(mode) {
            let slot = Slot::new(n).unwrap();
            if model().slot_active(&bank, slot).unwrap() {
                failed.push(format!("{mode} slot {n}: still holds a profile"));
            }
            // Clear slot keeps the macros stored, as its question says, until the next
            // profile written to the slot removes them.
            let left = model().decode_macro_metadata(&bank, slot).unwrap().len();
            if left > 0 {
                println!("{mode} slot {n}: empty, its {left} macros stay stored as Clear says");
            }
        }
        failed.extend(kept_changes(mode, &bank, &saved));
    }
    for line in &failed {
        println!("FAILED: {line}");
    }
    assert!(failed.is_empty(), "{} things are not as the check began", failed.len());
    println!("\nThe check's slots hold no profile, and slot {KEPT} is as the backup has it.");
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-gui-check restore`"]
fn restore_phase() {
    for mode in modes() {
        let saved = backup(mode);
        let pad = pad_in(mode);
        hw::write_bank(&pad.dev, mode, &saved);
        assert!(hw::same_bank(&pad.bank(mode), &saved), "{mode}: the pad differs from the backup");
        println!("{mode}: the pad holds the backup again");
    }
}
