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

/// A full `DInput` write by the official app, all three slots active, captured over USB.
/// The payload sits at wire byte 18, as in every other mode. Its face buttons hold the
/// `XInput` encodings, which a `DInput` slot presses as the other face of each pair.
const DINPUT_OFFICIAL: &str = "../../fixtures/pro3/dinput-official.blob";

#[test]
fn dinput_official_write_decodes_with_swapped_faces_and_recompiles_unchanged() {
    let official = std::fs::read(DINPUT_OFFICIAL).unwrap();
    for slot in 1..=3u8 {
        let raw = RawProfilePayload {
            payload: official.clone(),
            source_slot: slot,
            source_profile_index: slot - 1,
            mode_hint: Mode::DInput,
        };
        let profile = Pro3.map_profile(&raw).unwrap().canonical;
        assert_eq!(profile.mode, Mode::DInput);
        let default = Pro3.default_profile(Mode::DInput);
        assert_eq!(profile.sticks, default.sticks, "slot {slot}");
        assert_eq!(profile.triggers, default.triggers, "slot {slot}");
        // The faces swap in pairs, every other button maps to itself, and the four
        // paddles are unassigned.
        for m in &profile.button_mappings {
            let want = match m.source.as_str() {
                "right face" => "bottom face",
                "bottom face" => "right face",
                "top face" => "left face",
                "left face" => "top face",
                "rp" | "lp" | "l4" | "r4" => "disabled",
                other => other,
            };
            assert_eq!(m.target, want, "slot {slot}");
        }
        let mut rebuilt = Pro3
            .compile_profile_keep_macros(&profile, Slot::new(slot).unwrap(), &official)
            .unwrap();
        // The struct CRC at 0x0C uses an unknown formula in the official app, and the
        // firmware does not check it. Every other byte must match.
        let mut want = official.clone();
        rebuilt[0x0C..0x10].fill(0);
        want[0x0C..0x10].fill(0);
        assert_eq!(rebuilt, want, "slot {slot}");
    }
}

/// Stick and trigger bytes that are not an exact percent survive a decode and a
/// recompile onto the same blob byte for byte, in every mode.
#[test]
fn untouched_stick_and_trigger_bytes_do_not_drift() {
    // Each byte re-encodes 1 off through its percent (trigger 0xB2 -> 70% -> 0xB3).
    const STICKS: [u8; 4] = [0x02, 0x7E, 0x02, 0x7E];
    const ANALOG_TRIGGERS: [u8; 4] = [0xB2, 0xEF, 0x02, 0xFD];
    const SWITCH_TRIGGERS: [u8; 4] = [0xB2, 0xFF, 0x02, 0xFF];
    let cases = [
        ("xinput.blob", Mode::XInput, ANALOG_TRIGGERS),
        ("xinput.blob", Mode::DInput, ANALOG_TRIGGERS),
        ("switch.blob", Mode::Switch, SWITCH_TRIGGERS),
    ];
    for (file, mode, triggers) in cases {
        let mut base = std::fs::read(format!("../../fixtures/pro3/{file}")).unwrap();
        base[0x009C..0x00A0].copy_from_slice(&STICKS);
        base[0x00B4..0x00B8].copy_from_slice(&triggers);
        let raw = RawProfilePayload {
            payload: base.clone(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: mode,
        };
        let profile = Pro3.map_profile(&raw).unwrap().canonical;
        let out = Pro3.compile_profile(&profile, Slot::new(1).unwrap(), &base, &[]).unwrap();
        assert_eq!(&out[0x009C..0x00A0], &STICKS, "{mode:?} sticks");
        assert_eq!(&out[0x00B4..0x00B8], &triggers, "{mode:?} triggers");
    }
}

/// A `DInput` bank read from a real pad. D-pad left (entry 16) of slots 1 and 2 holds
/// `11 09 20 20`, which matches no table value; the pad fires it as a 6-output combo.
const DINPUT_SLOT_MARKER: &str = "../../fixtures/pro3/dinput-slot-marker.blob";

fn decode_slot(blob: &[u8], slot: u8, mode: Mode) -> CanonicalProfile {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: slot,
        source_profile_index: slot - 1,
        mode_hint: mode,
    };
    Pro3.map_profile(&raw).unwrap().canonical
}

fn target_of<'a>(profile: &'a CanonicalProfile, source: &str) -> &'a str {
    &profile.button_mappings.iter().find(|m| m.source == source).unwrap().target
}

#[test]
fn unknown_button_entry_decodes_unrecognised_and_recompiles_unchanged() {
    let base = std::fs::read(DINPUT_SLOT_MARKER).unwrap();
    for slot in 1..=3u8 {
        let profile = decode_slot(&base, slot, Mode::DInput);
        let want = if slot < 3 { "unrecognised" } else { "d-pad left" };
        assert_eq!(target_of(&profile, "d-pad left"), want, "slot {slot}");
        let mut out =
            Pro3.compile_profile_keep_macros(&profile, Slot::new(slot).unwrap(), &base).unwrap();
        let mut want = base.clone();
        out[0x0C..0x10].fill(0);
        want[0x0C..0x10].fill(0);
        assert_eq!(out, want, "slot {slot}");
    }
}

