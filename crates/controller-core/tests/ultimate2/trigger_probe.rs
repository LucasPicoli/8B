//! Probe: which `trigger_value` of a macro step does the Ultimate 2 play, and on which
//! trigger? The codec stores step triggers as `keys` bits, on or off, as the vendor app
//! does. This writes the raw `trigger_value` forms into one macro and logs `ABS_Z` (LT)
//! and `ABS_RZ` (RT) while a person plays it. Run once, at the desk, in `XInput`:
//!   `cargo test -p controller-core --features hardware --test ultimate2 -- --ignored
//!    --nocapture --exact trigger_probe::probe`
//!
//! It saves the bank first, refuses an occupied slot 3, and writes the bank back at the
//! end. Nothing retries or resets USB.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::Write as _;

use crate::fixture::{profile_json, SLOT};
use crate::guided::{backup_dir, line, model, pad_in};
use crate::hw;
use crate::input::Input;
use controller_core::devices::pro3::XINPUT;
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Slot};
use controller_core::orchestrator::{MacroEdit, ProfileWriteOrchestrator, WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;

/// Bytes per step record, and where `keys` and `trigger_value` sit in one.
const STEP_LEN: usize = 10;
const KEYS_AT: usize = 2;
const TRIGGER_AT: usize = 4;
/// The `keys` bits the codec uses for L2 and R2.
const TRIGGER_BITS: u16 = 0xC000;
/// Each case: the raw `trigger_value` a step holds for [`HOLD_MS`], then a release.
const CASES: [u16; 4] = [0xFF00, 0x00FF, 0x8040, 0x4080];
const HOLD_MS: u16 = 1000;
const REST_MS: u16 = 600;
/// `EV_ABS` codes of the triggers.
const EV_ABS: u16 = 3;
const ABS_Z: u16 = 2;
const ABS_RZ: u16 = 5;

/// The macro on X: one hold and one release per case. The codec fills the sticks and
/// the descriptor; the probe swaps in the raw trigger forms.
fn probe_macro() -> MacroDefinition {
    let steps = (0..CASES.len())
        .flat_map(|i| {
            let rest_ms = if i + 1 == CASES.len() { 0 } else { REST_MS };
            [
                MacroStep { duration_ms: HOLD_MS, trigger_left: 255, ..MacroStep::default() },
                MacroStep { duration_ms: rest_ms, ..MacroStep::default() },
            ]
        })
        .collect();
    MacroDefinition {
        name: "Trigger probe".into(),
        mode: XINPUT,
        trigger_buttons: vec!["left face".into()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(0),
        steps,
    }
}

/// `stream` with each hold step's triggers as the raw [`CASES`] value and no `keys` bit.
fn raw_cases(mut stream: Vec<u8>) -> Vec<u8> {
    for (i, value) in CASES.iter().enumerate() {
        let at = 2 * i * STEP_LEN;
        let keys = u16::from_le_bytes([stream[at + KEYS_AT], stream[at + KEYS_AT + 1]]);
        stream[at + KEYS_AT..at + KEYS_AT + 2]
            .copy_from_slice(&(keys & !TRIGGER_BITS).to_le_bytes());
        stream[at + TRIGGER_AT..at + TRIGGER_AT + 2].copy_from_slice(&value.to_le_bytes());
    }
    stream
}

#[test]
#[ignore = "needs an Ultimate 2 on USB in XInput and a person; writes slot 3, then restores"]
fn probe() {
    let mode = XINPUT;
    let slot = Slot::new(SLOT).unwrap();
    let macro_slot = MacroSlot::new(0).unwrap();
    let dir = backup_dir();
    let pad = pad_in(mode);
    let before = pad.bank(mode);
    assert!(!model().slot_active(&before, slot).unwrap(), "slot {SLOT} holds a profile");
    let path = dir.join("ultimate-2-trigger-probe-xinput.bin");
    std::fs::create_dir_all(&dir).unwrap();
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap_or_else(|e| panic!("{}: {e}; move the old backup away", path.display()));
    file.write_all(&before).and_then(|()| file.sync_all()).unwrap();
    println!("saved the bank to {}", path.display());

    // The check's profile, sticks unswapped, with the probe macro on X.
    let mut profile = profile_json(mode);
    profile["sticks"]["swap_sticks"] = serde_json::json!(false);
    let def = probe_macro();
    let op = WriteOp::Upload {
        profile: Some(profile),
        drop_macros: Vec::new(),
        macro_edits: vec![MacroEdit::Put { macro_slot, definition: def.clone() }],
    };
    let writer = ProfileWriteOrchestrator::new(&pad.dev, model(), &dir);
    for result in writer.write_slots(&[WriteJob { mode, slot, op }], &ConfirmPolicy::Force) {
        assert!(result.success, "write: {}", result.message);
    }
    let stream = raw_cases(model().encode_macro_steps(&def.steps, mode).unwrap());
    let back = pad.dev.begin_write().unwrap();
    let sent = pad
        .dev
        .write_macro_stream(mode, slot, macro_slot, &stream)
        .and_then(|()| pad.dev.send_apply(mode));
    pad.dev.end_write(back).unwrap();
    sent.unwrap();
    let used = def.steps.len() * STEP_LEN;
    let read = pad.dev.read_macro_stream(mode, slot, macro_slot, def.steps.len()).unwrap();
    assert_eq!(read[..used], stream[..used], "the pad reads the macro back as written");
    println!("wrote the probe macro on X");

    println!("\nSelect profile 3 on the pad with its profile button, then press Enter.");
    line();
    let mut input = Input::open(&pad.port);
    input.forget();
    println!("Press X once, without touching the triggers. The macro runs 6 s.");
    println!("When it has finished, press Enter.");
    line();
    let log: Vec<String> = input
        .drain()
        .into_iter()
        .filter_map(|(kind, code, value)| match (kind, code) {
            (EV_ABS, ABS_Z) => Some(format!("LT {value}")),
            (EV_ABS, ABS_RZ) => Some(format!("RT {value}")),
            _ => None,
        })
        .collect();
    println!("trigger events, in order: {log:?}");
    for (i, value) in CASES.iter().enumerate() {
        println!("  case {}: trigger_value {value:#06x}", i + 1);
    }

    hw::write_bank(&pad.dev, mode, &before);
    assert!(hw::same_bank(&pad.bank(mode), &before), "the pad differs from the saved bank");
    println!("\nthe pad holds the saved bank again");
}
