//! Golden + round-trip tests for the Pro 3 macro encoder.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{MacroDefinition, MacroSlot, MacroStep, Mode};

const SECTION4_GOLDENMAC_DESCRIPTOR_OFFSET: usize = 0x0694; // slot1 macro0 in macro-meta.blob

#[test]
fn macro_steps_encode_is_byte_exact_inverse_of_decoder() {
    // macro-sample.steps.bin is real C++-encoder output (32B = 3 steps padded).
    let golden = std::fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();
    let steps = Pro3.decode_macro_steps(&golden, 3, Mode::XInput).unwrap();
    let reencoded = Pro3.encode_macro_steps(&steps, Mode::XInput).unwrap();
    assert_eq!(reencoded, golden);
}

#[test]
fn macro_steps_encode_matches_known_three_step_sample() {
    // The exact steps that produced macro-sample.steps.bin (see Plan-1 Task 8 §1).
    let steps = vec![
        MacroStep {
            duration_ms: 50,
            pressed_buttons: vec!["bottom face".into()],
            ..MacroStep::default()
        },
        MacroStep {
            duration_ms: 100,
            left_stick_x: 200,
            left_stick_y: 30,
            trigger_left: 255,
            trigger_right: 128,
            ..MacroStep::default()
        },
        MacroStep {
            duration_ms: 0,
            pressed_buttons: vec!["top face".into(), "r1".into()],
            right_stick_x: 64,
            right_stick_y: 192,
            ..MacroStep::default()
        },
    ];
    let golden = std::fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();
    assert_eq!(Pro3.encode_macro_steps(&steps, Mode::XInput).unwrap(), golden);
}

#[test]
fn macro_steps_pad_to_32_byte_boundary() {
    let one = vec![MacroStep::default()];
    assert_eq!(Pro3.encode_macro_steps(&one, Mode::XInput).unwrap().len(), 32);
    let sixteen = vec![MacroStep::default(); 16];
    assert_eq!(Pro3.encode_macro_steps(&sixteen, Mode::XInput).unwrap().len(), 160);
    assert_eq!(Pro3.encode_macro_steps(&[], Mode::XInput).unwrap().len(), 0);
}

#[test]
fn macro_steps_switch_routes_triggers_into_keys_bits() {
    let steps = vec![MacroStep { trigger_left: 255, trigger_right: 0, ..MacroStep::default() }];
    let out = Pro3.encode_macro_steps(&steps, Mode::Switch).unwrap();
    let keys = u16::from_le_bytes([out[2], out[3]]);
    let trig = u16::from_le_bytes([out[4], out[5]]);
    assert_eq!(keys & 0x4000, 0x4000); // L2 -> bit 14
    assert_eq!(keys & 0x8000, 0x0000); // R2 clear
    assert_eq!(trig, 0x0000); // trigger_value unused in Switch
}

#[test]
fn macro_metadata_is_byte_exact_against_goldenmac_descriptor() {
    let blob = std::fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
    let golden =
        &blob[SECTION4_GOLDENMAC_DESCRIPTOR_OFFSET..SECTION4_GOLDENMAC_DESCRIPTOR_OFFSET + 52];
    let def = MacroDefinition {
        name: "GoldenMac".into(),
        mode: Mode::XInput,
        trigger: "l1".into(),
        repeat_count: 3,
        interval_ms: 100,
        steps: vec![MacroStep::default(); 3], // max_steps = 3
        macro_slot: Some(0),
    };
    let encoded = Pro3.encode_macro_metadata(&def, MacroSlot::new(0).unwrap()).unwrap();
    assert_eq!(encoded, golden);
}

#[test]
fn macro_metadata_round_trips_through_decoder() {
    let def = MacroDefinition {
        name: "GoldenMac".into(),
        mode: Mode::XInput,
        trigger: "l1".into(),
        repeat_count: 3,
        interval_ms: 100,
        steps: vec![MacroStep::default(); 3],
        macro_slot: Some(0),
    };
    // Embed the encoded descriptor into a zeroed Section-4 slot-1 block and decode it back.
    let descriptor = Pro3.encode_macro_metadata(&def, MacroSlot::new(0).unwrap()).unwrap();
    let mut blob = vec![0u8; 0x092C];
    blob[0x068C..0x0690].copy_from_slice(&[0x11, 0x09, 0x20, 0x20]); // slot-1 marker
    blob[0x0690..0x0694].copy_from_slice(&4u32.to_le_bytes()); // count
    blob[0x0694..0x0694 + 52].copy_from_slice(&descriptor);
    let metas =
        Pro3.decode_macro_metadata(&blob, controller_core::model::Slot::new(1).unwrap()).unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].name, "GoldenMac");
    assert_eq!(metas[0].trigger, "l1");
    assert_eq!(metas[0].repeat_count, 3);
    assert_eq!(metas[0].interval_ms, 100);
    assert_eq!(metas[0].macro_slot, Some(0));
}
