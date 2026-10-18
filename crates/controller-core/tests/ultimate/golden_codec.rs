//! The Ultimate codec against images read from the pad and saved by the vendor app.
//! Every compare skips the blob CRC, which the pad does not check.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use controller_core::devices::ultimate::edit::{
    compile_profile, deactivate_profile, drop_macros, erased_image, set_macro,
};
use controller_core::devices::ultimate::macros::decode_macros;
use controller_core::devices::ultimate::profile::{default_profile, map_profile};
use controller_core::devices::ultimate::settings::Settings;
use controller_core::devices::ultimate::tables::{
    CRC_LEN, CRC_OFFSET, FLAG_LEN, MACROS_LEN, MACROS_OFFSET, MAP_LEN, MAP_OFFSET, NAME_LEN,
    NAME_OFFSET, RANGE_LEN, SLOT_FLAG_OFFSET, STICKS_OFFSET, TOGGLES_LEN, TOGGLES_OFFSET,
    TRIGGERS_OFFSET, VIBRATION_LEN, VIBRATION_OFFSET,
};
use controller_core::devices::ultimate::{image_mode, slot_in_use, SWITCH, XINPUT};
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroSlot, MacroStep, RawProfilePayload, Slot,
};

/// Every image in `fixtures/ultimate/`.
const IMAGES: [&str; 18] = [
    "xinput-read",
    "saves/01",
    "saves/02",
    "saves/03",
    "saves/04",
    "saves/05",
    "saves/06",
    "saves/07",
    "saves/08",
    "saves/09",
    "switch-erased",
    "switch-saved",
    "swap-on-p2",
    "r1",
    "r2",
    "r2b",
    "r3",
    "r3b",
];

fn image(name: &str) -> Vec<u8> {
    std::fs::read(format!("../../fixtures/ultimate/{name}.bin")).unwrap()
}

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

/// Slot `n` of `blob` as a profile, in the mode the image records.
fn profile(blob: &[u8], n: u8) -> CanonicalProfile {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: n,
        source_profile_index: n - 1,
        mode_hint: image_mode(blob).unwrap(),
    };
    map_profile(&raw).unwrap().canonical
}

fn macros(blob: &[u8], n: u8) -> Vec<MacroDefinition> {
    decode_macros(blob, slot(n)).unwrap()
}

/// The output of `source` in `p`.
fn target<'a>(p: &'a CanonicalProfile, source: &str) -> &'a str {
    &p.button_mappings.iter().find(|m| m.source == source).unwrap().target
}

/// `blob` with its CRC zeroed.
fn without_crc(blob: &[u8]) -> Vec<u8> {
    let mut b = blob.to_vec();
    b[CRC_OFFSET..CRC_OFFSET + CRC_LEN].fill(0);
    b
}

/// Every byte of slot `n` but its name: the slot flag and its 6 sections.
fn slot_bytes(blob: &[u8], n: u8) -> Vec<u8> {
    let s = usize::from(n - 1);
    let places = [
        (SLOT_FLAG_OFFSET, FLAG_LEN),
        (VIBRATION_OFFSET, VIBRATION_LEN),
        (STICKS_OFFSET, RANGE_LEN),
        (TRIGGERS_OFFSET, RANGE_LEN),
        (TOGGLES_OFFSET, TOGGLES_LEN),
        (MAP_OFFSET, MAP_LEN),
        (MACROS_OFFSET, MACROS_LEN),
    ];
    places
        .iter()
        .flat_map(|&(base, len)| blob[base + s * len..base + (s + 1) * len].to_vec())
        .collect()
}

fn name(blob: &[u8], n: u8) -> &[u8] {
    let at = NAME_OFFSET + usize::from(n - 1) * NAME_LEN;
    &blob[at..at + NAME_LEN]
}

/// Slot `n` of `blob` encoded again from its own decode.
fn reencode(blob: &[u8], n: u8) -> Vec<u8> {
    compile_profile(&profile(blob, n), slot(n), blob, &macros(blob, n)).unwrap()
}

fn step(ms: u16, buttons: &[&str]) -> MacroStep {
    MacroStep {
        duration_ms: ms,
        pressed_buttons: buttons.iter().map(|&b| b.to_owned()).collect(),
        ..MacroStep::default()
    }
}

#[test]
fn every_slot_in_use_decodes_and_encodes_back_byte_for_byte() {
    let mut slots = 0;
    for name in IMAGES {
        let blob = image(name);
        for n in 1..=3 {
            if !slot_in_use(&blob, slot(n)).unwrap() {
                continue;
            }
            slots += 1;
            assert_eq!(without_crc(&reencode(&blob, n)), without_crc(&blob), "{name} slot {n}");
        }
    }
    assert_eq!(slots, 39);
}

