//! The Ultimate 2.4G: its description pinned to the codec, a synthetic image of each mode
//! through the driver, and the refusal of a pad in a mode it does not list.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::description::ControllerDescription;
use controller_core::device::{Model, UsbId};
use controller_core::devices::ultimate::tables::{GAMEPAD_MODE_OFFSET, MAP_OFFSET};
use controller_core::devices::ultimate::{image_mode, XINPUT};
use controller_core::devices::{find_model_by_port, model_named, models, refused_with};
use controller_core::model::{Mode, RawProfilePayload, Slot};
use controller_core::protocol::bytes::{put_u16_le, read_u32_le};
use serde_json::Value;

const DINPUT: Mode = Mode::from_static("dinput");

fn pad() -> &'static dyn Model {
    model_named("Ultimate 2.4G").unwrap()
}

fn d() -> &'static ControllerDescription {
    pad().description().unwrap()
}

#[test]
fn the_description_matches_its_schema() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../schemas/controller-description-v1.schema.json"
    ))
    .unwrap();
    let file: Value =
        serde_json::from_str(include_str!("../../controllers/ultimate-2-4g/description.json"))
            .unwrap();
    let validator = jsonschema::draft202012::new(&schema).unwrap();
    let errors: Vec<String> = validator.iter_errors(&file).map(|e| e.to_string()).collect();
    assert_eq!(errors, Vec::<String>::new());
}

#[test]
fn it_is_named_locked_and_not_yet_validated() {
    let d = d();
    assert_eq!(
        (d.display_name.as_str(), d.family_name.as_deref()),
        ("Ultimate 2.4G", Some("8BitDo Ultimate"))
    );
    let port = d.config_ports[0];
    assert_eq!(port.usb, UsbId { vendor: 0x2DC8, product: 0x3012 });
    assert_eq!(
        find_model_by_port(models(), port.usb).unwrap().description().unwrap().short_name,
        "Ultimate 2.4G"
    );
    let lock = d.lock.as_ref().unwrap();
    assert_eq!(lock.locked.usb, UsbId { vendor: 0x2DC8, product: 0x3106 });
    assert_eq!(lock.locked.product, "8BitDo Ultimate wireless Controller for PC");
    let bt = model_named("Ultimate (BT)").unwrap().description().unwrap().lock.as_ref().unwrap();
    assert_eq!(lock.unlock, bt.unlock, "the same unlock packets as the Bluetooth pad");
    assert_eq!(lock.relock, bt.relock);
    assert!(!d.validated);
    assert!(refused_with(d, false), "refused without EIGHTB_UNVALIDATED=1");
    assert!(!refused_with(d, true));
}

#[test]
fn it_lists_xinput_and_dinput_and_no_switch() {
    let modes: Vec<(Mode, Option<u8>)> = d().modes.iter().map(|m| (m.id, m.gamepad_mode)).collect();
    assert_eq!(modes, [(XINPUT, Some(3)), (DINPUT, Some(1))]);
    assert_eq!(d().mode(DINPUT).unwrap().label, "DInput");
    for mode in [XINPUT, DINPUT] {
        let m = d().mode(mode).unwrap();
        let ids: Vec<&str> = m.extra_outputs.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["turbo", "auto turbo", "swap"], "no Screenshot, no P1 or P2 output");
        let profile = pad().default_profile(mode);
        let target = |id: &str| {
            profile.button_mappings.iter().find(|b| b.source == id).unwrap().target.clone()
        };
        assert_eq!(
            (target("star"), target("p1"), target("p2")),
            ("turbo".into(), "disabled".into(), "disabled".into())
        );
    }
    assert!(pad().slot_select_value(Mode::from_static("switch")).is_err());
}

#[test]
fn the_face_slots_run_bottom_right_left_top_with_xbox_prints() {
    let rows = [
        ("bottom face", "A", 0, 0x2000),
        ("right face", "B", 1, 0x1000),
        ("left face", "X", 2, 0x10),
        ("top face", "Y", 3, 0x20),
    ];
    for (id, print, index, code) in rows {
        let b = d().button(id).unwrap();
        assert_eq!(b.printed_label.as_deref(), Some(print), "{id}");
        assert_eq!(b.map_index, Some(index), "{id}");
        for mode in [XINPUT, DINPUT] {
            assert_eq!(b.map_codes[&mode], code, "{id} {mode}");
            assert_eq!(b.labels[&mode], print, "an Xbox pad prints what it sends");
        }
        assert_eq!(b.macro_code, Some(code), "{id}");
    }
}

#[test]
fn each_mode_round_trips_through_the_driver() {
    let slot = Slot::new(1).unwrap();
    for (mode, byte) in [(XINPUT, 3), (DINPUT, 1)] {
        let profile = pad().default_profile(mode);
        let blob = pad().compile_profile(&profile, slot, &[], &[]).unwrap();
        assert_eq!(image_mode(d(), &blob).unwrap(), mode);
        assert_eq!(blob[GAMEPAD_MODE_OFFSET], byte);
        assert_eq!(pad().slot_select_value(mode).unwrap(), byte);
        let map = |i: usize| read_u32_le(&blob, MAP_OFFSET + 4 + 4 * i).unwrap();
        assert_eq!([map(0), map(1), map(2), map(3)], [0x2000, 0x1000, 0x10, 0x20], "{mode}");
        assert_eq!(map(12), 0x1_0000, "Turbo at Star");
        assert_eq!((map(18), map(19)), (0, 0), "nothing at P1 and P2");
        let raw = RawProfilePayload {
            payload: blob,
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: mode,
        };
        let back = pad().map_profile(&raw).unwrap().canonical;
        assert_eq!(back.button_mappings, profile.button_mappings, "{mode}");
    }
}

#[test]
fn a_pad_in_a_mode_the_description_lacks_is_told_to_switch() {
    let mut blob = pad()
        .compile_profile(&pad().default_profile(XINPUT), Slot::new(1).unwrap(), &[], &[])
        .unwrap();
    put_u16_le(&mut blob, GAMEPAD_MODE_OFFSET, 0).unwrap();
    let raw = RawProfilePayload {
        payload: blob,
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: XINPUT,
    };
    let message = pad().map_profile(&raw).unwrap_err().to_string();
    assert!(message.contains("a mode 8B does not support"), "{message}");
    assert!(message.contains("XInput or DInput"), "{message}");
    // The Bluetooth pad has a Switch mode, so byte 0 is a mode there.
    let bt = model_named("Ultimate (BT)").unwrap().description().unwrap();
    assert_eq!(image_mode(bt, &raw.payload).unwrap().as_str(), "switch");
}
