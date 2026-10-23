//! Probe: does writing one `DInput` slot of the Ultimate 2 erase a macro of another?
//! In the GUI check, a write of `DInput` slot 2 alone, the bank and an apply with no
//! macro command, left slot 3's second macro reading as erased flash. Run once, at the
//! desk, in `DInput`:
//!   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
//!    --nocapture --exact erase_probe::probe`
//!
//! It writes slots 2 and 3 with the GUI check's files, the state the check expects, so
//! it needs the GUI check's backup first. Slot 3's macros are read four times: after
//! slot 3's write, after an apply alone, after a switch to `XInput` and back, and after
//! slot 2's write. Nothing retries or resets USB.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::fixture::{differences_from, load};
use crate::gui_check_file;
use crate::guided::{backup_dir, line, model, pad_in, Pad};
use controller_core::model::{MacroDefinition, MacroSlot, Mode, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::read::read_macros;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo as _;
use serde_json::Value;

/// The length an erased step reads as: every byte `0xFF`.
const ERASED_MS: u16 = u16::MAX;

/// The GUI check's file of `DInput` slot `n` as a write job takes it: no macros.
fn profile_json(n: u8) -> Value {
    let path = gui_check_file("dinput", n);
    let mut json: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("macros");
    object.remove("macro_refs");
    json
}

/// Writes `DInput` slot `n` from its file, with `macros` put in their macro slots.
fn write(pad: &Pad, mode: Mode, n: u8, macros: &[MacroDefinition]) {
    let macro_edits = macros
        .iter()
        .map(|definition| MacroEdit::Put {
            macro_slot: MacroSlot::new(definition.macro_slot.unwrap()).unwrap(),
            definition: definition.clone(),
        })
        .collect();
    let op =
        WriteOp::Upload { profile: Some(profile_json(n)), drop_macros: Vec::new(), macro_edits };
    let slot = Slot::new(n).unwrap();
    let dir = backup_dir();
    let writer = ProfileWriteOrchestrator::new(&pad.dev, model(), &dir);
    for result in writer.write_slots(&[WriteJob { mode, slot, op }], &ConfirmPolicy::Force) {
        assert!(result.success, "write of slot {n}: {}", result.message);
    }
}

/// Reads slot 3's macros and prints, per macro, whether it matches the file. Returns
/// whether every macro matched.
fn report(pad: &Pad, mode: Mode, when: &str) -> bool {
    let (want, want_macros) = load(&gui_check_file("dinput", 3));
    let read = read_macros(&pad.dev, mode, Slot::new(3).unwrap()).unwrap().macros;
    println!("\n{when}:");
    let mut all_ok = true;
    for w in &want_macros {
        let n = w.macro_slot.unwrap();
        let Some(g) = read.iter().find(|g| g.macro_slot == w.macro_slot) else {
            println!("  macro {}: missing", n + 1);
            all_ok = false;
            continue;
        };
        let erased = g.steps.iter().filter(|s| s.duration_ms == ERASED_MS).count();
        let found =
            differences_from(&want, std::slice::from_ref(w), &want, std::slice::from_ref(g));
        match (erased, found.is_empty()) {
            (0, true) => println!("  macro {} {:?}: ok", n + 1, w.name),
            (e, _) if e == g.steps.len() => {
                println!("  macro {} {:?}: ERASED, all {e} steps read as 0xFF", n + 1, w.name);
                all_ok = false;
            }
            _ => {
                println!("  macro {} {:?}: DIFFERS", n + 1, w.name);
                for line in found {
                    println!("    {line}");
                }
                all_ok = false;
            }
        }
    }
    all_ok
}

#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slots 2 and 3"]
fn probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let pad = pad_in(mode);

    let (_, macros) = load(&gui_check_file("dinput", 3));
    write(&pad, mode, 3, &macros);
    println!("wrote slot 3 with its {} macros", macros.len());
    report(&pad, mode, "1. after slot 3's write");

    // An apply alone: if the pad plays back a page it never kept, this shows it.
    let back = pad.dev.begin_write().unwrap();
    let sent = pad.dev.send_apply(mode);
    pad.dev.end_write(back).unwrap();
    sent.unwrap();
    report(&pad, mode, "2. after an apply with no write");

    // The GUI session switched modes between slot 3's write and slot 2's.
    drop(pad);
    drop(pad_in(xinput));
    let pad = pad_in(mode);
    report(&pad, mode, "3. after a switch to XInput and back");

    write(&pad, mode, 2, &[]);
    println!("\nwrote slot 2 alone");
    report(&pad, mode, "4. after slot 2's write, the bank and an apply");
}

