//! The profile files in `fixtures/ultimate/import/`, the comparison of a slot read from
//! a pad with one of them, and the presses that check the pad plays it. The hardware
//! check in `import_check` runs on top of these.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::d;
use controller_core::devices::ultimate::edit::{compile_profile, erased_image};
use controller_core::devices::ultimate::macros::decode_macros;
use controller_core::devices::ultimate::profile::map_profile;
use controller_core::devices::ultimate::{image_mode, SWITCH, XINPUT};
use controller_core::model::{
    CanonicalProfile, MacroDefinition, MacroStep, Mode, RawProfilePayload, Slot,
};

/// The slot both files go into, 1-based. A slot 1 import would replace the slot a pad
/// is usually set to play, so the check uses the last one.
pub const SLOT: u8 = 3;

/// A profile file and the macros it embeds.
pub struct Fixture {
    pub mode: Mode,
    pub profile: CanonicalProfile,
    pub macros: Vec<MacroDefinition>,
}

/// The text of the file for `mode`.
fn text(mode: Mode) -> String {
    std::fs::read_to_string(format!("../../fixtures/ultimate/import/{mode}.json")).unwrap()
}

/// The file for `mode`.
pub fn fixture(mode: Mode) -> Fixture {
    let profile: CanonicalProfile = serde_json::from_str(&text(mode)).unwrap();
    let macros = profile.macros.clone().unwrap();
    Fixture { mode, profile, macros }
}

/// Slot `slot` of the image `blob` as the profile and macros a read gives.
pub fn read_slot(blob: &[u8], slot: u8) -> (CanonicalProfile, Vec<MacroDefinition>) {
    let raw = RawProfilePayload {
        payload: blob.to_vec(),
        source_slot: slot,
        source_profile_index: slot - 1,
        mode_hint: image_mode(d(), blob).unwrap(),
    };
    let profile = map_profile(d(), &raw).unwrap().canonical;
    (profile, decode_macros(d(), blob, Slot::new(slot).unwrap()).unwrap())
}

/// A step as one line. Button order does not matter.
fn show_step(s: &MacroStep) -> String {
    let mut buttons = s.pressed_buttons.clone();
    buttons.sort();
    format!(
        "{} ms, buttons {buttons:?}, left stick ({}, {}), right stick ({}, {})",
        s.duration_ms, s.left_stick_x, s.left_stick_y, s.right_stick_x, s.right_stick_y
    )
}

/// Every way the slot `got` differs from the file `want`: a mapping, a macro trigger,
/// a macro step. Names, ids and settings are not compared: the pad keeps no macro name.
pub fn differences(want: &Fixture, got: &(CanonicalProfile, Vec<MacroDefinition>)) -> Vec<String> {
    let mut out = Vec::new();
    for m in &want.profile.button_mappings {
        let read = got.0.button_mappings.iter().find(|g| g.source == m.source);
        if read.map(|g| &g.target) != Some(&m.target) {
            let read = read.map_or("nothing", |g| g.target.as_str());
            out.push(format!("mapping {}: expected {}, read {read}", m.source, m.target));
        }
    }
    for w in &want.macros {
        let n = w.macro_slot.unwrap_or_default();
        let Some(g) = got.1.iter().find(|g| g.macro_slot == w.macro_slot) else {
            out.push(format!("macro {n}: missing"));
            continue;
        };
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
        if g.steps.len() != w.steps.len() {
            out.push(format!(
                "macro {n}: expected {} steps, read {}",
                w.steps.len(),
                g.steps.len()
            ));
        }
        for (i, (a, b)) in w.steps.iter().zip(&g.steps).enumerate() {
            if show_step(a) != show_step(b) {
                out.push(format!(
                    "macro {n} step {}: expected {}; read {}",
                    i + 1,
                    show_step(a),
                    show_step(b)
                ));
            }
        }
    }
    for g in &got.1 {
        if !want.macros.iter().any(|w| w.macro_slot == g.macro_slot) {
            out.push(format!("macro {}: not in the file", g.macro_slot.unwrap_or_default()));
        }
    }
    out
}

/// One press for the person: what to do, and what should come out.
pub struct Prompt {
    pub action: String,
    pub expect: String,
}

fn prompt(action: impl Into<String>, expect: impl Into<String>) -> Prompt {
    Prompt { action: action.into(), expect: expect.into() }
}

