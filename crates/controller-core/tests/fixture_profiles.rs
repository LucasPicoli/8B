//! Remap profile fixtures: compile-golden + round-trip + spec byte-vectors.
//! The `regenerate` test (ignored) writes the committed `.blob` files from the
//! authored `.json` via the verified `compile_profile`; normal tests verify.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::device::ProtocolCodec;
use controller_core::devices::pro3::Pro3;
use controller_core::model::{CanonicalProfile, Mode, RawProfilePayload, Slot};

const DIR: &str = "../../fixtures/pro3/remap";

/// (file stem, 1-based slot, 0-based profile index, mode)
const PROFILES: &[(&str, u8, u8, Mode)] = &[
    ("xinput-slot1", 1, 0, Mode::XInput),
    ("xinput-slot2", 2, 1, Mode::XInput),
    ("xinput-slot3", 3, 2, Mode::XInput),
    ("switch-slot1", 1, 0, Mode::Switch),
    ("switch-slot2", 2, 1, Mode::Switch),
    ("switch-slot3", 3, 2, Mode::Switch),
];

fn load_profile(stem: &str) -> CanonicalProfile {
    serde_json::from_slice(&std::fs::read(format!("{DIR}/{stem}.profile.json")).unwrap()).unwrap()
}

#[test]
fn all_profiles_compile_match_committed_blob() {
    for &(stem, slot, _idx, _mode) in PROFILES {
        let p = load_profile(stem);
        let blob = Pro3.compile_profile(&p, Slot::new(slot).unwrap(), &[], &[]).unwrap();
        let committed = std::fs::read(format!("{DIR}/{stem}.profile.blob")).unwrap();
        assert_eq!(blob, committed, "{stem}: compile output drifted from committed blob");
        assert_eq!(blob.len(), 0x092C);
    }
}

#[test]
fn all_profiles_round_trip_through_decoder() {
    for &(stem, slot, idx, mode) in PROFILES {
        let original = load_profile(stem);
        let blob = Pro3.compile_profile(&original, Slot::new(slot).unwrap(), &[], &[]).unwrap();
        let raw = RawProfilePayload {
            payload: blob,
            source_slot: slot,
            source_profile_index: idx,
            mode_hint: mode,
        };
        let decoded = Pro3.map_profile(&raw).unwrap().canonical;
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&original).unwrap(),
            "{stem}: round-trip mismatch"
        );
    }
}

#[test]
fn slot_markers_land_in_the_right_flag_block() {
    // Active marker at 0x0000/0x0004/0x0008 for slot 1/2/3; the other two blocks zero.
    const MARKER: [u8; 4] = [0x11, 0x09, 0x20, 0x20];
    for &(stem, slot, _idx, _mode) in PROFILES {
        let p = load_profile(stem);
        let b = Pro3.compile_profile(&p, Slot::new(slot).unwrap(), &[], &[]).unwrap();
        for s in 1u8..=3 {
            let off = (usize::from(s) - 1) * 4;
            let expect = if s == slot { MARKER } else { [0, 0, 0, 0] };
            assert_eq!(&b[off..off + 4], &expect, "{stem}: flag block slot {s}");
        }
    }
}

#[test]
fn spec_byte_vectors_for_tricky_remaps() {
    // Entry base 0x00E4 + idx*0x5C; entries are 22*4B in table order.
    let entry = |b: &[u8], idx: u8, i: usize| -> [u8; 4] {
        let base = 0x00E4 + usize::from(idx) * 0x5C + i * 4;
        [b[base], b[base + 1], b[base + 2], b[base + 3]]
    };

    // xinput-slot1: right face(0)->left face uses the XInput variant set = 10 00 00 00.
    let b = Pro3
        .compile_profile(&load_profile("xinput-slot1"), Slot::new(1).unwrap(), &[], &[])
        .unwrap();
    assert_eq!(entry(&b, 0, 0), [0x10, 0x00, 0x00, 0x00]);

    // xinput-slot2: select/back(10)->disabled = NULL.
    let b = Pro3
        .compile_profile(&load_profile("xinput-slot2"), Slot::new(2).unwrap(), &[], &[])
        .unwrap();
    assert_eq!(entry(&b, 1, 10), [0x00, 0x00, 0x00, 0x00]);

    // switch-slot1: select/back(10)->screenshot and turbo(12) default both = 00 00 40 00.
    let b = Pro3
        .compile_profile(&load_profile("switch-slot1"), Slot::new(1).unwrap(), &[], &[])
        .unwrap();
    assert_eq!(entry(&b, 0, 10), [0x00, 0x00, 0x40, 0x00]);
    assert_eq!(entry(&b, 0, 12), [0x00, 0x00, 0x40, 0x00]);

    // switch-slot2: turbo(12)->l1 explicit overrides the default (NOT 00 00 40 00).
    let b = Pro3
        .compile_profile(&load_profile("switch-slot2"), Slot::new(2).unwrap(), &[], &[])
        .unwrap();
    assert_ne!(entry(&b, 1, 12), [0x00, 0x00, 0x40, 0x00]);
}

#[test]
fn home_guide_remap_attempt_is_forced_to_identity() {
    // Clone a valid profile and try to remap home/guide(13) -> top face.
    let mut p = load_profile("xinput-slot1");
    for m in &mut p.button_mappings {
        if m.source == "home/guide" {
            m.target = "top face".to_owned();
        }
    }
    let b = Pro3.compile_profile(&p, Slot::new(1).unwrap(), &[], &[]).unwrap();

    // The home/guide entry must equal the home/guide identity encoding, NOT top face's.
    let base = 0x00E4 + 13 * 4;
    let entry = [b[base], b[base + 1], b[base + 2], b[base + 3]];
    let identity = Pro3
        .compile_profile(&load_profile("xinput-slot1"), Slot::new(1).unwrap(), &[], &[])
        .unwrap();
    let identity_entry =
        [identity[base], identity[base + 1], identity[base + 2], identity[base + 3]];
    assert_eq!(entry, identity_entry, "home/guide remap must be forced to identity");

    // And it decodes back to home/guide.
    let raw = RawProfilePayload {
        payload: b,
        source_slot: 1,
        source_profile_index: 0,
        mode_hint: Mode::XInput,
    };
    let decoded = Pro3.map_profile(&raw).unwrap().canonical;
    let hg = decoded.button_mappings.iter().find(|m| m.source == "home/guide").unwrap();
    assert_eq!(hg.target, "home/guide");
}

/// Regenerate committed blobs from authored JSON. Ignored in normal runs.
/// Run: `cargo test -p controller-core --test fixture_profiles regenerate -- --ignored`
#[test]
#[ignore = "writes committed fixture blobs; run manually after authoring/encoder changes"]
fn regenerate() {
    for &(stem, slot, _idx, _mode) in PROFILES {
        let p = load_profile(stem);
        let blob = Pro3.compile_profile(&p, Slot::new(slot).unwrap(), &[], &[]).unwrap();
        std::fs::write(format!("{DIR}/{stem}.profile.blob"), &blob).unwrap();
    }
}
