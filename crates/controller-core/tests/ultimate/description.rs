//! The Ultimate's description pinned to its codec, its schema and the captured firmware
//! reply.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::description::{firmware_allows, MacroSticks};
use controller_core::device::{ControllerSpec, ProtocolCodec, UsbId};
use controller_core::devices::ultimate::profile::map_profile;
use controller_core::devices::ultimate::tables::{
    BLOB_SIZE, EXTRA_OUTPUTS, MACRO_SLOTS, MACRO_STEPS, MACRO_STEP_BUTTONS, MACRO_TRIGGERS,
    MAP_BUTTONS, SLOT_COUNT, TRIGGER_ONLY,
};
use controller_core::devices::ultimate::{Ultimate, MODES, SWITCH, XINPUT};
use controller_core::devices::{find_model_by_port, models};
use controller_core::model::{Mode, RawProfilePayload};
use controller_core::service::validation::validate_profile;
use serde_json::Value;

/// Every image in `fixtures/ultimate/` that holds a profile.
const IMAGES: [&str; 9] =
    ["xinput-read", "saves/02", "saves/09", "switch-saved", "swap-on-p2", "r1", "r2", "r2b", "r3"];

fn image(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../fixtures/ultimate/{name}.bin")).unwrap()
}

fn sorted<'a>(v: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut v: Vec<&str> = v.into_iter().collect();
    v.sort_unstable();
    v
}

#[test]
fn the_description_matches_its_schema() {
    let schema: Value = serde_json::from_str(include_str!(
        "../../../../schemas/controller-description-v1.schema.json"
    ))
    .unwrap();
    let file: Value =
        serde_json::from_str(include_str!("../../controllers/ultimate-bluetooth/description.json"))
            .unwrap();
    let validator = jsonschema::draft202012::new(&schema).unwrap();
    let errors: Vec<String> = validator.iter_errors(&file).map(|e| e.to_string()).collect();
    assert_eq!(errors, Vec::<String>::new());
}

#[test]
fn its_config_port_names_it_and_its_bytes_match_the_codec() {
    let d = Ultimate.description().unwrap();
    assert_eq!(d.model_ids, Vec::<u16>::new());
    let port = d.config_ports[0];
    assert_eq!(d.config_ports.len(), 1);
    assert_eq!(port.usb, UsbId { vendor: 0x2DC8, product: 0x6007 });
    assert_eq!((port.mode, port.interface), (XINPUT, 0));
    let named = find_model_by_port(models(), port.usb).unwrap();
    assert_eq!(named.description().unwrap().short_name, "Ultimate");
    assert_eq!(models()[0].description().unwrap().short_name, "Pro 3", "the Pro 3 stays first");

    let lock = d.lock.as_ref().unwrap();
    assert_eq!(lock.locked.usb, UsbId { vendor: 0x2DC8, product: 0x3106 });
    assert_eq!(lock.locked.product, "8BitDo Ultimate Controller");
    assert_eq!((lock.unlock.packets.len(), lock.relock.packets.len()), (15, 4));

    assert_eq!(Ultimate.blob_size(), BLOB_SIZE);
    assert_eq!(Ultimate.apply_argument(), 0x0015);
    assert_eq!(Ultimate.slot_select_value(XINPUT).unwrap(), 3);
    assert_eq!(Ultimate.slot_select_value(SWITCH).unwrap(), 0);
    assert!(Ultimate.slot_select_value(Mode::from_static("dinput")).is_err());
    assert!(Ultimate.macro_steps_in_blob());
    assert!(Ultimate.read_back_writes());
    let listed: Vec<Mode> = d.modes.iter().map(|m| m.id).collect();
    assert_eq!(listed, MODES);
    assert_eq!((d.slot_count, usize::from(d.macro_slot_count)), (SLOT_COUNT, MACRO_SLOTS));
}

#[test]
fn the_firmware_comes_from_the_captured_reply() {
    let request = Ultimate.firmware_request().unwrap();
    assert_eq!(request.wire[..5], [0x81, 0x05, 0x00, 0x21, 0x01]);
    assert!(request.wire[5..].iter().all(|&b| b == 0));
    let hex = include_str!("../../../../fixtures/ultimate/firmware-reply.txt").trim();
    let reply: Vec<u8> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect();
    assert!(reply.starts_with(&request.reply_head));
    assert_eq!(Ultimate.firmware_version(&reply).unwrap(), "1.09");
}

#[test]
fn buttons_follow_the_map_and_each_has_a_hotspot() {
    let d = Ultimate.description().unwrap();
    let ids: Vec<&str> = d.buttons.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, MAP_BUTTONS);
    let placed: Vec<&str> =
        d.views.iter().flat_map(|v| &v.hotspots).map(|h| h.button.as_str()).collect();
    assert_eq!(sorted(placed), sorted(MAP_BUTTONS));
    assert!(d.buttons.iter().all(|b| b.can_be_remapped));
    let never_outputs = d.buttons.iter().filter(|b| !b.can_be_output).map(|b| b.id.as_str());
    assert_eq!(sorted(never_outputs), sorted(TRIGGER_ONLY.iter().map(|&(n, _)| n)));
    let spoken = |id: &str, mode| d.button(id).unwrap().spoken_label(mode);
    assert_eq!((spoken("select/back", SWITCH), spoken("start/menu", SWITCH)), ("Minus", "Plus"));
    assert_eq!(d.button("right face").unwrap().labels[&XINPUT], "B", "printed A sends Xbox B");
}

