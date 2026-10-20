//! What the slot's button map remaps each button to, for the macro editor. A step presses
//! the button itself, so a step on X, in a slot that remaps X to the bottom button's
//! output, sends that output, as the Buttons tab names it.

use controller_core::description::{Button, ControllerDescription};
use controller_core::model::Mode;
use tr::tr;

use crate::buttons::{output_label, target};
use crate::state::AppState;

/// The name of `output`, which the map gives `button`, in `mode`, such as "A". When that
/// name is the button's own printed label, as on a pad whose print differs from the
/// mode's names, the button whose output it is follows: "A (B's output)".
fn target_text(d: &ControllerDescription, mode: Mode, button: &Button, output: &str) -> String {
    let name = output_label(d, mode, output);
    match d.button(output) {
        Some(from) if name == button.printed(mode) => {
            tr!("{output} ({button}'s output)", output = name, button = from.printed(mode))
        }
        _ => name,
    }
}

/// The buttons a step can press that the selected slot's button map remaps, in `mode`, as
/// (button id, the output's name). A trigger value is left
/// out: no pad has shown that it goes through the map.
#[must_use]
pub fn step_remaps(state: &AppState, mode: Mode) -> Vec<(String, String)> {
    let Some((_, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    let Some(profile) = slot.shown() else { return Vec::new() };
    let description = state.description();
    description
        .buttons
        .iter()
        .filter(|b| b.can_be_in_macro_step && b.step_trigger_value.is_none())
        .filter_map(|b| {
            let output = target(profile, &b.id);
            (output != b.id).then(|| (b.id.clone(), target_text(description, mode, b, output)))
        })
        .collect()
}

/// The output `remaps` give button `id`, or "" when the map leaves it as it is.
#[must_use]
pub fn remapped_to<'a>(remaps: &'a [(String, String)], id: &str) -> &'a str {
    remaps.iter().find(|(b, _)| b == id).map_or("", |(_, s)| s.as_str())
}

/// `label` of button `id` with the output `remaps` give it, such as "X (remapped to A)".
/// `label` alone for a button the map leaves as it is.
#[must_use]
pub fn with_play(remaps: &[(String, String)], id: &str, label: &str) -> String {
    match remapped_to(remaps, id) {
        "" => label.to_owned(),
        output => tr!("{button} (remapped to {output})", button = label, output = output),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use controller_core::description::DISABLED_OUTPUT;
    use controller_core::devices::{pro3, ultimate};

    use super::*;
    use crate::state::tests::{connected, ultimate as ultimate_bluetooth};

    /// The id of the button printed `label` in `mode`.
    fn printed(s: &AppState, mode: Mode, label: &str) -> String {
        s.description().buttons.iter().find(|b| b.printed(mode) == label).unwrap().id.clone()
    }

    #[test]
    fn a_remapped_ultimate_button_names_the_slots_output() {
        let mut s = ultimate_bluetooth("1.09");
        s.select(0, 0);
        let x = printed(&s, ultimate::XINPUT, "X");
        assert!(step_remaps(&s, ultimate::XINPUT).is_empty(), "the default map");
        s.set_output(&x, "bottom face");
        let remaps = step_remaps(&s, ultimate::XINPUT);
        assert_eq!(remapped_to(&remaps, &x), "A");
        assert_eq!(with_play(&remaps, &x, "X"), "X (remapped to A)");
        assert_eq!(with_play(&remaps, "right face", "A"), "A", "not remapped");
    }

    #[test]
    fn a_name_that_matches_the_print_names_the_button_whose_output_it_is() {
        let mut s = ultimate_bluetooth("1.09");
        s.select(0, 0);
        // The right button is printed A, and the bottom one, printed B, sends A.
        s.set_output("right face", "bottom face");
        let remaps = step_remaps(&s, ultimate::XINPUT);
        assert_eq!(remapped_to(&remaps, "right face"), "A (B's output)");
    }

    #[test]
    fn a_disabled_button_is_remapped_to_disabled() {
        let mut s = ultimate_bluetooth("1.09");
        s.select(0, 0);
        let x = printed(&s, ultimate::XINPUT, "X");
        s.set_output(&x, DISABLED_OUTPUT);
        assert_eq!(remapped_to(&step_remaps(&s, ultimate::XINPUT), &x), "Disabled");
    }

    #[test]
    fn a_remapped_pro_3_button_names_the_slots_output_and_trigger_values_are_left_out() {
        let mut s = connected(pro3::XINPUT);
        s.select(0, 0);
        s.set_output("left face", "bottom face");
        s.set_output("l2", "r2");
        let remaps = step_remaps(&s, pro3::XINPUT);
        assert_eq!(with_play(&remaps, "left face", "X"), "X (remapped to A)");
        assert_eq!(remapped_to(&remaps, "l2"), "", "LT is held as a trigger value");
    }
}
