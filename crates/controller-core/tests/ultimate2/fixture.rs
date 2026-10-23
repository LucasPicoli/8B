//! The profile files in `fixtures/ultimate-2/check/` and `fixtures/ultimate-2/gui-check/`,
//! the comparison of a slot read from a pad with one of them, and the presses that prove
//! the pad plays the check's file.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use controller_core::model::{CanonicalProfile, MacroDefinition, MacroStep, Mode};
use serde_json::Value;

/// The slot the check writes, 1-based. Slot 1 is the one a pad usually plays.
pub const SLOT: u8 = 3;

/// The text of the file for `mode`.
fn text(mode: Mode) -> String {
    std::fs::read_to_string(format!("../../fixtures/ultimate-2/check/{mode}.json")).unwrap()
}

/// The file for `mode`: its profile and its macros.
pub fn fixture(mode: Mode) -> (CanonicalProfile, Vec<MacroDefinition>) {
    parse(&text(mode))
}

/// The profile file at `path`, from the crate's folder: its profile and its macros, none
/// when it has none.
pub fn load(path: &str) -> (CanonicalProfile, Vec<MacroDefinition>) {
    parse(&std::fs::read_to_string(path).expect(path))
}

/// A profile file's text as its profile and its macros.
fn parse(text: &str) -> (CanonicalProfile, Vec<MacroDefinition>) {
    let profile: CanonicalProfile = serde_json::from_str(text).unwrap();
    let macros = profile.macros.clone().unwrap_or_default();
    (profile, macros)
}

/// The profile of the file for `mode` as a write job takes it: no macros.
pub fn profile_json(mode: Mode) -> Value {
    let mut json: Value = serde_json::from_str(&text(mode)).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("macros");
    object.remove("macro_refs");
    json
}

/// A step as one line. Button order does not matter.
fn show_step(s: &MacroStep) -> String {
    let mut buttons = s.pressed_buttons.clone();
    buttons.sort();
    format!(
        "{} ms, buttons {buttons:?}, triggers ({}, {}), sticks ({}, {}) ({}, {}), glide {:?}",
        s.duration_ms,
        s.trigger_left,
        s.trigger_right,
        s.left_stick_x,
        s.left_stick_y,
        s.right_stick_x,
        s.right_stick_y,
        s.glide
    )
}

/// Every way the slot read, `profile` and `macros`, differs from the check's file of
/// its mode, as [`differences_from`] finds them.
pub fn differences(profile: &CanonicalProfile, macros: &[MacroDefinition]) -> Vec<String> {
    let (want, want_macros) = fixture(profile.mode);
    differences_from(&want, &want_macros, profile, macros)
}

/// Every way the slot read, `profile` and `macros`, differs from `want` and
/// `want_macros`: the name, a setting, a mapping, a macro's name, trigger, repeat or step.
pub fn differences_from(
    want: &CanonicalProfile,
    want_macros: &[MacroDefinition],
    profile: &CanonicalProfile,
    macros: &[MacroDefinition],
) -> Vec<String> {
    let mut out = Vec::new();
    if profile.name != want.name {
        out.push(format!("name: expected {}, read {}", want.name, profile.name));
    }
    if profile.settings != want.settings {
        out.push(format!("settings: expected {:?}, read {:?}", want.settings, profile.settings));
    }
    for m in &want.button_mappings {
        let read = profile.button_mappings.iter().find(|g| g.source == m.source);
        if read.map(|g| &g.target) != Some(&m.target) {
            let read = read.map_or("nothing", |g| g.target.as_str());
            out.push(format!("mapping {}: expected {}, read {read}", m.source, m.target));
        }
    }
    for w in want_macros {
        let n = w.macro_slot.unwrap_or_default();
        let Some(g) = macros.iter().find(|g| g.macro_slot == w.macro_slot) else {
            out.push(format!("macro {n}: missing"));
            continue;
        };
        if g.name != w.name {
            out.push(format!("macro {n} name: expected {}, read {}", w.name, g.name));
        }
        if g.trigger_buttons != w.trigger_buttons {
            out.push(format!(
                "macro {n} trigger: expected {:?}, read {:?}",
                w.trigger_buttons, g.trigger_buttons
            ));
        }
        if (g.repeat_count, g.interval_ms) != (w.repeat_count, w.interval_ms) {
            out.push(format!(
                "macro {n} repeat: expected {}, read {}",
                w.repeat_count, g.repeat_count
            ));
        }
        let (a, b): (Vec<_>, Vec<_>) =
            (w.steps.iter().map(show_step).collect(), g.steps.iter().map(show_step).collect());
        if a != b {
            out.push(format!("macro {n} steps: expected {a:?}, read {b:?}"));
        }
    }
    for g in macros {
        if !want_macros.iter().any(|w| w.macro_slot == g.macro_slot) {
            out.push(format!("macro {}: not in the file", g.macro_slot.unwrap_or_default()));
        }
    }
    out
}

