//! Hardware macro test: requires a physical 8BitDo Pro 3 on USB. It seeds the pad with
//! the fixture profiles, then puts the `x-s1-m0-buttons` fixture macro (trigger `rp`)
//! into macro slot 0 of `XInput` slot 1: the profile blob with the new descriptor, then
//! the step stream. It reads both back, then gives `rp` the output X and removes the
//! macro, the way the window does on a pick. The banks and the macro's flash page end
//! as they began. Run with:
//!   `cargo test -p controller-core --features hardware --test pro3 hardware_macro -- --ignored`
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::{Pro3, XINPUT};
use controller_core::model::{
    ButtonMapping, MacroDefinition, MacroRef, MacroSlot, MacroStep, RawProfilePayload, Slot,
};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::read::read_macros;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

use crate::seed;

/// Section 4 record of profile slot 1: 216 bytes from `0x068C`.
const SLOT1_SECTION4: std::ops::Range<usize> = 0x068C..0x068C + 216;
/// The struct CRC.
const CRC: std::ops::Range<usize> = 0x0C..0x10;
const STREAM: &str = "../../fixtures/pro3/macros/x-s1-m0-buttons.steps.bin";
/// Bytes per macro step on the wire.
const STEP_LEN: usize = 10;
/// A step stream is written in chunks of this many bytes.
const CHUNK_LEN: usize = 32;

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
        mode: XINPUT,
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

/// The bytes a macro of `steps` steps takes in its flash page, padded as a stream is.
const fn page_span(steps: usize) -> usize {
    (steps * STEP_LEN).div_ceil(CHUNK_LEN) * CHUNK_LEN
}

/// The written span of macro slot 0 of `XInput` slot 1, if a macro there uses it. Read
/// before the test writes the page, so it can be written back after.
fn saved_page(dev: &HidrawDevice) -> Option<Vec<u8>> {
    let slot = Slot::new(1).unwrap();
    let read = dev.read_all_profiles().unwrap();
    let occupied =
        read.profiles.iter().any(|p| p.mode == XINPUT && p.source_slot == 1 && !p.id.is_empty());
    // An empty slot has no macro, so nothing points at the page.
    if !occupied {
        return None;
    }
    let held = read_macros(dev, XINPUT, slot).unwrap();
    let steps = held.macros.iter().find(|m| m.macro_slot == Some(0))?.steps.len();
    let span = page_span(steps);
    let mut page = dev
        .read_macro_stream(XINPUT, slot, MacroSlot::new(0).unwrap(), span.div_ceil(STEP_LEN))
        .unwrap();
    page.truncate(span);
    Some(page)
}

#[test]
#[ignore = "writes and removes a macro on XInput slot 1 of an attached 8BitDo Pro 3"]
#[serial]
fn put_and_remove_the_fixture_macro_on_xinput_slot1() {
    let dev = seed::pad();
    let page = saved_page(&dev);
    let seed = seed::seed(&dev);
    put_fixture_macro(&dev);
    remove_it_on_a_pick(&dev);
    if let Some(page) = page {
        let (slot, macro_slot) = (Slot::new(1).unwrap(), MacroSlot::new(0).unwrap());
        dev.write_macro_stream(XINPUT, slot, macro_slot, &page).unwrap();
        dev.send_apply(XINPUT).unwrap();
    }
    drop(seed);
}

/// Puts the fixture macro on `rp` of `XInput` slot 1, which must hold no macro.
fn put_fixture_macro(dev: &HidrawDevice) {
    let slot = Slot::new(1).unwrap();
    let macro_slot = MacroSlot::new(0).unwrap();
    let def = buttons_macro();
    let stream = std::fs::read(STREAM).unwrap();
    assert_eq!(stream, Pro3.encode_macro_steps(&def.steps, XINPUT).unwrap());

    let before = xinput_bank(dev);
    let raw = RawProfilePayload {
        payload: before.clone(),
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: XINPUT,
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
        dev.send_slot_select(XINPUT)?;
        dev.write_full_profile(XINPUT, &blob)?;
        dev.send_apply(XINPUT)?;
        dev.write_macro_stream(XINPUT, slot, macro_slot, &stream)?;
        dev.send_apply(XINPUT)?;
        dev.query_status()
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
    let back = dev.read_macro_stream(XINPUT, slot, macro_slot, def.steps.len()).unwrap();
    assert_eq!(Pro3.decode_macro_steps(&back, def.steps.len(), XINPUT).unwrap(), def.steps);
}

/// Gives `rp` the output X and removes its macro, as the window does on a pick.
fn remove_it_on_a_pick(dev: &HidrawDevice) {
    let before = dev.read_all_profiles().unwrap();
    let mut profile = before.profiles[0].canonical.clone();
    assert!(profile.macro_refs.iter().any(|m| m.trigger == "rp"), "the put came first");
    let x = ButtonMapping { source: "rp".to_owned(), target: "left face".to_owned() };
    profile.button_mappings.retain(|m| m.source != "rp");
    profile.button_mappings.push(x.clone());

    let dir = tempfile::tempdir().unwrap();
    let r = ProfileWriteOrchestrator::new(dev, &Pro3, dir.path()).upload_profile_dropping_macros(
        &serde_json::to_value(&profile).unwrap(),
        XINPUT,
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
