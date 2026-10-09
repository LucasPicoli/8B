//! Hardware macro test: requires a physical 8BitDo Pro 3 on USB with a profile in
//! `XInput` slot 1. It puts the `x-s1-m0-buttons` fixture macro (trigger `rp`) into macro
//! slot 0 of that profile, in this order: the profile blob with the new
//! descriptor, then the step stream. It then reads both back. A second test gives `rp`
//! the output X and removes the macro, the way the window does on a pick. Run with:
//!   `cargo test -p controller-core --features hardware --test hardware_macro -- --ignored`
#![cfg(feature = "hardware")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{
    ButtonMapping, MacroDefinition, MacroRef, MacroSlot, MacroStep, Mode, RawProfilePayload, Slot,
};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

/// Section 4 record of profile slot 1: 216 bytes from `0x068C`.
const SLOT1_SECTION4: std::ops::Range<usize> = 0x068C..0x068C + 216;
/// The struct CRC.
const CRC: std::ops::Range<usize> = 0x0C..0x10;
const STREAM: &str = "../../fixtures/pro3/macros/x-s1-m0-buttons.steps.bin";

fn step(duration_ms: u16, press: &[&str]) -> MacroStep {
    MacroStep {
        duration_ms,
        pressed_buttons: press.iter().map(|s| (*s).to_owned()).collect(),
        ..MacroStep::default()
    }
}

/// The `x-s1-m0-buttons` fixture, as `fixture_macros.rs` builds it.
fn buttons_macro() -> MacroDefinition {
    MacroDefinition {
        name: "Buttons".into(),
        mode: Mode::XInput,
        trigger: "rp".into(),
        repeat_count: 1,
        interval_ms: 0,
        macro_slot: Some(0),
        steps: vec![
            step(50, &["bottom face"]),
            step(100, &["top face", "r1"]),
            step(0, &["d-pad up", "l1"]),
        ],
    }
}

fn xinput_bank(dev: &HidrawDevice) -> Vec<u8> {
    dev.read_all_profiles().unwrap().raw_blobs.remove(0)
}

#[test]
#[ignore = "writes a macro to XInput slot 1 of an attached 8BitDo Pro 3"]
#[serial]
fn put_fixture_macro_on_xinput_slot1() {
    let dev = HidrawDevice::open().unwrap();
    let slot = Slot::new(1).unwrap();
    let macro_slot = MacroSlot::new(0).unwrap();
    let def = buttons_macro();
    let stream = std::fs::read(STREAM).unwrap();
    assert_eq!(stream, Pro3.encode_macro_steps(&def.steps, Mode::XInput).unwrap());

    let before = xinput_bank(&dev);
    let raw = RawProfilePayload {
        payload: before.clone(),
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: Mode::XInput,
    };
    let profile = Pro3.map_profile(&raw).unwrap().canonical;
    assert!(profile.macro_refs.is_empty(), "slot 1 must start without macros");
    let blob = Pro3.compile_profile(&profile, slot, &before, std::slice::from_ref(&def)).unwrap();
    let outside: Vec<usize> = (0..blob.len())
        .filter(|&i| blob[i] != before[i] && !SLOT1_SECTION4.contains(&i) && !CRC.contains(&i))
        .collect();
    assert!(outside.is_empty(), "the compile changes bytes outside the macro table: {outside:x?}");

    let back_to = dev.begin_write().unwrap();
    let sent = (|| {
        dev.send_slot_select(Mode::XInput)?;
        dev.write_full_profile(Mode::XInput, &blob)?;
        dev.send_apply(Mode::XInput)?;
        dev.write_macro_stream(Mode::XInput, slot, macro_slot, &stream)?;
        dev.send_apply(Mode::XInput)?;
        dev.query_status(Mode::XInput)
    })();
    if let Some(mode) = back_to {
        dev.end_write(mode).unwrap();
    }
    sent.unwrap();

    let read = dev.read_all_profiles().unwrap();
    let refs = &read.profiles[0].canonical.macro_refs;
    let expected =
        MacroRef { trigger: "rp".to_owned(), path: "xinput-slot1-macro0-Buttons.json".to_owned() };
    assert_eq!(refs, &vec![expected]);
    let back = dev.read_macro_stream(Mode::XInput, slot, macro_slot, def.steps.len()).unwrap();
    assert_eq!(Pro3.decode_macro_steps(&back, def.steps.len(), Mode::XInput).unwrap(), def.steps);
}

#[test]
#[ignore = "removes the macro on rp from XInput slot 1 of an attached 8BitDo Pro 3"]
#[serial]
fn remove_fixture_macro_on_a_pick() {
    let dev = HidrawDevice::open().unwrap();
    let before = dev.read_all_profiles().unwrap();
    let mut profile = before.profiles[0].canonical.clone();
    assert!(profile.macro_refs.iter().any(|m| m.trigger == "rp"), "run the put test first");
    let x = ButtonMapping { source: "rp".to_owned(), target: "left face".to_owned() };
    profile.button_mappings.retain(|m| m.source != "rp");
    profile.button_mappings.push(x.clone());

    let dir = tempfile::tempdir().unwrap();
    let r = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path()).upload_profile_dropping_macros(
        &serde_json::to_value(&profile).unwrap(),
        Mode::XInput,
        Slot::new(1).unwrap(),
        &["rp".to_owned()],
        &ConfirmPolicy::Force,
    );
    assert!(r.success, "{}", r.message);

    let after = dev.read_all_profiles().unwrap();
    let read = &after.profiles[0].canonical;
    assert!(read.macro_refs.is_empty(), "the macro is gone: {:?}", read.macro_refs);
    assert!(read.button_mappings.contains(&x));
    assert_eq!(after.raw_blobs[1..], before.raw_blobs[1..], "the Switch and DInput banks stay");
}
