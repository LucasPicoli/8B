//! Every window control writes the profile field it names, and only that one. The
//! window and the CLI `upload` verb send the same profile JSON, so this closes the
//! one link a hardware run cannot see: control to field.
//!
//! The expected table below is written by hand, apart from the tables in
//! `buttons.rs` and `settings.rs` that build the controls, so a pointer swapped
//! there (left X wired to right X) fails here.

#![allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::*;
use crate::buttons::{picked_output, rows};
use crate::state::tests::connected;

use controller_core::devices::pro3::{SWITCH, XINPUT};

/// Key of a control, then the JSON pointer of the field it sets. A button row
/// names its mapping by source button, as `/button_mappings/<source>/target`.
const EXPECTED: &[(&str, &str)] = &[
    ("Buttons||A", "/button_mappings/bottom face/target"),
    ("Buttons||B", "/button_mappings/right face/target"),
    ("Buttons||X", "/button_mappings/left face/target"),
    ("Buttons||Y", "/button_mappings/top face/target"),
    ("Buttons||LB", "/button_mappings/l1/target"),
    ("Buttons||RB", "/button_mappings/r1/target"),
    ("Buttons||LT", "/button_mappings/l2/target"),
    ("Buttons||RT", "/button_mappings/r2/target"),
    ("Buttons||LS", "/button_mappings/l3/target"),
    ("Buttons||RS", "/button_mappings/r3/target"),
    ("Buttons||Menu", "/button_mappings/start/menu/target"),
    ("Buttons||View", "/button_mappings/select/back/target"),
    ("Buttons||D-pad up", "/button_mappings/d-pad up/target"),
    ("Buttons||D-pad down", "/button_mappings/d-pad down/target"),
    ("Buttons||D-pad left", "/button_mappings/d-pad left/target"),
    ("Buttons||D-pad right", "/button_mappings/d-pad right/target"),
    ("Buttons||PL", "/button_mappings/lp/target"),
    ("Buttons||PR", "/button_mappings/rp/target"),
    ("Buttons||L4", "/button_mappings/l4/target"),
    ("Buttons||R4", "/button_mappings/r4/target"),
    ("Buttons||Turbo", "/button_mappings/turbo/target"),
    ("Sticks|Left stick|Dead zone low", "/sticks/left_min_pct"),
    ("Sticks|Left stick|Dead zone high", "/sticks/left_max_pct"),
    ("Sticks|Left stick|Invert X", "/sticks/invert_left_x"),
    ("Sticks|Left stick|Invert Y", "/sticks/invert_left_y"),
    ("Sticks|Right stick|Dead zone low", "/sticks/right_min_pct"),
    ("Sticks|Right stick|Dead zone high", "/sticks/right_max_pct"),
    ("Sticks|Right stick|Invert X", "/sticks/invert_right_x"),
    ("Sticks|Right stick|Invert Y", "/sticks/invert_right_y"),
    ("Sticks|Swaps|Swap the left and right sticks", "/sticks/swap_sticks"),
    ("Sticks|Swaps|Swap the D-pad and the left stick", "/sticks/swap_dpad_with_left_stick"),
    ("Triggers analog|Left trigger|Range low", "/triggers/left_min_pct"),
    ("Triggers analog|Left trigger|Range high", "/triggers/left_max_pct"),
    ("Triggers analog|Right trigger|Range low", "/triggers/right_min_pct"),
    ("Triggers analog|Right trigger|Range high", "/triggers/right_max_pct"),
    ("Triggers analog|Swaps|Swap the left and right triggers", "/triggers/swap_triggers"),
    ("Triggers threshold|Left trigger|Press point", "/triggers/left_threshold_pct"),
    ("Triggers threshold|Right trigger|Press point", "/triggers/right_threshold_pct"),
    ("Triggers threshold|Swaps|Swap the left and right triggers", "/triggers/swap_triggers"),
    ("Vibration|Motors|Left motor", "/vibration/left_level"),
    ("Vibration|Motors|Right motor", "/vibration/right_level"),
];

/// A Pro 3 read in `XInput`, or with a default Switch profile in slot 1 of Switch.
/// Returns the state and the slot its controls edit.
fn start(switch: bool) -> (AppState, (Mode, u8)) {
    let mut s = connected(XINPUT);
    if !switch {
        return (s, (XINPUT, 1));
    }
    s.select(1, 0);
    let default = s.defaults()[&SWITCH].clone();
    s.active_mut().unwrap().slots.get_mut(&(SWITCH, 1)).unwrap().pad = Some(default);
    (s, (SWITCH, 1))
}

/// Every leaf of `json` by pointer.
fn leaves(json: &Value, at: &str, out: &mut BTreeMap<String, Value>) {
    match json {
        Value::Object(m) => m.iter().for_each(|(k, v)| leaves(v, &format!("{at}/{k}"), out)),
        Value::Array(a) => {
            a.iter().enumerate().for_each(|(i, v)| leaves(v, &format!("{at}/{i}"), out));
        }
        _ => {
            out.insert(at.to_owned(), json.clone());
        }
    }
}