/// The window's run that lost the macro rewrote macro 2's page alone before slot 2's
/// write. This puts one page at a time, then writes slot 2, twice: with macro 2's page,
/// then with macro 3's. Run once, in `DInput`:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::one_page_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slots 2 and 3"]
fn one_page_probe() {
    let mode = model().description().unwrap().modes.iter().find(|m| m.id.to_string() == "dinput");
    let mode = mode.unwrap().id;
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let pad = pad_in(mode);
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let only = |slot: u8| -> Vec<MacroDefinition> {
        macros.iter().filter(|m| m.macro_slot == Some(slot)).cloned().collect()
    };

    write(&pad, mode, 3, &macros);
    report(&pad, mode, "1. after slot 3's write with every page");
    write(&pad, mode, 2, &[]);
    report(&pad, mode, "2. after slot 2's write");

    write(&pad, mode, 3, &only(1));
    report(&pad, mode, "3. after slot 3's write with macro 2's page alone");
    write(&pad, mode, 2, &[]);
    report(&pad, mode, "4. after slot 2's write");

    write(&pad, mode, 3, &only(2));
    report(&pad, mode, "5. after slot 3's write with macro 3's page alone");
    write(&pad, mode, 2, &[]);
    report(&pad, mode, "6. after slot 2's write");
}

/// Between its page write and slot 2's write, the window's run switched to `XInput`
/// and read the steps of `XInput` slot 3's macros. This puts macro 2's page, switches
/// to `XInput` and back, and writes slot 2: first with no read in `XInput`, then with
/// one. It needs `XInput` slot 3 to hold the GUI check's macros. Run once, in `DInput`:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::mode_trip_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slots 2 and 3"]
fn mode_trip_probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let macro_2: Vec<MacroDefinition> =
        macros.iter().filter(|m| m.macro_slot == Some(1)).cloned().collect();

    for read_in_xinput in [false, true] {
        let pad = pad_in(mode);
        write(&pad, mode, 3, &macro_2);
        report(&pad, mode, &format!("after macro 2's page (XInput read next: {read_in_xinput})"));
        drop(pad);
        let pad = pad_in(xinput);
        if read_in_xinput {
            let read = read_macros(&pad.dev, xinput, Slot::new(3).unwrap()).unwrap().macros;
            println!("read the steps of {} XInput slot 3 macros", read.len());
        }
        drop(pad);
        let pad = pad_in(mode);
        report(&pad, mode, "after the switch to XInput and back");
        write(&pad, mode, 2, &[]);
        report(&pad, mode, "after slot 2's write");
    }
}

/// `mode_trip_probe` lost macro 2's page, the last page written, to a read of every
/// `XInput` slot 3 macro. This puts macro 3's page and reads all of them, then puts
/// macro 2's page and reads macro 1's page alone. Run once, in `DInput`:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::which_page_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3"]
fn which_page_probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let only = |slot: u8| -> Vec<MacroDefinition> {
        macros.iter().filter(|m| m.macro_slot == Some(slot)).cloned().collect()
    };
    let slot_3 = Slot::new(3).unwrap();

    for (put, read_all) in [(2, true), (1, false)] {
        let pad = pad_in(mode);
        write(&pad, mode, 3, &macros);
        write(&pad, mode, 3, &only(put));
        report(&pad, mode, &format!("after every page, then macro {}'s page", put + 1));
        drop(pad);
        let pad = pad_in(xinput);
        if read_all {
            let read = read_macros(&pad.dev, xinput, slot_3).unwrap().macros;
            println!("read the steps of {} XInput slot 3 macros", read.len());
        } else {
            let first = MacroSlot::new(0).unwrap();
            let stream = pad.dev.read_macro_stream(xinput, slot_3, first, 1).unwrap();
            println!("read the first step of XInput slot 3 macro 1: {stream:02x?}");
        }
        drop(pad);
        let pad = pad_in(mode);
        report(&pad, mode, "after the XInput read and back");
    }
}

/// `which_page_probe` kept every page. This puts macro 2's page last again, then reads
/// `XInput` slot 3's macro 2 page alone, then every macro, as `mode_trip_probe` did.
/// Run once, in `DInput`:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::page_one_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3"]
fn page_one_probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let macro_2: Vec<MacroDefinition> =
        macros.iter().filter(|m| m.macro_slot == Some(1)).cloned().collect();
    let slot_3 = Slot::new(3).unwrap();
    // The page of macro 2 holds 12 steps once its glide is expanded.
    let page_steps = 12;

    for read_all in [false, true] {
        let pad = pad_in(mode);
        write(&pad, mode, 3, &macros);
        write(&pad, mode, 3, &macro_2);
        report(&pad, mode, "after every page, then macro 2's page");
        drop(pad);
        let pad = pad_in(xinput);
        if read_all {
            let read = read_macros(&pad.dev, xinput, slot_3).unwrap().macros;
            println!("read the steps of {} XInput slot 3 macros", read.len());
        } else {
            let second = MacroSlot::new(1).unwrap();
            let stream = pad.dev.read_macro_stream(xinput, slot_3, second, page_steps).unwrap();
            println!("read XInput slot 3 macro 2's page: {stream:02x?}");
        }
        drop(pad);
        let pad = pad_in(mode);
        report(&pad, mode, "after the XInput read and back");
    }
}

