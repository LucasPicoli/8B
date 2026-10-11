//! Hardware probe: does a stick-and-trigger macro play on a Pro 3? It writes one macro
//! into an occupied slot, leaves it on the pad for a person to trigger and for evdev to
//! capture, and later puts the bank back from a dump. Three phases, set by `PLAY`:
//!
//! - `trigger-value`: L2 and R2 go in `trigger_value`, as the encoder writes them.
//! - `keys-bits`: L2 and R2 go in `keys` bits 14 and 15, as the vendor app writes them.
//! - `restore`: the bank returns to `BACKUP_DIR/raw-blob-<n>.bin` and the page is erased.
//!
//! `MODE` is `xinput`, `switch` or `dinput`, and must be the position of the mode switch
//! on the pad. `SLOT` is the profile slot, `MACRO_SLOT` the macro slot. Run with:
//!   `PLAY=trigger-value MODE=xinput SLOT=2 MACRO_SLOT=2 cargo test -p controller-core
//!    --features hardware --test pro3 hardware_macro_play -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
use controller_core::model::{
    MacroDefinition, MacroSlot, MacroStep, Mode, RawProfilePayload, Slot,
};
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;
use std::fmt::Write as _;

use crate::seed;

/// Bytes per macro step on the wire.
const STEP_LEN: usize = 10;
/// Where `keys` sits in a step record.
const KEYS_AT: usize = 2;
/// Where `trigger_value` sits in a step record.
const TRIGGER_AT: usize = 4;
/// `keys` bit of L2, as the vendor app writes it.
const L2_BIT: u16 = 0x4000;
/// `keys` bit of R2, as the vendor app writes it.
const R2_BIT: u16 = 0x8000;
/// Long enough for evdev to see a step.
const HOLD_MS: u16 = 500;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

fn mode_and_bank() -> (Mode, usize) {
    match env("MODE").as_str() {
        "xinput" => (XINPUT, 0),
        "switch" => (SWITCH, 1),
        "dinput" => (DINPUT, 2),
        other => panic!("MODE is xinput, switch or dinput, not {other}"),
    }
}

fn steps() -> Vec<MacroStep> {
    vec![
        MacroStep {
            duration_ms: HOLD_MS,
            left_stick_x: 200,
            left_stick_y: 30,
            trigger_left: 255,
            trigger_right: 128,
            ..MacroStep::default()
        },
        MacroStep {
            duration_ms: HOLD_MS,
            right_stick_x: 64,
            right_stick_y: 192,
            ..MacroStep::default()
        },
        // The last step releases everything, as the fixture's does.
        MacroStep::default(),
    ]
}

/// The step stream as the vendor app writes `XInput` triggers: bits 14 and 15 of `keys`,
/// `trigger_value` zero.
fn keys_bits(stream: &mut [u8], count: usize) {
    for i in 0..count {
        let at = i * STEP_LEN;
        let value = u16::from_le_bytes([stream[at + TRIGGER_AT], stream[at + TRIGGER_AT + 1]]);
        let mut keys = u16::from_le_bytes([stream[at + KEYS_AT], stream[at + KEYS_AT + 1]]);
        if value >> 8 > 0 {
            keys |= L2_BIT;
        }
        if value & 0xFF > 0 {
            keys |= R2_BIT;
        }
        stream[at + KEYS_AT..at + KEYS_AT + 2].copy_from_slice(&keys.to_le_bytes());
        stream[at + TRIGGER_AT..at + TRIGGER_AT + 2].fill(0);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Runs `packets` with the pad flipped for writes, and flips it back.
fn with_write<T>(dev: &HidrawDevice, packets: impl FnOnce() -> controller_core::Result<T>) -> T {
    let back_to = dev.begin_write().unwrap();
    let sent = packets();
    if let Some(mode) = back_to {
        dev.end_write(mode).unwrap();
    }
    sent.unwrap()
}

#[test]
#[ignore = "writes a macro to, or restores, an attached 8BitDo Pro 3"]
#[serial]
fn play_stick_and_trigger_macro() {
    let dev = seed::pad();
    let (mode, bank) = mode_and_bank();
    let slot = Slot::new(env("SLOT").parse().unwrap()).unwrap();
    let macro_slot = MacroSlot::new(env("MACRO_SLOT").parse().unwrap()).unwrap();
    let phase = env("PLAY");

    if phase == "restore" {
        let saved = std::fs::read(format!("{}/raw-blob-{bank}.bin", env("BACKUP_DIR"))).unwrap();
        with_write(&dev, || {
            dev.send_slot_select(mode)?;
            dev.write_full_profile(mode, &saved)?;
            dev.send_apply(mode)?;
            dev.erase_macro(mode, slot, macro_slot)?;
            dev.send_apply(mode)
        });
        assert_eq!(dev.read_all_profiles().unwrap().raw_blobs[bank], saved, "bank restored");
        println!("RESTORED {mode} slot {slot:?}");
        return;
    }

    let def = MacroDefinition {
        name: "SticksTrig".into(),
        mode,
        trigger: "r4".into(),
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(macro_slot.get()),
        steps: steps(),
    };
    let mut stream = Pro3.encode_macro_steps(&def.steps, mode).unwrap();
    if phase == "keys-bits" {
        keys_bits(&mut stream, def.steps.len());
    } else {
        assert_eq!(phase, "trigger-value");
    }
    println!("STREAM {}", hex(&stream[..def.steps.len() * STEP_LEN]));

    let before = dev.read_all_profiles().unwrap().raw_blobs.remove(bank);
    let raw = RawProfilePayload {
        payload: before.clone(),
        source_slot: slot.get(),
        source_profile_index: slot.get() - 1,
        mode_hint: mode,
    };
    let profile = Pro3.map_profile(&raw).unwrap().canonical;
    assert!(profile.macro_refs.is_empty(), "the slot starts without macros");
    let blob = Pro3.compile_profile(&profile, slot, &before, std::slice::from_ref(&def)).unwrap();
    let section = 0x068C + (usize::from(slot.get()) - 1) * 216 + 8;
    let table = section + usize::from(macro_slot.get()) * 52;
    let strays: Vec<usize> = (0..blob.len())
        .filter(|&i| {
            blob[i] != before[i] && !(table..table + 52).contains(&i) && !(0x0C..0x10).contains(&i)
        })
        .collect();
    assert!(strays.is_empty(), "the compile changes bytes outside the descriptor: {strays:x?}");

    with_write(&dev, || {
        dev.send_slot_select(mode)?;
        dev.write_full_profile(mode, &blob)?;
        dev.send_apply(mode)?;
        dev.write_macro_stream(mode, slot, macro_slot, &stream)?;
        dev.send_apply(mode)
    });

    let read = dev.read_all_profiles().unwrap();
    let held = &read.raw_blobs[bank];
    assert_eq!(held[table..table + 52], blob[table..table + 52], "descriptor reads back");
    let back = dev.read_macro_stream(mode, slot, macro_slot, def.steps.len()).unwrap();
    assert_eq!(back[..stream.len().min(back.len())], stream[..back.len().min(stream.len())]);
    println!("READBACK {}", hex(&back));
    println!("PUT {mode} slot {slot:?} macro slot {macro_slot:?} ({phase}). Trigger r4 now.");
}
