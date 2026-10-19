//! A second Ultimate model, with codes of its own, through the same driver.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use std::sync::LazyLock;

use controller_core::description::ControllerDescription;
use controller_core::device::{ControllerSpec, ProtocolCodec};
use controller_core::devices::ultimate::macros::decode_macros;
use controller_core::devices::ultimate::tables::{
    GAMEPAD_MODE_OFFSET, MACROS_OFFSET, MACRO_RECORDS_AT, MAP_OFFSET, RECORD_CODES_AT,
    RECORD_TRIGGER_AT,
};
use controller_core::devices::ultimate::{image_mode, Ultimate, SWITCH, XINPUT};
use controller_core::model::{MacroDefinition, MacroStep, RawProfilePayload, Slot};
use controller_core::protocol::bytes::{read_u16_le, read_u32_le};
use serde_json::{json, Value};

/// The registered description with its own codes: the two face buttons swap their macro
/// codes, `right face` gets a map code that no other model uses, `XInput` selects its image
/// by 7, and P1 defaults to its own output in `XInput`.
fn variant() -> Result<&'static ControllerDescription, controller_core::Error> {
    static DESCRIPTION: LazyLock<Result<ControllerDescription, controller_core::Error>> =
        LazyLock::new(|| {
            let mut v: Value = serde_json::from_str(include_str!(
                "../../controllers/ultimate-bluetooth/description.json"
            ))
            .unwrap();
            let button = |v: &mut Value, id: &str| -> usize {
                v["buttons"].as_array().unwrap().iter().position(|b| b["id"] == id).unwrap()
            };
            let (right, bottom, p1) =
                (button(&mut v, "right face"), button(&mut v, "bottom face"), button(&mut v, "p1"));
            v["buttons"][right]["map_codes"]["xinput"] = json!("0x200000");
            v["buttons"][right]["macro_code"] = json!("0x1000");
            v["buttons"][bottom]["macro_code"] = json!("0x2000");
            v["buttons"][p1]["map_codes"] = json!({ "xinput": "0x2000000", "switch": "0x2000000" });
            v["modes"][0]["gamepad_mode"] = json!(7);
            v["modes"][0]["default_targets"]["p1"] = json!("p1");
            let svgs = [
                ("front.svg", include_str!("../../controllers/ultimate-bluetooth/front.svg")),
                ("back.svg", include_str!("../../controllers/ultimate-bluetooth/back.svg")),
            ];
            ControllerDescription::parse(&v.to_string(), &svgs)
        });
    DESCRIPTION.as_ref().map_err(Clone::clone)
}

#[test]
fn a_second_model_writes_and_reads_its_own_codes() {
    let model = Ultimate::new(variant);
    let d = variant().unwrap();
    let slot = Slot::new(1).unwrap();

    let profile = model.default_profile(XINPUT);
    let target = |p: &controller_core::model::CanonicalProfile, id: &str| {
        p.button_mappings.iter().find(|m| m.source == id).unwrap().target.clone()
    };
    assert_eq!(target(&profile, "p1"), "p1", "this model defaults P1 to its own output");
    assert_eq!(target(&profile, "p2"), "disabled");
    assert_eq!(target(&profile, "star"), "turbo");

    let def = MacroDefinition {
        name: String::new(),
        mode: XINPUT,
        trigger_buttons: vec!["right face".to_owned()],
        unknown_trigger_bits: 0,
        repeat_count: 1,
        interval_ms: 0,
        steps: vec![MacroStep {
            duration_ms: 100,
            pressed_buttons: vec!["right face".to_owned()],
            ..MacroStep::default()
        }],
        macro_slot: Some(0),
    };
    let blob = model.compile_profile(&profile, slot, &[], std::slice::from_ref(&def)).unwrap();

    assert_eq!(read_u16_le(&blob, GAMEPAD_MODE_OFFSET).unwrap(), 7);
    assert_eq!(image_mode(d, &blob).unwrap(), XINPUT);
    assert_eq!(read_u32_le(&blob, MAP_OFFSET + 4).unwrap(), 0x20_0000, "right face, index 0");
    assert_eq!(read_u32_le(&blob, MAP_OFFSET + 4 + 4 * 18).unwrap(), 0x200_0000, "p1, index 18");
    let record = MACROS_OFFSET + MACRO_RECORDS_AT;
    assert_eq!(read_u32_le(&blob, record + RECORD_TRIGGER_AT).unwrap(), 0x1000);
    assert_eq!(read_u16_le(&blob, record + RECORD_CODES_AT).unwrap(), 0x1000);

    let raw = RawProfilePayload {
        payload: blob.clone(),
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: XINPUT,
    };
    let back = model.map_profile(&raw).unwrap().canonical;
    assert_eq!(back.button_mappings, profile.button_mappings);
    assert_eq!(decode_macros(d, &blob, slot).unwrap(), [def]);
    assert!(model.slot_select_value(XINPUT).is_ok_and(|b| b == 7));

    // The same blob through the registered model: its XInput image is selected by 3, so
    // it is another mode's image there.
    assert!(image_mode(crate::d(), &blob).is_err());
    assert_eq!(crate::ultimate().slot_select_value(XINPUT).unwrap(), 3);
    assert!(model.slot_select_value(SWITCH).is_ok_and(|b| b == 0));
}
