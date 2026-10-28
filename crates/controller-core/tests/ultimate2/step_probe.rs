//! Probes of the Ultimate 2's macro step limit. The vendor app stops at 200; 8B allows
//! 406, and 307 in `DInput` slot 3's macro 1. Run each once, at the desk, with the 8B window closed:
//!   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
//!    --nocapture --exact step_probe::<name>`
//!
//! - `probe`, in `XInput`: does the pad play every step? It played all 408 on
//!   firmware 1.10. It writes one macro of
//!   [`STEPS`] steps that press A before step [`VENDOR_STEPS`] and B from it, with a
//!   rest between presses, and counts the presses on evdev while a person plays it.
//! - `record_probe`, in `DInput`: does a long macro 1 of slot 3 survive `DInput`
//!   start-ups? Up to 307 steps it does; from 308 the pad erases it.
//!
//! Each saves the bank first, refuses an occupied slot 3, and writes the bank back at
//! the end. Nothing retries or resets USB.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;

use crate::fixture::{profile_json, SLOT};
use crate::guided::{backup_dir, line, model, pad_in, Pad};
use crate::hw;
use crate::input::Input;
use controller_core::devices::pro3::{DINPUT, XINPUT};
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Mode, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo as _;

/// The steps of the probe macro: the `macro_steps` limit of the Ultimate 2's description.
pub const STEPS: usize = 406;
/// The vendor app's step limit. Steps before it press A, the rest press B.
const VENDOR_STEPS: usize = 200;
/// Each step's length. 406 of them run about 20 s.
const STEP_MS: u16 = 50;
/// Bytes per step record.
pub const STEP_LEN: usize = 10;
/// `EV_KEY` and the codes of A and B.
const EV_KEY: u16 = 1;
const BTN_A: u16 = 0x130;
const BTN_B: u16 = 0x131;
/// Where the pad keeps a byte of its own in `DInput` slot 3's page 0. A macro of 407 or
/// 408 steps would pad its write with zeros over it.
const RECORD_OFFSET: usize = 0x0FF0;
/// Steps that read a 4096-byte page but for its last 6 bytes.
pub const PAGE_STEPS: usize = 409;
/// How many switches to `XInput` and back `record_probe` watches, unless
/// `ULTIMATE_2_PROBE_TRIPS` sets another number.
const TRIPS: u32 = 1;