#[test]
fn made_up_button_mask_decodes_unrecognised_and_survives_an_edit() {
    let mut base = std::fs::read("../../fixtures/pro3/xinput.blob").unwrap();
    // Entry 0 (right face) of slot 1: a mask with no table value.
    base[0x00E4..0x00E8].copy_from_slice(&[0x00, 0x00, 0x00, 0x80]);
    let mut profile = decode_slot(&base, 1, Mode::XInput);
    assert_eq!(target_of(&profile, "right face"), "unrecognised");
    // Remap an unrelated button; the unknown entry keeps its bytes.
    for m in &mut profile.button_mappings {
        if m.source == "l1" {
            "r1".clone_into(&mut m.target);
        }
    }
    let out = Pro3.compile_profile(&profile, Slot::new(1).unwrap(), &base, &[]).unwrap();
    assert_eq!(&out[0x00E4..0x00E8], &[0x00, 0x00, 0x00, 0x80]);
    assert_eq!(&out[0x00E4 + 16..0x00E4 + 20], &[0x00, 0x08, 0x00, 0x00]);
}

#[test]
fn unrecognised_target_without_bytes_to_keep_is_refused() {
    let base = std::fs::read(DINPUT_SLOT_MARKER).unwrap();
    let mut profile = decode_slot(&base, 1, Mode::DInput);
    // Slot 3 d-pad left is a plain d-pad left, so there is nothing to keep.
    let slot3 = Slot::new(3).unwrap();
    assert!(Pro3.compile_profile_keep_macros(&profile, slot3, &base).is_err());
    // No base blob at all.
    assert!(Pro3.compile_profile(&profile, Slot::new(1).unwrap(), &[], &[]).is_err());
    // Home is forced to identity, so it never has an unrecognised entry.
    for m in &mut profile.button_mappings {
        if m.source == "home/guide" {
            "unrecognised".clone_into(&mut m.target);
        }
    }
    assert_eq!(target_of(&profile, "d-pad left"), "unrecognised");
    assert!(Pro3.compile_profile_keep_macros(&profile, Slot::new(1).unwrap(), &base).is_err());
}

/// A `DInput` bank read from a real pad. Macro slot 3 of every slot holds a macro on `r4`.
const DINPUT_MACRO: &str = "../../fixtures/pro3/dinput-macro.blob";

#[test]
fn dinput_macros_decode_as_refs_and_survive_or_leave_with_an_edit() {
    let base = std::fs::read(DINPUT_MACRO).unwrap();
    for slot in 1..=3u8 {
        let profile = decode_slot(&base, slot, Mode::DInput);
        let want = format!("dinput-slot{slot}-macro3-mx_d{slot}4.json");
        let refs: Vec<_> = profile.macro_refs.iter().map(|m| (&*m.path, &*m.trigger)).collect();
        assert_eq!(refs, [(want.as_str(), "r4")], "slot {slot}");

        // An edit keeps the descriptors byte for byte, and dropping the trigger clears them.
        let at = Slot::new(slot).unwrap();
        let kept = Pro3.compile_profile_keep_macros(&profile, at, &base).unwrap();
        let region = 0x068C + usize::from(slot - 1) * 216..0x068C + usize::from(slot) * 216;
        assert_eq!(kept[region.clone()], base[region.clone()], "slot {slot} keeps");
        let dropped = Pro3.drop_macros(&base, at, &["r4".to_owned()]).unwrap();
        assert!(decode_slot(&dropped, slot, Mode::DInput).macro_refs.is_empty(), "slot {slot}");
    }
}

/// The official `XInput` capture `xinput_swap_left_and_right_sticks_on` writes
/// `00 00 11 09 20 20 10 00` at canonical `0x00C8`, so slot 1 flags `10 00` land
/// at device-native `0x00CC` after the -2 shift. Swap sticks is the only change.
#[test]
fn xinput_swap_sticks_writes_the_bytes_of_the_official_capture() {
    let mut p = load("../../fixtures/pro3/xinput-slot1.profile.json");
    p.sticks.swap_sticks = true;
    p.sticks.invert_left_x = false;
    p.sticks.invert_left_y = false;
    p.sticks.invert_right_x = false;
    p.sticks.invert_right_y = false;
    p.sticks.swap_dpad_with_left_stick = false;
    if let controller_core::model::Triggers::Analog(a) = &mut p.triggers {
        a.swap_triggers = false;
    }
    let b = Pro3.compile_profile(&p, Slot::new(1).unwrap(), &[], &[]).unwrap();
    assert_eq!(&b[0x00C8..0x00D0], &[0x11, 0x09, 0x20, 0x20, 0x10, 0x00, 0x00, 0x00]);
}