/// The name printed on the pad for button `id`.
fn printed(mode: Mode, id: &str) -> String {
    d().button(id).unwrap().printed(mode).to_owned()
}

/// What the pad sends for button `id` in `mode`, as a tester names it.
fn sends(mode: Mode, id: &str) -> String {
    d().button(id).unwrap().labels[&mode].clone()
}

/// A stick position of a step, such as "left stick up". Empty at centre.
fn direction(side: &str, x: u8, y: u8) -> Option<String> {
    let word = match (x, y) {
        (127, 0) => "up",
        (127, 255) => "down",
        (0, 127) => "left",
        (255, 127) => "right",
        (127, 127) => return None,
        _ => "diagonal",
    };
    Some(format!("{side} stick {word}"))
}

/// What a macro plays, step by step, such as "A, then left stick up".
fn plays(mode: Mode, m: &MacroDefinition) -> String {
    let parts: Vec<String> = m
        .steps
        .iter()
        .filter_map(|s| {
            let mut now: Vec<String> = s.pressed_buttons.iter().map(|b| sends(mode, b)).collect();
            now.extend(direction("left", s.left_stick_x, s.left_stick_y));
            now.extend(direction("right", s.right_stick_x, s.right_stick_y));
            (!now.is_empty()).then(|| now.join(" and "))
        })
        .collect();
    parts.join(", then ")
}

/// The presses for the remap of `source` to the extra output `target`.
fn extra(mode: Mode, source: &str, target: &str) -> Vec<Prompt> {
    let (this, a, b) =
        (printed(mode, source), printed(mode, "right face"), printed(mode, "bottom face"));
    let (sends_a, sends_b) = (sends(mode, "right face"), sends(mode, "bottom face"));
    match target {
        "turbo" => vec![
            prompt(
                format!("Hold {a}, tap {this} once, then let go of both."),
                "The tester shows nothing.",
            ),
            prompt(
                format!("Hold {a} for 2 seconds."),
                format!("The tester shows {sends_a} turning on and off by itself."),
            ),
            prompt(
                format!("Hold {a}, tap {this} once, then let go of both."),
                "The tester shows nothing.",
            ),
            prompt(
                format!("Hold {a} for 2 seconds."),
                format!("The tester shows {sends_a} held on."),
            ),
        ],
        "swap" => vec![
            prompt(
                format!("Hold {a} and {b}, tap {this} once, then let go of all three."),
                "The tester shows nothing.",
            ),
            prompt(format!("Press {a}."), format!("The tester shows {sends_b}, not {sends_a}.")),
            prompt(
                format!("Hold {a}, tap {this} once, then let go of both."),
                "The tester shows nothing.",
            ),
            prompt(format!("Press {a}."), format!("The tester shows {sends_a} again.")),
        ],
        "screenshot" => {
            vec![prompt(format!("Press {this}."), "The tester shows the Screenshot button.")]
        }
        other => {
            vec![prompt(format!("Press {this}."), format!("The tester shows the {other} output."))]
        }
    }
}

/// Every press the person makes for `want`, in order: each remapped button, then each
/// macro trigger.
pub fn prompts(want: &Fixture) -> Vec<Prompt> {
    let mode = want.mode;
    let extras = &d().mode(mode).unwrap().extra_outputs;
    let mut out = Vec::new();
    for m in &want.profile.button_mappings {
        if m.source == m.target {
            continue;
        }
        if extras.iter().any(|e| e.id == m.target) {
            out.extend(extra(mode, &m.source, &m.target));
        } else {
            out.push(prompt(
                format!("Press {}.", printed(mode, &m.source)),
                format!("The tester shows {}.", sends(mode, &m.target)),
            ));
        }
    }
    for m in &want.macros {
        let trigger = &m.trigger_buttons[0];
        out.push(prompt(
            format!("Press {} once.", printed(mode, trigger)),
            format!("The tester shows, in order: {}.", plays(mode, m)),
        ));
    }
    out
}

fn slot(n: u8) -> Slot {
    Slot::new(n).unwrap()
}

/// The image of `mode` after the file goes into [`SLOT`] of an erased pad.
fn compiled(want: &Fixture) -> Vec<u8> {
    let erased = erased_image(d(), want.mode).unwrap();
    compile_profile(d(), &want.profile, slot(SLOT), &erased, &want.macros).unwrap()
}