#[test]
fn the_first_read_holds_one_profile() {
    let blob = image("xinput-read");
    assert_eq!(image_mode(&blob).unwrap(), XINPUT);
    let in_use: Vec<bool> = (1..=3).map(|n| slot_in_use(&blob, slot(n)).unwrap()).collect();
    assert_eq!(in_use, [true, false, false]);
    let p = profile(&blob, 1);
    assert_eq!(target(&p, "right face"), "right face", "printed A sends Xbox B");
    assert_eq!(target(&p, "star"), "auto turbo");
    assert_eq!(p.name, "", "a blank name");
}

#[test]
fn each_save_decodes_to_the_edit_the_user_made() {
    let p = profile(&image("saves/01"), 2);
    assert_eq!(target(&p, "right face"), "bottom face", "index A holds 0x2000");
    assert_eq!(target(&p, "p2"), "turbo");

    let s = Settings::of(&profile(&image("saves/02"), 2)).unwrap();
    let t = &s.sticks;
    assert_eq!(
        (t.left_min_pct, t.left_max_pct, t.right_min_pct, t.right_max_pct),
        (49, 100, 0, 50)
    );
    assert!(t.invert_left_x && t.invert_left_y && t.invert_right_x && t.invert_right_y);
    assert!(t.swap_sticks && t.dpad_four_way && !t.swap_dpad_with_left_stick);

    let s = Settings::of(&profile(&image("saves/03"), 2)).unwrap();
    let t = &s.triggers;
    assert_eq!(
        (t.left_min_pct, t.left_max_pct, t.right_min_pct, t.right_max_pct),
        (46, 100, 0, 47)
    );
    assert!(t.swap_triggers);

    let s = Settings::of(&profile(&image("saves/04"), 2)).unwrap();
    assert_eq!((s.vibration.left_level, s.vibration.right_level), (0, 2));

    let m = macros(&image("saves/09"), 2);
    assert_eq!(m.len(), 2);
    assert_eq!(m[0].trigger_buttons, ["p1"]);
    assert_eq!(m[0].steps, [step(10, &["bottom face"])], "printed B");
    assert_eq!(m[1].trigger_buttons, ["p2"]);
    let p2 = [
        step(50, &["right face"]),
        step(1010, &[]),
        step(51, &["top face"]),
        step(2010, &["left face"]),
        step(3010, &[]),
        step(4010, &["d-pad down"]),
    ];
    assert_eq!(m[1].steps, p2);
    assert_eq!((m[1].repeat_count, m[1].interval_ms, m[1].macro_slot), (1, 0, Some(1)));
}

#[test]
fn macros_hold_stick_directions() {
    let m = macros(&image("r3"), 1);
    let triggers: Vec<&str> = m.iter().map(|d| d.trigger_buttons[0].as_str()).collect();
    assert_eq!(triggers, ["right face", "star", "home/guide", "p1"]);
    let counts: Vec<usize> = m.iter().map(|d| d.steps.len()).collect();
    assert_eq!(counts, [18, 5, 6, 8]);
    let sticks: Vec<[u8; 4]> = m[2]
        .steps
        .iter()
        .map(|s| [s.left_stick_x, s.left_stick_y, s.right_stick_x, s.right_stick_y])
        .collect();
    assert_eq!(
        sticks,
        [
            [127, 0, 127, 127],
            [255, 127, 127, 127],
            [127, 127, 127, 255],
            [127, 127, 0, 127],
            [255, 0, 0, 255],
            [127, 127, 127, 127],
        ],
        "left up, left right, right down, right left, both diagonals, centre"
    );
}

#[test]
fn hidden_toggle_bits_survive_an_edit() {
    let blob = image("r3");
    let mut p = profile(&blob, 2);
    assert!(p.set_setting("/sticks/invert_left_x", true.into()));
    let out = compile_profile(&p, slot(2), &blob, &macros(&blob, 2)).unwrap();
    let at = TOGGLES_OFFSET + TOGGLES_LEN + FLAG_LEN;
    assert_eq!(u32::from_le_bytes(out[at..at + 4].try_into().unwrap()), 0xCA01);
}

#[test]
fn swap_reads_on_p2() {
    assert_eq!(target(&profile(&image("swap-on-p2"), 3), "p2"), "swap");
}

