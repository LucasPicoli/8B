//! A guided check of the Ultimate 2 in both its modes, run by a person at the desk,
//! see `docs/ultimate-2-check.md`: `just ultimate-2-check backup|check|restore`. Each
//! phase runs once and stops at its first failure. Nothing retries or resets USB.
//!
//! The pad keeps one USB id in both modes and serves only the bank of the mode it is
//! in, so each phase asks the person to move the pad's mode switch between modes.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::fixture::{differences, fixture, profile_json, Expect, PRESSES, SLOT};
use crate::hw;
use crate::input::Input;
use controller_core::detect::scan_sysfs_all;
use controller_core::device::Model;
use controller_core::devices::{model_named, SYSFS_USB};
use controller_core::model::{MacroSlot, Mode, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::read::read_macros;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo, HidrawDevice};

/// The header byte where the pad records the profile it plays: 0 for slot 1, 3 for
/// its default map.
const CUR_SLOT: usize = 0x12;

/// The Ultimate 2.
fn model() -> &'static dyn Model {
    model_named("Ultimate 2").expect("the Ultimate 2 is registered")
}

/// The modes `ULTIMATE_2_CHECK_MODES` names, such as "dinput", or both when it names
/// none, in the description's order.
fn modes() -> Vec<Mode> {
    let listed: Vec<Mode> = model().description().unwrap().modes.iter().map(|m| m.id).collect();
    let named = std::env::var("ULTIMATE_2_CHECK_MODES").unwrap_or_default();
    let picked: Vec<Mode> = named
        .split_whitespace()
        .map(|n| {
            *listed.iter().find(|m| m.to_string() == n).unwrap_or_else(|| {
                panic!("ULTIMATE_2_CHECK_MODES: no mode {n}; use xinput or dinput")
            })
        })
        .collect();
    if picked.is_empty() {
        listed
    } else {
        picked
    }
}

/// Where the backup of `mode`'s bank goes: the app's backup folder.
fn backup_file(mode: Mode) -> PathBuf {
    backup_dir().join(format!("ultimate-2-check-{mode}.bin"))
}

/// The app's backup folder.
fn backup_dir() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .expect("neither XDG_STATE_HOME nor HOME is set");
    state.join("8b/backups")
}

/// Reads one line, without its newline.
fn line() -> String {
    let mut text = String::new();
    let n = std::io::stdin().read_line(&mut text).unwrap();
    assert!(n > 0, "no input: run this in a terminal");
    text.trim().to_owned()
}

/// An attached Ultimate 2 and its USB port path.
struct Pad {
    port: String,
    dev: HidrawDevice,
}

impl Pad {
    /// The first attached pad that answers as an Ultimate 2.
    fn find() -> Option<Self> {
        let ports = &model().description().unwrap().config_ports;
        scan_sysfs_all(Path::new(SYSFS_USB), ports).into_iter().find_map(|usb| {
            let dev = HidrawDevice::at(usb.port_path());
            let ours = dev.model().is_ok_and(|m| {
                std::ptr::eq(m.description().unwrap(), model().description().unwrap())
            });
            ours.then(|| Self { port: usb.port_path().to_owned(), dev })
        })
    }

    /// The bank of `mode`, which must be the mode the pad is in.
    fn bank(&self, mode: Mode) -> Vec<u8> {
        let read = self.dev.read_all_profiles().unwrap();
        let index = model().description().unwrap().modes.iter().position(|m| m.id == mode).unwrap();
        let bank = read.raw_blobs[index].clone();
        assert_eq!(bank.len(), model().blob_size(), "the pad served no {mode} bank");
        bank
    }
}

/// Asks the person to put the pad in `mode`, and waits until the pad says it is.
fn pad_in(mode: Mode) -> Pad {
    let label = model().description().unwrap().mode_label(mode);
    println!("\n== {label} ==");
    println!("Put the pad in {label} mode with its own switch, on its USB cable. Press Enter.");
    loop {
        line();
        match Pad::find() {
            Some(pad) => {
                let firmware = pad.dev.read_all_profiles().unwrap().firmware_version;
                match pad.dev.current_mode() {
                    Some(now) if now == mode => {
                        println!("found the pad on {}, firmware {firmware}", pad.port);
                        return pad;
                    }
                    now => println!(
                        "The pad says it is in {now:?}, not {label}. Switch it, then press Enter."
                    ),
                }
            }
            None => println!("No Ultimate 2 found. Plug it in, then press Enter."),
        }
    }
}

