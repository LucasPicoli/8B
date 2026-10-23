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
use crate::guided::{backup_dir, model, pad_in, Pad};
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

/// Reads slot 3's macros and prints, per macro, whether it matches the file.
fn report(pad: &Pad, mode: Mode, when: &str) {
    let (want, want_macros) = load(&gui_check_file("dinput", 3));
    let read = read_macros(&pad.dev, mode, Slot::new(3).unwrap()).unwrap().macros;
    println!("\n{when}:");
    for w in &want_macros {
        let n = w.macro_slot.unwrap();
        let Some(g) = read.iter().find(|g| g.macro_slot == w.macro_slot) else {
            println!("  macro {}: missing", n + 1);
            continue;
        };
        let erased = g.steps.iter().filter(|s| s.duration_ms == ERASED_MS).count();
        let found =
            differences_from(&want, std::slice::from_ref(w), &want, std::slice::from_ref(g));
        match (erased, found.is_empty()) {
            (0, true) => println!("  macro {} {:?}: ok", n + 1, w.name),
            (e, _) if e == g.steps.len() => {
                println!("  macro {} {:?}: ERASED, all {e} steps read as 0xFF", n + 1, w.name);
            }
            _ => {
                println!("  macro {} {:?}: DIFFERS", n + 1, w.name);
                for line in found {
                    println!("    {line}");
                }
            }
        }
    }
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