#[test]
fn both_files_pass_validation_and_keep_the_export_shape() {
    for mode in [XINPUT, SWITCH] {
        let value: serde_json::Value = serde_json::from_str(&text(mode)).unwrap();
        let check =
            controller_core::service::validation::validate_profile(crate::ultimate(), &value)
                .unwrap();
        assert!(check.valid, "{mode}: {:?}", check.errors);
        // The GUI's export writes pretty JSON and a final newline.
        let again = serde_json::to_string_pretty(&fixture(mode).profile).unwrap() + "\n";
        assert_eq!(again, text(mode), "{mode}");
    }
}

#[test]
fn each_file_remaps_buttons_and_holds_a_button_and_a_stick_macro() {
    for mode in [XINPUT, SWITCH] {
        let want = fixture(mode);
        assert_eq!((want.profile.mode, want.profile.preferred_slot), (mode, Some(SLOT)));
        let remapped = want.profile.button_mappings.iter().filter(|m| m.source != m.target);
        assert_eq!(remapped.count(), 6, "{mode}");
        assert_eq!(want.macros.len(), 2, "{mode}");
        assert!(want.macros[0]
            .steps
            .iter()
            .all(|s| (s.left_stick_x, s.left_stick_y) == (127, 127)));
        assert!(want.macros[1]
            .steps
            .iter()
            .any(|s| (s.left_stick_x, s.left_stick_y) != (127, 127)));
        assert!(want.macros[1]
            .steps
            .iter()
            .any(|s| (s.right_stick_x, s.right_stick_y) != (127, 127)));
    }
}

#[test]
fn a_compiled_file_reads_back_the_same_mappings_and_macros() {
    for mode in [XINPUT, SWITCH] {
        let want = fixture(mode);
        let got = read_slot(&compiled(&want), SLOT);
        assert_eq!(differences(&want, &got), Vec::<String>::new(), "{mode}");
        assert_eq!(got.1.len(), want.macros.len());
    }
}

#[test]
fn a_wrong_mapping_is_listed() {
    let want = fixture(XINPUT);
    let mut got = read_slot(&compiled(&want), SLOT);
    got.0.button_mappings.iter_mut().find(|m| m.source == "l1").unwrap().target = "l1".into();
    assert_eq!(differences(&want, &got), ["mapping l1: expected r1, read l1"]);
}

#[test]
fn a_wrong_macro_step_and_trigger_are_listed() {
    let want = fixture(SWITCH);
    let mut got = read_slot(&compiled(&want), SLOT);
    got.1[1].steps[2].duration_ms = 500;
    got.1[0].trigger_buttons = vec!["r3".into()];
    let diff = differences(&want, &got);
    assert_eq!(diff.len(), 2, "{diff:?}");
    assert!(diff[0].starts_with("macro 0 trigger: expected [\"l3\"]"), "{diff:?}");
    assert!(diff[1].starts_with("macro 1 step 3: expected 400 ms"), "{diff:?}");
    assert!(diff[1].contains("read 500 ms"), "{diff:?}");
}

#[test]
fn a_missing_or_extra_macro_is_listed() {
    let want = fixture(XINPUT);
    let mut got = read_slot(&compiled(&want), SLOT);
    let extra = got.1.remove(1);
    assert_eq!(differences(&want, &got), ["macro 1: missing"]);
    got.1.push(MacroDefinition { macro_slot: Some(2), ..extra });
    assert_eq!(differences(&want, &got), ["macro 1: missing", "macro 2: not in the file"]);
}

#[test]
fn the_presses_name_each_button_and_macro() {
    let say = |mode| -> Vec<String> {
        prompts(&fixture(mode)).iter().map(|p| format!("{} -> {}", p.action, p.expect)).collect()
    };
    let x = say(XINPUT);
    assert_eq!(x[0], "Press X. -> The tester shows A.");
    assert_eq!(x.len(), 3 + 4 + 1 + 4 + 2, "{x:#?}");
    assert_eq!(x[x.len() - 2], "Press LS once. -> The tester shows, in order: A, then B, then Y.");
    assert_eq!(
        x[x.len() - 1],
        "Press RS once. -> The tester shows, in order: left stick up, then left stick right, then right stick down, then right stick left."
    );
    let s = say(SWITCH);
    assert!(
        s.contains(&"Press Star. -> The tester shows the Screenshot button.".to_owned()),
        "{s:#?}"
    );
}
