//! Hardware probe: what does the Pro 3 do with a macro trigger, step or step count the
//! encoder never writes? It puts one macro into an occupied slot, leaves it on the pad for
//! a person to trigger and for evdev to capture, and later puts the bank back from a dump.
//! `CASE` picks the macro:
//!
//! - `backup`: writes `BACKUP_DIR/raw-blob-<n>.bin` for the three banks. Writes nothing to the pad.
//! - `restore`: the bank returns to that dump and the macro page is erased.
//! - `dtrig`: `DInput` step with `trigger_value` L2 128, R2 64 and `keys` bits 14 and 15 clear.
//! - `dtrig2`: as `dtrig`, with bottom face pressed in the same step, to show the macro ran.
//! - `held`: bottom face for one step, then bottom face and right face.
//! - `plain`: one bottom face press. With `KEYMAP` (hex) the descriptor's `key_map` is
//!   replaced, so one macro can have several trigger bits or none.
//! - `glide`: left stick x from 0 to 255 in `GSTEP` increments, `STEP_MS` per step.
//! - `count`: `STEPS` steps of 20 ms, each moving the left stick x to a new value.
//!
//! `MODE` is `xinput`, `switch` or `dinput`, and must be the position of the mode switch on
//! the pad. `SLOT` is the profile slot, `MACRO_SLOT` the macro slot. Run with:
//!   `CASE=held MODE=xinput SLOT=2 MACRO_SLOT=2 cargo test -p controller-core --features
//!    hardware --test pro3 hardware_macro_probe -- --ignored --nocapture`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
use controller_core::model::{
    MacroDefinition, MacroSlot, MacroStep, Mode, RawProfilePayload, Slot,
};
use controller_core::protocol::crc16::crc16_modbus;
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
/// `keys` bits 14 and 15, the L2 and R2 flags.
const TRIGGER_BITS: u16 = 0xC000;
/// Long enough for evdev to see a step.
const HOLD_MS: u16 = 500;
/// Start of the macro section for profile slot 1, then the stride per slot and the header.
const SECTION4: usize = 0x068C;
const SLOT_STRIDE: usize = 216;
const SLOT_HEADER: usize = 8;
/// Bytes per macro descriptor, and where its `key_map` sits.
const DESCRIPTOR_LEN: usize = 52;
const KEY_MAP_AT: usize = 40;
/// Where the blob CRC sits.
const CRC_AT: usize = 0x0C;

fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