#[test]
fn each_save_is_the_one_before_it_plus_its_edit() {
    let mut prev = image("xinput-read");
    for n in 1..=9 {
        let next = image(&format!("saves/{n:02}"));
        let out = compile_profile(&profile(&next, 2), slot(2), &prev, &macros(&next, 2)).unwrap();
        assert_eq!(profile(&out, 2), profile(&next, 2), "save {n}: the edited slot");
        // A profile holds ranges in percent, and the vendor app stores raw values that
        // no percent rounds to, such as 118 for 46%. Those land one raw unit away.
        let ranges = FLAG_LEN + VIBRATION_LEN..FLAG_LEN + VIBRATION_LEN + 2 * RANGE_LEN;
        for (i, (a, b)) in slot_bytes(&out, 2).iter().zip(slot_bytes(&next, 2)).enumerate() {
            let near = ranges.contains(&i) && a.abs_diff(b) <= 1;
            assert!(a == &b || near, "save {n}: byte {i} of the edited slot is {a}, not {b}");
        }
        // The vendor app also rewrote other bytes: its own junk in slot 1, defaults in
        // slot 3, a blank name and a new selection. 8B leaves each of them as read.
        for other in [1, 3] {
            assert_eq!(slot_bytes(&out, other), slot_bytes(&prev, other), "save {n} slot {other}");
        }
        for s in 1..=3 {
            assert_eq!(name(&out, s), name(&prev, s), "save {n}: name {s}");
        }
        assert_eq!(
            without_crc(&out)[CRC_OFFSET..NAME_OFFSET],
            without_crc(&prev)[CRC_OFFSET..NAME_OFFSET]
        );
        prev = next;
    }
}

#[test]
fn a_first_switch_edit_is_the_vendor_apps() {
    let erased = image("switch-erased");
    assert_eq!(erased_image(SWITCH).unwrap(), erased);
    let saved = image("switch-saved");
    let mut out = compile_profile(&default_profile(SWITCH), slot(1), &erased, &[]).unwrap();
    out = compile_profile(&profile(&saved, 3), slot(3), &out, &[]).unwrap();
    for n in [1, 3] {
        assert_eq!(slot_bytes(&out, n), slot_bytes(&saved, n), "slot {n}");
    }
    assert_eq!(without_crc(&out)[..MACROS_OFFSET], without_crc(&saved)[..MACROS_OFFSET]);
    // The vendor app zeroed slot 2's padding bytes; 8B never opened slot 2.
    assert_eq!(slot_bytes(&out, 2), slot_bytes(&erased, 2));
    let p = profile(&saved, 3);
    assert_eq!(target(&p, "right face"), "top face");
    assert_eq!(target(&p, "star"), "screenshot");
    let decoded = profile(&saved, 1);
    let default = default_profile(SWITCH);
    assert_eq!(
        (decoded.settings, decoded.button_mappings),
        (default.settings, default.button_mappings)
    );
}

#[test]
fn the_xinput_default_is_the_vendor_apps() {
    // Save 1 filled slot 3 with the defaults under cleared flags.
    let blob = image("saves/09");
    let out = compile_profile(&default_profile(XINPUT), slot(3), &blob, &[]).unwrap();
    assert!(slot_in_use(&out, slot(3)).unwrap());
    assert_eq!(without_crc(&deactivate_profile(&out, slot(3)).unwrap()), without_crc(&blob));
}

#[test]
fn macro_edits_pack_the_records() {
    let blob = image("saves/09");
    let out = set_macro(&blob, slot(2), MacroSlot::new(0).unwrap(), None).unwrap();
    let left = macros(&out, 2);
    assert_eq!(left.len(), 1);
    assert_eq!(
        (left[0].trigger_buttons.as_slice(), left[0].macro_slot),
        (["p2".to_owned()].as_slice(), Some(0))
    );
    let record = MACROS_OFFSET + MACROS_LEN + 8 + 96;
    assert!(out[record..record + 96].iter().all(|&b| b == 0), "the freed record is zeroed");

    let back = macros(&blob, 2)[0].clone();
    let out = set_macro(&out, slot(2), MacroSlot::new(3).unwrap(), Some(&back)).unwrap();
    let triggers: Vec<Vec<String>> =
        macros(&out, 2).into_iter().map(|d| d.trigger_buttons).collect();
    assert_eq!(triggers, [vec!["p2".to_owned()], vec!["p1".to_owned()]], "packed after the last");

    let dropped = drop_macros(&blob, slot(2), &["p1".to_owned()]).unwrap();
    let triggers: Vec<Vec<String>> =
        macros(&dropped, 2).into_iter().map(|d| d.trigger_buttons).collect();
    assert_eq!(triggers, [vec![], vec!["p2".to_owned()]]);
}

#[test]
fn a_profile_never_goes_into_the_other_modes_image() {
    let switch = image("switch-saved");
    assert!(compile_profile(&default_profile(XINPUT), slot(1), &switch, &[]).is_err());
}