/// Every slot of a bank the official app wrote with every setting at default decodes
/// to the default profile, and its button map recompiles unchanged.
#[test]
fn vendor_default_banks_decode_to_the_default_profile() {
    /// Bytes per slot of the button map: 22 entries of 4 bytes.
    const BUTTON_MAP_BYTES: usize = 22 * 4;
    for (file, mode) in [
        ("vendor-default-xinput.blob", Mode::XInput),
        ("vendor-default-switch.blob", Mode::Switch),
        ("vendor-default-dinput.blob", Mode::DInput),
    ] {
        let base = std::fs::read(format!("../../fixtures/pro3/{file}")).unwrap();
        let default = Pro3.default_profile(mode);
        for slot in 1..=3u8 {
            let profile = decode_slot(&base, slot, mode);
            assert_eq!(profile.button_mappings, default.button_mappings, "{mode} slot {slot}");
            assert_eq!(profile.sticks, default.sticks, "{mode} slot {slot}");
            assert_eq!(profile.triggers, default.triggers, "{mode} slot {slot}");
            assert_eq!(profile.vibration, default.vibration, "{mode} slot {slot}");
            let out = Pro3
                .compile_profile_keep_macros(&profile, Slot::new(slot).unwrap(), &base)
                .unwrap();
            let start = 0x00E4 + usize::from(slot - 1) * 0x5C;
            let map = start..start + BUTTON_MAP_BYTES;
            assert_eq!(&out[map.clone()], &base[map], "{mode} slot {slot}");
        }
    }
}

/// A pad test proved that in `DInput` a source holding a back-paddle code sends that
/// paddle's button. Each output writes the paddle's own code and reads back as itself.
#[test]
fn dinput_paddle_outputs_write_the_paddle_codes_and_read_back() {
    let official = std::fs::read(DINPUT_OFFICIAL).unwrap();
    let raw = |payload: Vec<u8>| RawProfilePayload {
        payload,
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: Mode::DInput,
    };
    let mut profile = Pro3.map_profile(&raw(official.clone())).unwrap().canonical;
    // (source, entry index, output, code)
    let cases = [
        ("l1", 4, "rp output", [0x00, 0x00, 0x00, 0x02]),
        ("r1", 5, "lp output", [0x00, 0x00, 0x00, 0x04]),
        ("l3", 8, "l4 output", [0x00, 0x00, 0x20, 0x00]),
        ("r3", 9, "r4 output", [0x00, 0x00, 0x00, 0x40]),
    ];
    for (source, _, output, _) in cases {
        let m = profile.button_mappings.iter_mut().find(|m| m.source == source).unwrap();
        output.clone_into(&mut m.target);
    }
    let blob = Pro3.compile_profile(&profile, Slot::new(1).unwrap(), &official, &[]).unwrap();
    for (source, index, _, code) in cases {
        assert_eq!(&blob[0x00E4 + index * 4..0x00E4 + index * 4 + 4], &code, "{source}");
    }
    let decoded = Pro3.map_profile(&raw(blob)).unwrap().canonical;
    assert_eq!(decoded.button_mappings, profile.button_mappings);
}

/// A pad test proved `00 00 00 00` is the real "disabled" for a `DInput` paddle: its
/// own code still sends its button. Disabling the four paddles of a vendor-default
/// slot writes the zeros the official app writes, and they read back as disabled.
#[test]
fn dinput_disabled_paddles_write_the_zeros_of_the_official_blob() {
    /// Button entries of the paddles `rp`, `lp`, `l4` and `r4`.
    const PADDLES: std::ops::Range<usize> = 18..22;
    let official = std::fs::read(DINPUT_OFFICIAL).unwrap();
    let base = std::fs::read("../../fixtures/pro3/vendor-default-dinput.blob").unwrap();
    for slot in 1..=3u8 {
        let mut profile = decode_slot(&base, slot, Mode::DInput);
        for m in &mut profile.button_mappings[PADDLES] {
            assert_eq!(m.target, format!("{} output", m.source), "slot {slot}");
            "disabled".clone_into(&mut m.target);
        }
        let blob =
            Pro3.compile_profile_keep_macros(&profile, Slot::new(slot).unwrap(), &base).unwrap();
        let start = 0x00E4 + usize::from(slot - 1) * 0x5C;
        let paddles = start + PADDLES.start * 4..start + PADDLES.end * 4;
        assert_eq!(&blob[paddles.clone()], &[0u8; 16], "slot {slot}");
        assert_eq!(&blob[paddles.clone()], &official[paddles], "slot {slot}");
        let back = decode_slot(&blob, slot, Mode::DInput);
        assert_eq!(back.button_mappings, profile.button_mappings, "slot {slot}");
    }
}