/// How many trials `quiet_trip_probe` runs when no page is lost.
const QUIET_TRIALS: u32 = 10;

/// Does a mode switch alone lose a page? Each trial puts macro 2's page, closes the pad
/// and waits while the person switches to `XInput` and back: 8B sends nothing in
/// between. Then it reads slot 3. It stops at the first lost page. Run once, in
/// `DInput`, with the 8B window closed:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::quiet_trip_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3"]
fn quiet_trip_probe() {
    let mode = model().description().unwrap().modes.iter().find(|m| m.id.to_string() == "dinput");
    let mode = mode.unwrap().id;
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let macro_2: Vec<MacroDefinition> =
        macros.iter().filter(|m| m.macro_slot == Some(1)).cloned().collect();

    let mut pad = pad_in(mode);
    write(&pad, mode, 3, &macros);
    for trial in 1..=QUIET_TRIALS {
        write(&pad, mode, 3, &macro_2);
        assert!(report(&pad, mode, &format!("trial {trial}: after macro 2's page")));
        drop(pad);
        println!("\nSwitch the pad to XInput, then back to DInput. Press Enter.");
        line();
        pad = pad_in(mode);
        if !report(&pad, mode, &format!("trial {trial}: after the switch, no 8B traffic")) {
            println!("\nLost a page in trial {trial} of {QUIET_TRIALS}, with no 8B traffic.");
            return;
        }
    }
    println!("\nNo page lost in {QUIET_TRIALS} trials.");
}

/// Steps that fill a 4096-byte macro page but for its last 6 bytes, at 10 bytes a step.
const PAGE_STEPS: usize = 409;

/// Does the pad keep its own data in `DInput` slot 3's page `0x1000`, past a macro's
/// steps? Reads the four pages of slot 3 whole, in the mode the pad is in, and saves
/// each to the backup directory. Reads only. Run once:
///   `ULTIMATE_2_DUMP_MODE=dinput cargo test -p controller-core --features hardware
///    --test ultimate2 -- --ignored --nocapture --exact erase_probe::page_dump_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB and a person; reads only"]
fn page_dump_probe() {
    let name = std::env::var("ULTIMATE_2_DUMP_MODE").unwrap_or_else(|_| "dinput".to_owned());
    let mode = model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name);
    let mode = mode.unwrap().id;
    let pad = pad_in(mode);
    for n in 0..4 {
        let macro_slot = MacroSlot::new(n).unwrap();
        let page = pad.dev.read_macro_stream(mode, Slot::new(3).unwrap(), macro_slot, PAGE_STEPS);
        let page = page.unwrap();
        let path = backup_dir().join(format!("ultimate-2-{name}-slot3-page{n}.bin"));
        std::fs::write(&path, &page).unwrap();
        let used = page.iter().rposition(|&b| b != 0xFF).map_or(0, |last| last + 1);
        println!("page {n}: {used} of {} bytes up to the last non-0xFF one", page.len());
    }
}

/// Where the pad keeps a byte of its own in `DInput` slot 3's page 0: 16 bytes before
/// page 1, which it erases.
const RECORD_OFFSET: usize = 0x0FF0;

/// How many switches to `XInput` and back `record_watch_probe` watches.
const WATCHED_TRIPS: u32 = 6;

/// How the pad's own byte at [`RECORD_OFFSET`] and the page after it change across
/// mode switches. Puts macro 2's page once, then reads only: after each switch to
/// `XInput` and back, prints page 0's last 10 bytes and the used length of page 1. Run
/// once, in `DInput`, with the 8B window closed:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::record_watch_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3 once"]
fn record_watch_probe() {
    let mode = model().description().unwrap().modes.iter().find(|m| m.id.to_string() == "dinput");
    let mode = mode.unwrap().id;
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let macro_2: Vec<MacroDefinition> =
        macros.iter().filter(|m| m.macro_slot == Some(1)).cloned().collect();
    let slot_3 = Slot::new(3).unwrap();
    let watch = |pad: &Pad, when: &str| {
        let page =
            |n| pad.dev.read_macro_stream(mode, slot_3, MacroSlot::new(n).unwrap(), PAGE_STEPS);
        let (first, second) = (page(0).unwrap(), page(1).unwrap());
        let used = second.iter().rposition(|&b| b != 0xFF).map_or(0, |last| last + 1);
        println!("{when}: page 0 tail {:02x?}, page 1 used {used}", &first[RECORD_OFFSET..]);
    };

    let mut pad = pad_in(mode);
    write(&pad, mode, 3, &macro_2);
    watch(&pad, "after macro 2's page");
    for trip in 1..=WATCHED_TRIPS {
        drop(pad);
        println!("\nSwitch the pad to XInput, then back to DInput. Press Enter.");
        line();
        pad = pad_in(mode);
        watch(&pad, &format!("trip {trip}"));
    }
}

