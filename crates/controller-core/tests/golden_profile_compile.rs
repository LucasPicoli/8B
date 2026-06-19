//! Round-trip (compile -> decode) + byte-vector tests for the Pro 3 compiler.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{CanonicalProfile, Mode, RawProfilePayload, Slot};

fn load(path: &str) -> CanonicalProfile {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn roundtrip(json: &str, slot: u8, index: u8, mode: Mode) {
    let original = load(json);
    let blob = Pro3.compile_profile(&original, Slot::new(slot).unwrap(), &[], &[]).unwrap();
    assert_eq!(blob.len(), 0x092C);
    let raw = RawProfilePayload {
        payload: blob,
        source_slot: slot,
        source_profile_index: index,
        mode_hint: mode,
    };
    let decoded = Pro3.map_profile(&raw).unwrap().canonical;
    assert_eq!(serde_json::to_value(&decoded).unwrap(), serde_json::to_value(&original).unwrap());
}

#[test]
fn xinput_slot1_round_trips() {
    roundtrip("../../fixtures/pro3/xinput-slot1.profile.json", 1, 0, Mode::XInput);
}
#[test]
fn xinput_slot2_round_trips() {
    roundtrip("../../fixtures/pro3/xinput-slot2.profile.json", 2, 1, Mode::XInput);
}
#[test]
fn switch_slot1_round_trips() {
    roundtrip("../../fixtures/pro3/switch-slot1.profile.json", 1, 0, Mode::Switch);
}

#[test]
fn xinput_byte_vectors_match_spec() {
    let p = load("../../fixtures/pro3/xinput-slot1.profile.json");
    let b = Pro3.compile_profile(&p, Slot::new(1).unwrap(), &[], &[]).unwrap();
    // flags / mode
    assert_eq!(&b[0x0000..0x0004], &[0x11, 0x09, 0x20, 0x20]);
    assert_eq!(&b[0x0004..0x000C], &[0u8; 8]);
    assert_eq!(u16::from_le_bytes([b[0x0010], b[0x0011]]), 0x0003);
    // CRC recomputes
    let mut z = b.clone();
    z[0x000C..0x0010].fill(0);
    assert_eq!(
        u16::from_le_bytes([b[0x000C], b[0x000D]]),
        controller_core::protocol::crc16::crc16_modbus(&z)
    );
    // sticks 20/80/15/85 -> 26/102/19/109 ; triggers 10/90/5/95 -> 26/230/13/242
    assert_eq!(&b[0x009C..0x00A0], &[26, 102, 19, 109]);
    assert_eq!(&b[0x00B4..0x00B8], &[26, 230, 13, 242]);
    // button entries (variant write set): right face(0)->left face = 10 00 00 00 ; l1(4)->r1 = 00 08 00 00 ; rp(18)->disabled = 0
    assert_eq!(&b[0x00E4..0x00E8], &[0x10, 0x00, 0x00, 0x00]);
    assert_eq!(&b[0x00E4 + 16..0x00E4 + 20], &[0x00, 0x08, 0x00, 0x00]);
    assert_eq!(&b[0x00E4 + 72..0x00E4 + 76], &[0x00, 0x00, 0x00, 0x00]);
    // vibration intensity floats: 4/5=0.8, 2/5=0.4
    assert_eq!(&b[0x0078..0x007C], &0.8f32.to_le_bytes());
    assert_eq!(&b[0x007C..0x0080], &0.4f32.to_le_bytes());
}

#[test]
fn switch_byte_vectors_match_spec() {
    let p = load("../../fixtures/pro3/switch-slot1.profile.json");
    let b = Pro3.compile_profile(&p, Slot::new(1).unwrap(), &[], &[]).unwrap();
    assert_eq!(u16::from_le_bytes([b[0x0010], b[0x0011]]), 0x0000); // switch mode
                                                                    // turbo(12) -> screenshot default = 00 00 40 00
    assert_eq!(&b[0x00E4 + 48..0x00E4 + 52], &[0x00, 0x00, 0x40, 0x00]);
    // switch trigger threshold form [thr,0xFF,thr,0xFF]; 25% -> 64, 40% -> 102
    assert_eq!(b[0x00B4 + 1], 0xFF);
    assert_eq!(b[0x00B4 + 3], 0xFF);
    // swap_triggers true -> flags0 bit7
    assert_eq!(b[0x00CC] & 0x80, 0x80);
}

#[test]
fn compile_profile_embeds_macro_into_section4() {
    use controller_core::model::{MacroDefinition, MacroSlot, MacroStep};
    let profile = load("../../fixtures/pro3/xinput-slot1.profile.json");
    let def = MacroDefinition {
        name: "GoldenMac".into(),
        mode: Mode::XInput,
        trigger: "l1".into(),
        repeat_count: 3,
        interval_ms: 100,
        steps: vec![MacroStep::default(); 3],
        macro_slot: Some(0),
    };
    let blob = Pro3
        .compile_profile(&profile, Slot::new(1).unwrap(), &[], std::slice::from_ref(&def))
        .unwrap();
    // The descriptor lands at Section-4 slot-1 macro-0 = 0x068C + 8 + 0*52 = 0x0694,
    // byte-exact equal to the standalone encoder, AND decodes back through the verified decoder.
    assert_eq!(
        &blob[0x0694..0x0694 + 52],
        Pro3.encode_macro_metadata(&def, MacroSlot::new(0).unwrap()).unwrap().as_slice()
    );
    let metas = Pro3.decode_macro_metadata(&blob, Slot::new(1).unwrap()).unwrap();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].name, "GoldenMac");
    assert_eq!(metas[0].trigger, "l1");
    assert_eq!(metas[0].repeat_count, 3);
    assert_eq!(metas[0].interval_ms, 100);
    assert_eq!(metas[0].macro_slot, Some(0));
}

#[test]
fn switch_explicit_turbo_remap_overrides_screenshot_default() {
    let mut profile = load("../../fixtures/pro3/switch-slot1.profile.json");
    for m in &mut profile.button_mappings {
        if m.source == "turbo" {
            m.target = "l1".into();
        }
    }
    let b = Pro3.compile_profile(&profile, Slot::new(1).unwrap(), &[], &[]).unwrap();
    // turbo is source index 12; entry at 0x00E4 + 12*4 = 0x0114. Switch l1 encoding = 00 04 00 00,
    // NOT the screenshot default 00 00 40 00.
    assert_eq!(&b[0x00E4 + 48..0x00E4 + 52], &[0x00, 0x04, 0x00, 0x00]);
}

#[test]
fn read_modify_write_preserves_other_slot() {
    // Compile a fresh slot-1 onto the real two-slot xinput.blob; slot-2 bytes must be untouched.
    let base = std::fs::read("../../fixtures/pro3/xinput.blob").unwrap();
    let p = load("../../fixtures/pro3/xinput-slot2.profile.json"); // slot-2 fixture compiled into slot-1 (exercises that any profile compiles into any slot)
    let out = Pro3.compile_profile(&p, Slot::new(1).unwrap(), &base, &[]).unwrap();
    // Slot-2 name field (0x0034) and slot-2 button map (0x0140 region) are preserved verbatim.
    assert_eq!(&out[0x0034..0x0054], &base[0x0034..0x0054]);
    assert_eq!(&out[0x0140..0x0140 + 88], &base[0x0140..0x0140 + 88]);
}
