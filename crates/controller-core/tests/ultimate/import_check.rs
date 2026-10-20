//! A guided check that a pad takes a profile file with macros and plays it. Three
//! phases, each run once by a person at the desk, see `docs/ultimate-import-check.md`:
//! `just ultimate-import-check backup|check|restore`. Each stops at its first failure,
//! never retries, and relocks the pad on every path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;
use std::path::PathBuf;

use crate::hw;
use crate::import_fixtures::{differences, fixture, prompts, read_slot, SLOT};
use crate::ultimate;
use controller_core::devices::locked_ports;
use controller_core::devices::ultimate::tables::BLOB_SIZE;
use controller_core::devices::ultimate::{MODES, SWITCH, XINPUT};
use controller_core::model::ProfileReadResult;
use controller_core::transport::lock::{relock, unlock, Unlocked};
use controller_core::transport::{DeviceIo, HidrawDevice};

/// Where the backup of the pad's banks goes: the app's backup folder.
fn backup_file() -> PathBuf {
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .expect("neither XDG_STATE_HOME nor HOME is set");
    state.join("8b/backups/ultimate-import-check.bin")
}

/// An unlocked pad, relocked when this drops, a failed phase included.
struct Session {
    port: String,
    dev: HidrawDevice,
}

impl Session {
    /// Unlocks the locked Ultimate on USB.
    fn new() -> Self {
        let locked = locked_ports().into_iter().next().expect("no locked Ultimate is attached");
        let port = locked.port_path.clone();
        match unlock(&locked) {
            Ok(Unlocked::Port(_)) => {}
            other => panic!("unlock: {other:?}"),
        }
        Self { port, dev: hw::pad(ultimate()) }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        match relock(ultimate(), &self.port) {
            Ok(()) => println!("pad relocked"),
            Err(e) => eprintln!("the pad could not be relocked: {e}; unplug it and plug it in"),
        }
    }
}

#[test]
#[ignore = "needs a locked Ultimate on USB; run by `just ultimate-import-check backup`"]
fn backup() {
    let path = backup_file();
    let banks = Session::new().dev.read_all_profiles().unwrap().raw_blobs;
    assert_eq!(banks.len(), MODES.len());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    // `create_new`: a second backup would hold the imported profile, not the pad's own.
    let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&path).unwrap();
    file.write_all(&banks.concat()).and_then(|()| file.sync_all()).unwrap();
    println!("saved {} banks to {}", banks.len(), path.display());
}

#[test]
#[ignore = "needs a locked Ultimate on USB; run by `just ultimate-import-check check`"]
fn check() {
    let banks = Session::new().dev.read_all_profiles().unwrap().raw_blobs;
    let modes = modes_to_press();
    let mut failed = false;
    for (mode, blob) in MODES.iter().zip(&banks).filter(|(m, _)| modes.contains(m)) {
        let want = fixture(*mode);
        let found = differences(&want, &read_slot(blob, SLOT));
        println!("{mode} slot {SLOT}: {} differences", found.len());
        for line in &found {
            println!("  {line}");
        }
        failed |= !found.is_empty();
    }
    assert!(!failed, "the pad does not hold the files: import them again and write them");
    // The pad sends input only while locked, and the guard in `Session` has relocked it.
    for mode in modes {
        press(mode);
    }
    println!("every press passed");
}

#[test]
#[ignore = "needs a locked Ultimate on USB; run by `just ultimate-import-check restore`"]
fn restore() {
    let saved = std::fs::read(backup_file()).unwrap();
    assert_eq!(saved.len(), BLOB_SIZE * MODES.len(), "the backup file is not two banks");
    let target = ProfileReadResult {
        raw_blobs: saved.chunks(BLOB_SIZE).map(<[u8]>::to_vec).collect(),
        ..ProfileReadResult::default()
    };
    let pad = Session::new();
    hw::put(&pad.dev, &MODES, &target);
    let now = pad.dev.read_all_profiles().unwrap().raw_blobs;
    for ((mode, held), want) in MODES.iter().zip(&now).zip(&target.raw_blobs) {
        assert!(hw::same_bank(held, want), "{mode}: the pad differs from the backup");
    }
    println!("the pad holds the backup again");
}

/// The modes to compare and press: the ones `IMPORT_CHECK_MODES` names, such as "switch", or both when it names none.
fn modes_to_press() -> Vec<controller_core::model::Mode> {
    let named = std::env::var("IMPORT_CHECK_MODES").unwrap_or_default();
    let picked: Vec<_> = named
        .split_whitespace()
        .map(|m| match m {
            "xinput" => XINPUT,
            "switch" => SWITCH,
            other => panic!("IMPORT_CHECK_MODES: no mode {other}; use xinput or switch"),
        })
        .collect();
    if picked.is_empty() {
        vec![XINPUT, SWITCH]
    } else {
        picked
    }
}

/// Where the presses of `mode` go. In Switch mode, this computer's Bluetooth link made the
/// desk pad drop the slot's profile after a moment; a phone kept it.
fn tester_host(mode: controller_core::model::Mode) -> &'static str {
    if mode == SWITCH {
        "Pair the pad by Bluetooth to an Android phone or a Switch, not this computer: on this\ncomputer's link the pad drops the profile after a moment. Open a gamepad tester there."
    } else {
        "Keep the pad on its cable to this computer. Open a gamepad tester here."
    }
}

/// Reads one line, without its newline.
fn line() -> String {
    let mut text = String::new();
    let n = std::io::stdin().read_line(&mut text).unwrap();
    assert!(n > 0, "no input: run this in a terminal");
    text.trim().to_owned()
}

/// Asks the person to select `mode` and the slot, then every press of its file.
fn press(mode: controller_core::model::Mode) {
    let label = crate::d().mode_label(mode);
    println!("\n== {label} ==");
    println!("Put the pad in {label} mode. Select slot {SLOT}. Use the pad's own way for both:");
    println!("this repo does not say how the pad picks its mode or slot.");
    println!("{}", tester_host(mode));
    println!("Press Enter when ready.");
    line();
    let steps = prompts(&fixture(mode));
    for (i, step) in steps.iter().enumerate() {
        println!("\n[{}/{}] {}\n  Expect: {}", i + 1, steps.len(), step.action, step.expect);
        loop {
            println!("  Enter = it passed, n = it failed");
            match line().as_str() {
                "" => break,
                "n" => panic!("failed: {} / {}", step.action, step.expect),
                _ => {}
            }
        }
    }
}