/// How many single switches `boot_side_probe` watches.
const SINGLE_SWITCHES: u32 = 12;

/// Which start-up erases `DInput` slot 3's page 1: into `XInput` or into `DInput`?
/// Puts macro 2's page, then after each single switch reads the `DInput` pages from
/// whichever mode the pad is in, as each read packet names its mode. Page 0's own byte
/// at [`RECORD_OFFSET`] shows the read reached the `DInput` bank. A lost page is put
/// again at the next `DInput` start-up. Run once, in `DInput`, with the 8B window closed:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::boot_side_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3"]
fn boot_side_probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let macro_2: Vec<MacroDefinition> =
        macros.iter().filter(|m| m.macro_slot == Some(1)).cloned().collect();
    let slot_3 = Slot::new(3).unwrap();
    // Whether the `DInput` page 1 holds data, and page 0's last 10 bytes.
    let look = |pad: &Pad| {
        let page =
            |n| pad.dev.read_macro_stream(mode, slot_3, MacroSlot::new(n).unwrap(), PAGE_STEPS);
        let (first, second) = (page(0).unwrap(), page(1).unwrap());
        (second.iter().any(|&b| b != 0xFF), first[RECORD_OFFSET..].to_vec())
    };

    let mut pad = pad_in(mode);
    write(&pad, mode, 3, &macro_2);
    let mut lost = false;
    for switch in 1..=SINGLE_SWITCHES {
        let into = if switch % 2 == 1 { xinput } else { mode };
        drop(pad);
        pad = pad_in(into);
        let (held, tail) = look(&pad);
        println!(
            "switch {switch}, into {into}: page 1 {}, page 0 tail {tail:02x?}",
            if held { "holds data" } else { "ERASED" }
        );
        lost |= !held;
        if lost && into == mode {
            write(&pad, mode, 3, &macro_2);
            lost = false;
            println!("  put macro 2's page again");
        }
    }
}

/// Tests a model of the erase. On a `DInput` start-up the pad sets its byte at
/// [`RECORD_OFFSET`] of page 0. Set once already, the byte makes the pad erase page 1.
/// A write of every page erases page 0 and clears the byte. So after such a write the
/// model predicts, for two trips to `XInput` and back:
///   trip 1: `XInput` page 1 holds, tail `ff`; `DInput` page 1 holds, tail `3c`
///   trip 2: `XInput` page 1 holds, tail `3c`; `DInput` page 1 erased, tail `3c`
/// Runs that twice. Run once, in `DInput`, with the 8B window closed:
///   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
///    --nocapture --exact erase_probe::record_cycle_probe`
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3"]
fn record_cycle_probe() {
    let named = |name: &str| {
        model().description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
    };
    let (mode, xinput) = (named("dinput"), named("xinput"));
    let backup = backup_dir().join("ultimate-2-gui-check-dinput.bin");
    assert!(backup.exists(), "no GUI check backup: run `just ultimate-2-gui-check backup` first");
    let (_, macros) = load(&gui_check_file("dinput", 3));
    let slot_3 = Slot::new(3).unwrap();
    let look = |pad: &Pad, when: &str| {
        let page =
            |n| pad.dev.read_macro_stream(mode, slot_3, MacroSlot::new(n).unwrap(), PAGE_STEPS);
        let (first, second) = (page(0).unwrap(), page(1).unwrap());
        let held = if second.iter().any(|&b| b != 0xFF) { "holds" } else { "ERASED" };
        println!("{when}: page 1 {held}, tail {:02x}", first[RECORD_OFFSET]);
    };

    let mut pad = pad_in(mode);
    for cycle in 1..=2 {
        write(&pad, mode, 3, &macros);
        look(&pad, &format!("\ncycle {cycle}, after every page"));
        for trip in 1..=2 {
            drop(pad);
            pad = pad_in(xinput);
            look(&pad, &format!("cycle {cycle}, trip {trip}, XInput"));
            drop(pad);
            pad = pad_in(mode);
            look(&pad, &format!("cycle {cycle}, trip {trip}, DInput"));
        }
    }
}
