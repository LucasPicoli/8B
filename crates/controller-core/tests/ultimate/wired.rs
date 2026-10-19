//! The Ultimate Wired model: its description through the shared driver, in both modes.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::description::firmware_allows;
use controller_core::description::ControllerDescription;
use controller_core::device::Model;
use controller_core::devices::ultimate::macros::decode_macros;
use controller_core::devices::ultimate::tables::{
    GAMEPAD_MODE_OFFSET, MACROS_OFFSET, MACRO_RECORDS_AT, MAP_OFFSET, RECORD_CODES_AT,
};
use controller_core::devices::ultimate::{image_mode, XINPUT};
use controller_core::devices::{model_named, refused_with};
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroStep, Mode, RawProfilePayload, Slot,
};
use controller_core::protocol::bytes::{read_u16_le, read_u32_le};

const DINPUT: Mode = Mode::from_static("dinput");

fn wired() -> &'static dyn Model {
    model_named("Ultimate Wired").unwrap()
}

fn d() -> &'static ControllerDescription {
    wired().description().unwrap()
}

fn target<'a>(p: &'a CanonicalProfile, id: &str) -> &'a str {
    &p.button_mappings.iter().find(|m| m.source == id).unwrap().target
}

fn macro_on(button: &str, mode: Mode) -> MacroDefinition {
    MacroDefinition {
        name: String::new(),
        mode,
        trigger_buttons: vec!["l1".to_owned()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        steps: vec![MacroStep {
            duration_ms: 100,
            pressed_buttons: vec![button.to_owned()],
            ..MacroStep::default()
        }],
        macro_slot: Some(0),
    }
}

#[test]
fn the_wired_pad_is_named_by_its_config_port_and_is_refused_until_validated() {
    let d = d();
    assert_eq!(
        (d.display_name.as_str(), d.family_name.as_deref()),
        ("Ultimate Wired", Some("8BitDo Ultimate"))
    );
    assert_eq!(d.config_ports.len(), 1);
    assert_eq!(d.config_ports[0].usb.product, 0x3011);
    assert_eq!(d.lock.as_ref().unwrap().locked.usb.product, 0x3106);
    assert_eq!(d.lock.as_ref().unwrap().locked.product, "8BitDo Ultimate Wired Controller");
    assert!(!d.validated);
    assert!(refused_with(d, false));
    assert!(!refused_with(d, true));
    let modes: Vec<(&str, Option<u8>)> =
        d.modes.iter().map(|m| (m.id.as_str(), m.gamepad_mode)).collect();
    assert_eq!(modes, [("xinput", Some(3)), ("dinput", Some(1))]);
}

#[test]
fn the_face_buttons_follow_the_swap_in_the_map_but_not_in_macros() {
    let d = d();
    // id, print, map index, map code, macro code
    let faces = [
        ("right face", "B", 0, 0x1000, 0x1000),
        ("bottom face", "A", 1, 0x2000, 0x2000),
        ("top face", "Y", 2, 0x20, 0x20),
        ("left face", "X", 3, 0x10, 0x10),
    ];
    for (id, print, index, map, mac) in faces {
        let b = d.button(id).unwrap();
        assert_eq!((b.printed_label.as_deref(), b.map_index), (Some(print), Some(index)), "{id}");
        for mode in [XINPUT, DINPUT] {
            assert_eq!(b.map_codes[&mode], map, "{id} in {mode}");
            assert_eq!(b.labels[&mode], print, "{id} in {mode}");
        }
        assert_eq!(b.macro_code, Some(mac), "{id}");
    }
}

#[test]
fn each_mode_round_trips_a_synthetic_blob_and_a_macro_a_step_stores_0x2000() {
    let model = wired();
    let slot = Slot::new(1).unwrap();
    for (mode, byte) in [(XINPUT, 3), (DINPUT, 1)] {
        let profile = model.default_profile(mode);
        let def = macro_on("bottom face", mode);
        let blob = model.compile_profile(&profile, slot, &[], std::slice::from_ref(&def)).unwrap();
        assert_eq!(read_u16_le(&blob, GAMEPAD_MODE_OFFSET).unwrap(), byte);
        assert_eq!(image_mode(d(), &blob).unwrap(), mode);
        assert_eq!(read_u32_le(&blob, MAP_OFFSET + 4).unwrap(), 0x1000, "right face sends B");
        assert_eq!(read_u32_le(&blob, MAP_OFFSET + 4 + 4).unwrap(), 0x2000, "bottom face sends A");
        let record = MACROS_OFFSET + MACRO_RECORDS_AT;
        assert_eq!(read_u16_le(&blob, record + RECORD_CODES_AT).unwrap(), 0x2000, "macro A");
        let raw = RawProfilePayload {
            payload: blob.clone(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: mode,
        };
        assert_eq!(
            model.map_profile(&raw).unwrap().canonical.button_mappings,
            profile.button_mappings
        );
        assert_eq!(decode_macros(d(), &blob, slot).unwrap(), [def]);
    }
}

#[test]
fn dinput_defaults_p1_and_p2_to_themselves_and_xinput_disables_them() {
    let model = wired();
    let x = model.default_profile(XINPUT);
    let dinput = model.default_profile(DINPUT);
    assert_eq!((target(&dinput, "p1"), target(&dinput, "p2")), ("p1 output", "p2 output"));
    assert_eq!((target(&x, "p1"), target(&x, "p2")), ("disabled", "disabled"));
    assert_eq!((target(&x, "star"), target(&dinput, "star")), ("turbo", "turbo"));
    let blob = model.compile_profile(&dinput, Slot::new(1).unwrap(), &[], &[]).unwrap();
    let entry = |index: usize| read_u32_le(&blob, MAP_OFFSET + 4 + 4 * index).unwrap();
    assert_eq!((entry(18), entry(19)), (0x200_0000, 0x400_0000), "P1 and P2 send themselves");
    let outputs = |m: Mode| -> Vec<&str> {
        d().mode(m).unwrap().extra_outputs.iter().map(|o| o.id.as_str()).collect()
    };
    assert_eq!(outputs(XINPUT), ["turbo", "auto turbo", "swap"]);
    assert_eq!(outputs(DINPUT), ["turbo", "auto turbo", "swap", "p1 output", "p2 output"]);
}

#[test]
fn only_xinput_has_vibration_and_the_four_way_gate_is_the_wired_pads() {
    let d = d();
    let shown = |id: &str, m: Mode| d.pages(m).any(|p| p.id == id);
    assert!(shown("vibration", XINPUT) && !shown("vibration", DINPUT));
    let gates: Vec<_> = d
        .settings
        .iter()
        .flat_map(|p| &p.frames)
        .flat_map(|f| &f.flags)
        .filter(|f| f.field == "/sticks/dpad_four_way")
        .map(|f| f.min_firmware)
        .collect();
    assert_eq!(gates.len(), 2, "one per mode");
    for g in gates {
        assert!(!firmware_allows(g, "1.01") && firmware_allows(g, "1.02"));
    }
}

#[test]
fn p1_and_p2_are_outputs_in_dinput_only() {
    let d = d();
    assert!(d.validate_remap(DINPUT, "star", "p1 output").is_ok());
    assert!(d.validate_remap(DINPUT, "right face", "p2 output").is_ok());
    assert!(d.validate_remap(XINPUT, "star", "p1 output").is_err());
}