/// The full macro on X, in macro slot 1: even steps press A, or B from
/// [`VENDOR_STEPS`]; odd steps rest.
pub fn full_macro(mode: Mode, steps: usize) -> MacroDefinition {
    let steps = (0..steps)
        .map(|k| {
            let face = if k < VENDOR_STEPS { "bottom face" } else { "right face" };
            let pressed_buttons = if k % 2 == 0 { vec![face.into()] } else { Vec::new() };
            MacroStep { duration_ms: STEP_MS, pressed_buttons, ..MacroStep::default() }
        })
        .collect();
    MacroDefinition {
        name: "Step probe".into(),
        mode,
        trigger_buttons: vec!["left face".into()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(0),
        steps,
    }
}

/// Saves `mode`'s bank to the backup directory as `name`, refuses an occupied slot 3,
/// and returns the bank.
fn save_bank(pad: &Pad, mode: Mode, name: &str) -> Vec<u8> {
    let dir = backup_dir();
    let before = pad.bank(mode);
    let slot = Slot::new(SLOT).unwrap();
    assert!(!model().slot_active(&before, slot).unwrap(), "slot {SLOT} holds a profile");
    let path = dir.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; move the old backup away", path.display()));
    file.write_all(&before).and_then(|()| file.sync_all()).unwrap();
    println!("saved the bank to {}", path.display());
    before
}

/// Writes `slot` with the check's profile, sticks unswapped, and `macros`, then checks
/// that each macro's steps read back as written.
pub fn put(pad: &Pad, mode: Mode, slot: Slot, macros: &[MacroDefinition]) {
    let mut profile = profile_json(mode);
    profile["sticks"]["swap_sticks"] = serde_json::json!(false);
    let macro_edits = macros
        .iter()
        .map(|def| MacroEdit::Put {
            macro_slot: MacroSlot::new(def.macro_slot.unwrap()).unwrap(),
            definition: def.clone(),
        })
        .collect();
    let op = WriteOp::Upload { profile: Some(profile), drop_macros: Vec::new(), macro_edits };
    let dir = backup_dir();
    let writer = ProfileWriteOrchestrator::new(&pad.dev, model(), &dir);
    for result in writer.write_slots(&[WriteJob { mode, slot, op }], &ConfirmPolicy::Force) {
        assert!(result.success, "write: {}", result.message);
    }
    for def in macros {
        let macro_slot = MacroSlot::new(def.macro_slot.unwrap()).unwrap();
        let stream = model().encode_macro_steps(&def.steps, mode).unwrap();
        let used = def.steps.len() * STEP_LEN;
        let read = pad.dev.read_macro_stream(mode, slot, macro_slot, def.steps.len()).unwrap();
        assert_eq!(
            read[..used],
            stream[..used],
            "the pad reads macro {} as written",
            macro_slot.get() + 1
        );
    }
}

#[test]
#[ignore = "needs an Ultimate 2 on USB in XInput and a person; writes slot 3, then restores"]
fn probe() {
    let mode = XINPUT;
    let pad = pad_in(mode);
    let before = save_bank(&pad, mode, "ultimate-2-step-probe-xinput.bin");
    put(&pad, mode, Slot::new(SLOT).unwrap(), &[full_macro(mode, STEPS)]);
    println!("wrote the {STEPS}-step probe macro on X");

    println!("\nSelect profile 3 on the pad with its profile button, then press Enter.");
    line();
    let mut input = Input::open(&pad.port);
    input.forget();
    println!("Press X once and touch nothing else. The macro runs about 20 s.");
    println!("When the pad has stopped sending A and B, press Enter.");
    line();
    let events = input.drain();
    let count = |code| {
        events.iter().filter(|&&(kind, c, value)| kind == EV_KEY && c == code && value == 1).count()
    };
    let (a, b) = (count(BTN_A), count(BTN_B));
    println!("A presses: {a} of {}", VENDOR_STEPS / 2);
    println!("B presses: {b} of {}", (STEPS - VENDOR_STEPS) / 2);
    match (a, b) {
        (a, b) if a == VENDOR_STEPS / 2 && b == (STEPS - VENDOR_STEPS) / 2 => {
            println!("the pad played every step");
        }
        (a, 0) if a == VENDOR_STEPS / 2 => println!("the pad stopped at step {VENDOR_STEPS}"),
        _ => println!("the pad stopped near step {}", 2 * (a + b)),
    }

    hw::write_bank(&pad.dev, mode, &before);
    assert!(hw::same_bank(&pad.bank(mode), &before), "the pad differs from the saved bank");
    println!("\nthe pad holds the saved bank again");
}

/// What a long macro 1 does in `DInput` slot 3. The pad sets its byte at
/// [`RECORD_OFFSET`] to `0x3C` when it reads `0xFF` there at a `DInput` start-up, and
/// erases page 1 when it reads `0x3C`. This puts a [`STEPS`]-step macro 1, or as many as
/// `ULTIMATE_2_PROBE_STEPS` says, and a one-step macro 2. Then after each switch to
/// `XInput` and back it prints the byte, whether macro 1's steps are intact, and whether
/// page 1 holds data, and saves page 0.
///
/// Firmware 1.10: when the padded write of macro 1 ends by `0x0C00` (307 steps or fewer),
/// the byte reads `3c` from trip 1, macro 1 stays intact through 4 trips, and page 1 is
/// erased from trip 2, as the macro 2 warning says. When it passes `0x0C00` (308, 355,
/// 406 and 408 steps), the first `DInput` start-up erases page 0, macro 1 with it.
#[test]
#[ignore = "needs an Ultimate 2 on USB in DInput and a person; writes slot 3, then restores"]
fn record_probe() {
    let (mode, slot) = (DINPUT, Slot::new(SLOT).unwrap());
    let steps = std::env::var("ULTIMATE_2_PROBE_STEPS").map_or(STEPS, |n| n.parse().unwrap());
    println!("macro 1 has {steps} steps");
    let full = full_macro(mode, steps);
    let short = MacroDefinition {
        name: "Page 1".into(),
        trigger_buttons: vec!["d-pad up".into()],
        macro_slot: Some(1),
        steps: vec![MacroStep {
            duration_ms: STEP_MS,
            pressed_buttons: vec!["bottom face".into()],
            ..MacroStep::default()
        }],
        ..full
    };
    let stream = model().encode_macro_steps(&full.steps, mode).unwrap();
    let look = |pad: &Pad, when: &str| {
        let page =
            |n| pad.dev.read_macro_stream(mode, slot, MacroSlot::new(n).unwrap(), PAGE_STEPS);
        let (first, second) = (page(0).unwrap(), page(1).unwrap());
        let used = steps * STEP_LEN;
        let differ: Vec<usize> = (0..used).filter(|&i| first[i] != stream[i]).collect();
        let macro_1 = match differ.as_slice() {
            [] => "intact".to_owned(),
            _ if first[..used].iter().all(|&b| b == 0xFF) => "ERASED".to_owned(),
            [at, ..] => format!(
                "CHANGED in {} bytes, from {at:#06x} ({:02x}, written {:02x}) to {:#06x}",
                differ.len(),
                first[*at],
                stream[*at],
                differ[differ.len() - 1]
            ),
        };
        let held = if second.iter().any(|&b| b != 0xFF) { "holds" } else { "ERASED" };
        println!("{when}: byte {:02x}, macro 1 {macro_1}, page 1 {held}", first[RECORD_OFFSET]);
        let name = format!("ultimate-2-record-probe-{steps}-{}.bin", when.replace(' ', "-"));
        std::fs::write(backup_dir().join(name), &first).unwrap();
    };

    let mut pad = pad_in(mode);
    let before = save_bank(&pad, mode, &format!("ultimate-2-record-probe-dinput-{steps}.bin"));
    put(&pad, mode, slot, &[full, short]);
    look(&pad, "after the write");
    let trips = std::env::var("ULTIMATE_2_PROBE_TRIPS").map_or(TRIPS, |n| n.parse().unwrap());
    for trip in 1..=trips {
        drop(pad);
        pad = pad_in(XINPUT);
        drop(pad);
        pad = pad_in(mode);
        look(&pad, &format!("trip {trip}"));
    }

    // Leave page 0 as any short macro 1 leaves it: erased, so the byte reads 0xFF.
    let one_step = model().encode_macro_steps(&[MacroStep::default()], mode).unwrap();
    let back = pad.dev.begin_write().unwrap();
    let sent = pad.dev.write_macro_stream(mode, slot, MacroSlot::new(0).unwrap(), &one_step);
    pad.dev.end_write(back).unwrap();
    sent.unwrap();
    look(&pad, "after a one-step macro 1");

    hw::write_bank(&pad.dev, mode, &before);
    assert!(hw::same_bank(&pad.bank(mode), &before), "the pad differs from the saved bank");
    println!("\nthe pad holds the saved bank again");
}