/// The pointer of the one field that differs between the slot's profile and the
/// upload of its edits. Panics, naming `key`, when none or several differ.
fn written_field(s: &AppState, slot: (Mode, u8), key: &str) -> String {
    let Some(WriteOp::Upload { profile, .. }) = s.upload_job(slot).map(|j| j.op) else {
        panic!("{key}: no upload");
    };
    let mut pad = serde_json::to_value(s.slot(slot.0, slot.1).pad.unwrap()).unwrap();
    // A default profile has no id until it is written.
    pad["id"] = profile["id"].clone();
    let (mut old, mut new) = (BTreeMap::new(), BTreeMap::new());
    leaves(&pad, "", &mut old);
    leaves(&profile, "", &mut new);
    let diff: Vec<_> = old
        .keys()
        .chain(new.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|k| old.get(*k) != new.get(*k))
        .collect();
    let [field] = diff[..] else { panic!("{key}: expected one changed field, got {diff:?}") };
    // Name a mapping by its source button, not by its place in the list.
    field.strip_suffix("/target").filter(|p| p.starts_with("/button_mappings/")).map_or_else(
        || field.clone(),
        |entry| {
            let source = pad.pointer(&format!("{entry}/source")).and_then(Value::as_str).unwrap();
            format!("/button_mappings/{source}/target")
        },
    )
}

/// Each control of one window form, moved on its own: its key and the field the
/// upload changed. `switch` picks the threshold trigger form.
fn walk(switch: bool) -> BTreeMap<String, String> {
    let (s, slot) = start(switch);
    let tab = if switch { "Triggers threshold" } else { "Triggers analog" };
    let mut seen = BTreeMap::new();
    let mut record = |key: String, field: String| {
        if let Some(other) = seen.insert(key.clone(), field.clone()) {
            panic!("{key} appears twice ({other}, {field})");
        }
    };
    let tabs = ["Sticks", tab, "Vibration"];
    for (name, page) in tabs.iter().zip(pages_of(&s, slot)) {
        for frame in page.frames.iter() {
            for setting in frame.settings.iter() {
                let ends = std::iter::once((" low", setting.low_field.clone(), setting.low))
                    .chain(
                        (!setting.high_field.is_empty())
                            .then(|| (" high", setting.high_field.clone(), setting.high)),
                    )
                    .collect::<Vec<_>>();
                let single = setting.high_field.is_empty();
                for (end, field, old) in ends {
                    // The limit farther from the slot's value, so it always differs.
                    let far = if old - setting.minimum >= setting.maximum - old {
                        setting.minimum
                    } else {
                        setting.maximum
                    };
                    let (mut s, slot) = start(switch);
                    s.set_number(&field, far);
                    let key = format!(
                        "{name}|{}|{}{}",
                        frame.title,
                        setting.label,
                        if single { "" } else { end }
                    );
                    record(key.clone(), written_field(&s, slot, &key));
                }
            }
            for toggle in frame.toggles.iter() {
                let (mut s, slot) = start(switch);
                s.set_flag(&toggle.field, !toggle.on);
                let key = format!("{name}|{}|{}", frame.title, toggle.label);
                record(key.clone(), written_field(&s, slot, &key));
            }
        }
    }
    if !switch {
        for (row, map_row) in rows(&s).iter().enumerate().filter(|(_, r)| !r.fixed) {
            let (mut s, slot) = start(false);
            let (button, output) = (0..2).find_map(|c| picked_output(&s, row, c)).unwrap();
            s.set_output(&button, &output);
            let key = format!("Buttons||{}", map_row.label);
            record(key.clone(), written_field(&s, slot, &key));
        }
    }
    seen
}

#[test]
fn each_control_writes_only_the_field_it_names() {
    let mut observed = walk(false);
    for (key, field) in walk(true) {
        if let Some(other) = observed.insert(key.clone(), field.clone()) {
            assert_eq!(other, field, "{key} differs between the two trigger forms");
        }
    }
    let expected: BTreeMap<_, _> =
        EXPECTED.iter().map(|&(k, p)| (k.to_owned(), p.to_owned())).collect();
    assert_eq!(expected.len(), EXPECTED.len(), "a key listed twice");
    let keys: BTreeSet<_> = observed.keys().chain(expected.keys()).collect();
    let wrong: Vec<_> = keys
        .into_iter()
        .filter(|k| observed.get(*k) != expected.get(*k))
        .map(|k| format!("{k}: window sets {:?}, expected {:?}", observed.get(k), expected.get(k)))
        .collect();
    assert!(wrong.is_empty(), "controls and expected table disagree:\n{}", wrong.join("\n"));
}