#[test]
fn extra_outputs_match_the_codec_and_their_buttons() {
    let d = Ultimate.description().unwrap();
    let ids = |mode| -> Vec<&str> {
        d.mode(mode).unwrap().extra_outputs.iter().map(|o| o.id.as_str()).collect()
    };
    assert_eq!(ids(XINPUT), ["turbo", "auto turbo", "swap"]);
    assert_eq!(ids(SWITCH), ["turbo", "auto turbo", "swap", "screenshot"]);
    assert_eq!(sorted(ids(SWITCH)), sorted(EXTRA_OUTPUTS.iter().map(|&(n, _)| n)));

    let ok = |mode, source, target| Ultimate.validate_remap(mode, source, target).is_ok();
    for mode in MODES {
        for source in ["star", "p1", "p2"] {
            assert!(ok(mode, source, "turbo") && ok(mode, source, "auto turbo"), "{source}");
            assert!(ok(mode, source, "swap"), "{source}");
        }
        assert!(!ok(mode, "right face", "turbo"));
        assert!(!ok(mode, "l1", "swap"));
        assert!(ok(mode, "star", "right face"), "star takes a button output");
        assert!(!ok(mode, "right face", "star"), "star is never an output");
    }
    assert!(ok(SWITCH, "star", "screenshot"));
    assert!(!ok(SWITCH, "p1", "screenshot"));
    assert!(!ok(XINPUT, "star", "screenshot"));
}

#[test]
fn macro_flags_and_limits_match_the_codec() {
    let d = Ultimate.description().unwrap();
    let flagged = |pick: fn(&controller_core::description::Button) -> bool| {
        sorted(d.buttons.iter().filter(|b| pick(b)).map(|b| b.id.as_str()))
    };
    assert_eq!(flagged(|b| b.can_start_macro), sorted(MACRO_TRIGGERS));
    assert_eq!(flagged(|b| b.can_be_in_macro_step), sorted(MACRO_STEP_BUTTONS));
    assert!(d.buttons.iter().all(|b| b.step_trigger_value.is_none()), "L2 and R2 are buttons");
    let l = d.limits;
    assert_eq!((l.macro_steps.min, l.macro_steps.max), (1, i32::try_from(MACRO_STEPS).unwrap()));
    assert_eq!((l.macro_step_ms.min, l.macro_step_ms.max), (10, 60990));
    assert_eq!(l.triggers_per_macro, Some(1));
    assert!(l.macro_name_length.is_none() && l.macro_repeat_count.is_none());
    assert!(l.macro_interval_ms.is_none() && l.glide_step_ms.is_none());
    assert_eq!(d.macro_sticks, MacroSticks::Directions8);
    assert!(!d.profile_rename);
}

#[test]
fn every_decoded_profile_and_default_passes_validation() {
    let check = |json: &Value, what: &str| {
        let result = validate_profile(&Ultimate, json).unwrap();
        assert!(result.valid, "{what}: {:?}", result.errors);
    };
    for mode in MODES {
        check(&serde_json::to_value(Ultimate.default_profile(mode)).unwrap(), mode.as_str());
    }
    for name in IMAGES {
        let blob = image(name);
        let mode = controller_core::devices::ultimate::image_mode(&blob).unwrap();
        for n in 1..=SLOT_COUNT {
            if !Ultimate.slot_active(&blob, controller_core::model::Slot::new(n).unwrap()).unwrap()
            {
                continue;
            }
            let raw = RawProfilePayload {
                payload: blob.clone(),
                source_slot: n,
                source_profile_index: n - 1,
                mode_hint: mode,
            };
            let profile = map_profile(&raw).unwrap().canonical;
            check(&serde_json::to_value(profile).unwrap(), &format!("{name} slot {n}"));
        }
    }
}

#[test]
fn settings_show_per_mode_and_four_way_needs_firmware_1_03() {
    let d = Ultimate.description().unwrap();
    let has = |mode, field: &str| d.flag(mode, field).is_some() || d.number(mode, field).is_some();
    assert!(has(XINPUT, "/sticks/no_dead_zone") && !has(SWITCH, "/sticks/no_dead_zone"));
    let swap = "/sticks/swap_right_stick_with_triggers";
    assert!(has(SWITCH, swap) && !has(XINPUT, swap));
    assert!(has(XINPUT, "/triggers/left_max_pct") && !has(SWITCH, "/triggers/left_max_pct"));
    for mode in MODES {
        let gate = d.flag(mode, "/sticks/dpad_four_way").unwrap().min_firmware.unwrap();
        assert!(firmware_allows(Some(gate), "1.09"));
        assert!(!firmware_allows(Some(gate), "1.02"));
        assert!(!firmware_allows(Some(gate), ""));
    }
}