fn env_or(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

fn mode_and_bank() -> (Mode, usize) {
    match env("MODE").as_str() {
        "xinput" => (XINPUT, 0),
        "switch" => (SWITCH, 1),
        "dinput" => (DINPUT, 2),
        other => panic!("MODE is xinput, switch or dinput, not {other}"),
    }
}

fn button_step(buttons: &[&str], duration_ms: u16) -> MacroStep {
    MacroStep {
        duration_ms,
        pressed_buttons: buttons.iter().map(|b| (*b).to_owned()).collect(),
        ..MacroStep::default()
    }
}

/// The steps of `case`.
fn steps(case: &str) -> Vec<MacroStep> {
    let release = MacroStep::default();
    match case {
        "dtrig" => vec![
            MacroStep {
                duration_ms: HOLD_MS,
                trigger_left: 128,
                trigger_right: 64,
                ..MacroStep::default()
            },
            release,
        ],
        "dtrig2" => vec![
            MacroStep {
                duration_ms: HOLD_MS,
                pressed_buttons: vec!["bottom face".into()],
                trigger_left: 128,
                trigger_right: 64,
                ..MacroStep::default()
            },
            release,
        ],
        "held" => vec![
            button_step(&["bottom face"], HOLD_MS),
            button_step(&["bottom face", "right face"], HOLD_MS),
            release,
        ],
        "plain" => vec![button_step(&["bottom face"], HOLD_MS), release],
        "glide" => {
            let step_ms: u16 = env_or("STEP_MS", "20").parse().unwrap();
            let gstep: usize = env_or("GSTEP", "1").parse().unwrap();
            let mut out: Vec<MacroStep> = (0..=255u8)
                .step_by(gstep)
                .map(|x| MacroStep {
                    duration_ms: step_ms,
                    left_stick_x: x,
                    left_stick_y: 128,
                    ..MacroStep::default()
                })
                .collect();
            out.push(release);
            out
        }
        "count" => {
            let n: usize = env("STEPS").parse().unwrap();
            let mut out: Vec<MacroStep> = (0..n - 1)
                .map(|i| MacroStep {
                    duration_ms: 20,
                    // Neighbours always differ, and no step is the neutral value.
                    left_stick_x: u8::try_from(1 + (i % 2) * 100 + i / 2 % 100).unwrap(),
                    left_stick_y: 128,
                    ..MacroStep::default()
                })
                .collect();
            out.push(release);
            out
        }
        other => panic!("CASE is backup, restore, dtrig, held, plain, glide or count, not {other}"),
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
fn probe_macro_triggers() {
    let dev = seed::pad();
    let case = env("CASE");

    if case == "backup" {
        let dir = env("BACKUP_DIR");
        for (bank, blob) in dev.read_all_profiles().unwrap().raw_blobs.iter().enumerate() {
            std::fs::write(format!("{dir}/raw-blob-{bank}.bin"), blob).unwrap();
            println!("BACKUP bank {bank}: {} bytes", blob.len());
        }
        return;
    }

    let (mode, bank) = mode_and_bank();
    let slot = Slot::new(env("SLOT").parse().unwrap()).unwrap();
    let macro_slot = MacroSlot::new(env_or("MACRO_SLOT", "0").parse().unwrap()).unwrap();

    if case == "restore" {
        let saved = std::fs::read(format!("{}/raw-blob-{bank}.bin", env("BACKUP_DIR"))).unwrap();
        with_write(&dev, || {
            dev.send_slot_select(mode)?;
            dev.write_full_profile(mode, &saved)?;
            dev.send_apply(mode)?;
            for n in 0..4 {
                dev.erase_macro(mode, slot, MacroSlot::new(n).unwrap())?;
                dev.send_apply(mode)?;
            }
            Ok(())
        });
        let now = dev.read_all_profiles().unwrap().raw_blobs[bank].clone();
        assert!(crate::hw::same_bank(&now, &saved), "bank restored");
        println!("RESTORED {mode} slot {slot:?}");
        return;
    }

    let def = MacroDefinition {
        name: format!("P-{case}"),
        mode,
        trigger_buttons: vec![env_or("TRIGGER", "r4")],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(macro_slot.get()),
        steps: steps(&case),
    };
    let mut stream = Pro3.encode_macro_steps(&def.steps, mode).unwrap();
    if case == "dtrig" || case == "dtrig2" {
        let first = &mut stream[..STEP_LEN];
        let keys = u16::from_le_bytes([first[KEYS_AT], first[KEYS_AT + 1]]) & !TRIGGER_BITS;
        first[KEYS_AT..KEYS_AT + 2].copy_from_slice(&keys.to_le_bytes());
        first[TRIGGER_AT..TRIGGER_AT + 2].copy_from_slice(&((128u16 << 8) | 64).to_le_bytes());
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
    let compiled =
        Pro3.compile_profile(&profile, slot, &before, std::slice::from_ref(&def)).unwrap();
    let table = SECTION4
        + (usize::from(slot.get()) - 1) * SLOT_STRIDE
        + SLOT_HEADER
        + usize::from(macro_slot.get()) * DESCRIPTOR_LEN;
    // Only this macro's descriptor changes: the slot's other macros stay as the pad holds them.
    let mut blob = before;
    blob[table..table + DESCRIPTOR_LEN].copy_from_slice(&compiled[table..table + DESCRIPTOR_LEN]);
    if let Ok(key_map) = std::env::var("KEYMAP") {
        let key_map = u32::from_str_radix(&key_map, 16).unwrap();
        blob[table + KEY_MAP_AT..table + KEY_MAP_AT + 4].copy_from_slice(&key_map.to_le_bytes());
    }
    blob[CRC_AT..CRC_AT + 4].fill(0);
    let crc = crc16_modbus(&blob);
    blob[CRC_AT..CRC_AT + 2].copy_from_slice(&crc.to_le_bytes());

    with_write(&dev, || {
        dev.send_slot_select(mode)?;
        dev.write_full_profile(mode, &blob)?;
        dev.send_apply(mode)?;
        dev.write_macro_stream(mode, slot, macro_slot, &stream)?;
        dev.send_apply(mode)
    });

    let read = dev.read_all_profiles().unwrap();
    let held = &read.raw_blobs[bank];
    println!("DESCRIPTOR WRITTEN {}", hex(&blob[table..table + DESCRIPTOR_LEN]));
    println!("DESCRIPTOR READ    {}", hex(&held[table..table + DESCRIPTOR_LEN]));
    println!(
        "DESCRIPTOR MATCH {}",
        held[table..table + DESCRIPTOR_LEN] == blob[table..table + DESCRIPTOR_LEN]
    );
    let back = dev.read_macro_stream(mode, slot, macro_slot, def.steps.len()).unwrap();
    let want = &stream[..def.steps.len() * STEP_LEN];
    println!(
        "STREAM MATCH {} ({} of {} bytes read)",
        back.get(..want.len()) == Some(want),
        back.len(),
        want.len()
    );
    println!(
        "PUT {mode} slot {slot:?} macro slot {macro_slot:?} ({case}, {} steps).",
        def.steps.len()
    );
}