/// What one press must make the pad send.
pub enum Expect {
    /// These controls, in any order, and nothing else.
    Sends(&'static [&'static str]),
    /// Nothing at all, confirmed by the person with Enter.
    Nothing,
    /// At least these controls, among others: a motion sensor sends many small moves.
    Includes(&'static [&'static str]),
    /// A question the person answers with y or n, about what the pad shows, such as the
    /// colour of its light rings.
    Seen(&'static str),
}

/// One press the person makes, and what it must send.
pub struct Press {
    pub action: &'static str,
    pub expect: Expect,
}

/// Every press of the check, the same in both modes: the files hold the same remaps
/// and macros. Button names are the ones printed on the pad.
pub const PRESSES: [Press; 16] = [
    Press { action: "Press A (bottom face). Remapped to B.", expect: Expect::Sends(&["BTN_B"]) },
    Press { action: "Press B (right face). Remapped to A.", expect: Expect::Sends(&["BTN_A"]) },
    Press { action: "Press Y (top face). Remapped to LB.", expect: Expect::Sends(&["BTN_TL"]) },
    Press { action: "Press LB. Unchanged.", expect: Expect::Sends(&["BTN_TL"]) },
    Press { action: "Press View. Remapped to Menu.", expect: Expect::Sends(&["BTN_START"]) },
    Press { action: "Press RB. Disabled.", expect: Expect::Nothing },
    Press {
        action: "Press the left back paddle (PL). Mapped to A.",
        expect: Expect::Sends(&["BTN_A"]),
    },
    Press {
        action: "Press the right back paddle (PR). Mapped to R3.",
        expect: Expect::Sends(&["BTN_THUMBR"]),
    },
    Press {
        action: "Press X (left face). It plays a macro: Y.",
        expect: Expect::Sends(&["BTN_Y"]),
    },
    // The step holds the left trigger, stored as the vendor app stores it: a `keys` bit.
    // Stored as a `trigger_value`, it played as RT, with or without swap sticks.
    Press {
        action: "Press L4. It plays a macro: A with LT, then B.",
        expect: Expect::Sends(&["BTN_A", "LT", "BTN_B"]),
    },
    Press {
        action: "Push the LEFT stick to the RIGHT, then let go. The sticks are swapped.",
        expect: Expect::Sends(&["right stick right"]),
    },
    // Swap sticks moves the motion sensor's stick too.
    Press {
        action: "Hold LT, tilt the pad hard left, then hard right, then let go. While LT is held, the motion sensor moves the right stick, which swap sticks makes the left.",
        expect: Expect::Includes(&["LT", "left stick left", "left stick right"]),
    },
    // The effect that plays is the pad's, not the slot's: from off, the Star button
    // cycles it through Fire Ring, Custom and Light-tracing.
    Press {
        action: "Press Star until the rings spin (Fire Ring).",
        expect: Expect::Seen("Do they show green on magenta, spinning fast?"),
    },
    Press {
        action: "Press Star once for Custom.",
        expect: Expect::Seen(
            "Is each ring orange, with white at 6 o'clock, then red, then green going clockwise?",
        ),
    },
    Press {
        action: "Press Star once for Light-tracing, then move the right stick around.",
        expect: Expect::Seen("Does the lit part show red on a blue ring?"),
    },
    Press {
        action: "Press Star until the rings are off, as before the check.",
        expect: Expect::Seen("Are the rings off?"),
    },
];