/// The slot the check writes.
fn slot() -> Slot {
    Slot::new(SLOT).unwrap()
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-check backup`"]
fn backup() {
    for mode in modes() {
        let path = backup_file(mode);
        let pad = pad_in(mode);
        let bank = pad.bank(mode);
        // Restoring the bank undoes the check only if slot 3 had nothing to lose:
        // its macro steps live on flash pages the bank does not hold.
        assert!(
            !model().slot_active(&bank, slot()).unwrap(),
            "{mode} slot {SLOT} holds a profile: move it, then back up again"
        );
        assert!(
            model().decode_macro_metadata(&bank, slot()).unwrap().is_empty(),
            "{mode} slot {SLOT} holds macros"
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // `create_new`: a second backup could hold the check's profile, not the pad's own.
        let mut file =
            std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap_or_else(
                |e| panic!("{}: {e}; move the old backup away to back up again", path.display()),
            );
        file.write_all(&bank).and_then(|()| file.sync_all()).unwrap();
        println!("saved the {mode} bank to {}", path.display());
    }
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-check check`"]
fn check() {
    for mode in modes() {
        assert!(
            backup_file(mode).exists(),
            "no backup of {mode}: run `just ultimate-2-check backup` first"
        );
        let pad = pad_in(mode);
        write(&pad, mode);
        compare(&pad, mode);
        refuse_other_mode(&pad, mode);
        press(&pad, mode);
        println!("{mode}: every check passed");
    }
    println!("\nRun `just ultimate-2-check restore` to put the backups back.");
}

#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; run by `just ultimate-2-check restore`"]
fn restore() {
    for mode in modes() {
        let saved = std::fs::read(backup_file(mode)).unwrap();
        assert_eq!(saved.len(), model().blob_size(), "the {mode} backup is not one bank");
        let pad = pad_in(mode);
        hw::write_bank(&pad.dev, mode, &saved);
        assert!(hw::same_bank(&pad.bank(mode), &saved), "{mode}: the pad differs from the backup");
        println!("{mode}: the pad holds the backup again");
    }
}

/// Writes the file of `mode` into slot 3 with its macros, in one job.
fn write(pad: &Pad, mode: Mode) {
    let (_, macros) = fixture(mode);
    let macro_edits = macros
        .into_iter()
        .map(|definition| MacroEdit::Put {
            macro_slot: MacroSlot::new(definition.macro_slot.unwrap()).unwrap(),
            definition,
        })
        .collect();
    let op =
        WriteOp::Upload { profile: Some(profile_json(mode)), drop_macros: Vec::new(), macro_edits };
    let dir = backup_dir();
    let writer = ProfileWriteOrchestrator::new(&pad.dev, model(), &dir);
    for result in writer.write_slots(&[WriteJob { mode, slot: slot(), op }], &ConfirmPolicy::Force)
    {
        assert!(result.success, "{mode} write: {}", result.message);
    }
    println!("wrote the {mode} file into slot {SLOT}");
}

/// Reads slot 3 back and compares it with the file: mappings, settings, macros.
fn compare(pad: &Pad, mode: Mode) {
    let read = pad.dev.read_all_profiles().unwrap();
    let profile = read.profiles.iter().find(|p| p.mode == mode && p.source_slot == SLOT).unwrap();
    let macros = read_macros(&pad.dev, mode, slot()).unwrap().macros;
    let found = differences(&profile.canonical, &macros);
    for line in &found {
        println!("  {line}");
    }
    assert!(found.is_empty(), "{mode} slot {SLOT} differs from the file in {} ways", found.len());
    println!("read slot {SLOT} back: it matches the file");
}

/// Tries to write the other mode's file while the pad is in `mode`: it must be refused
/// before anything is sent, and the bank must not change.
fn refuse_other_mode(pad: &Pad, mode: Mode) {
    let other =
        model().description().unwrap().modes.iter().map(|m| m.id).find(|m| *m != mode).unwrap();
    let before = pad.bank(mode);
    let op = WriteOp::Upload {
        profile: Some(profile_json(other)),
        drop_macros: Vec::new(),
        macro_edits: Vec::new(),
    };
    let dir = backup_dir();
    let writer = ProfileWriteOrchestrator::new(&pad.dev, model(), &dir);
    let results =
        writer.write_slots(&[WriteJob { mode: other, slot: slot(), op }], &ConfirmPolicy::Force);
    assert!(
        results.iter().all(|r| !r.success),
        "a {other} write went through while the pad is in {mode}"
    );
    assert!(
        hw::same_bank(&pad.bank(mode), &before),
        "the refused {other} write changed the {mode} bank"
    );
    println!("a {other} write was refused: {}", results[0].message);
}

/// Asks the person to select slot 3, checks the pad plays it, then runs every press.
fn press(pad: &Pad, mode: Mode) {
    println!("\nSelect profile {SLOT} on the pad with its profile button, then press Enter.");
    loop {
        line();
        let playing = pad.bank(mode)[CUR_SLOT];
        if usize::from(playing) == usize::from(SLOT) - 1 {
            break;
        }
        println!(
            "The pad plays profile {} (3 is its default map). Select {SLOT}, then press Enter.",
            playing + 1
        );
    }
    let mut input = Input::open(&pad.port);
    // Every press runs, so one run shows all the pad does; the mode fails at the end.
    let mut failed = Vec::new();
    for (i, step) in PRESSES.iter().enumerate() {
        println!("\n[{}/{}] {}", i + 1, PRESSES.len(), step.action);
        input.forget();
        match step.expect {
            Expect::Sends(want) => {
                let got = input.gesture();
                let (mut a, mut b) =
                    (want.to_vec(), got.iter().map(String::as_str).collect::<Vec<_>>());
                a.sort_unstable();
                b.sort_unstable();
                if b == a {
                    println!("  ok: {got:?}");
                } else {
                    println!("  FAILED: sent {got:?}, expected {want:?}");
                    failed.push(format!("{}: sent {got:?}, expected {want:?}", step.action));
                }
            }
            Expect::Nothing => {
                println!("  Then press Enter here.");
                line();
                let got = input.so_far();
                if got.is_empty() {
                    println!("  ok: nothing");
                } else {
                    println!("  FAILED: sent {got:?}, expected nothing");
                    failed.push(format!("{}: sent {got:?}, expected nothing", step.action));
                }
            }
        }
    }
    assert!(failed.is_empty(), "{mode}: {} presses failed:\n{}", failed.len(), failed.join("\n"));
}
