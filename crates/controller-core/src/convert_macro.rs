//! Converts a macro made for one mode into another mode, for an import from a file.
//!
//! A button name is the canonical position name in every mode, so a button keeps its
//! name and its place on the controller; only the printed label differs, and the
//! description holds it. What can change is the content. A trigger press becomes a full
//! press in a mode whose triggers are on or off. A trigger button that cannot start the
//! macro in the target slot is dropped. Each loss is listed for the import warning.

use crate::description::{ControllerDescription, TriggerKind};
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, GlideControl, MacroDefinition, Mode};
use crate::orchestrator::macros::bank_refusal;

/// One thing a conversion removed from a macro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroLoss {
    /// What was removed: a button name, or a glide such as `left_trigger glide in step 3`.
    pub item: String,
    /// Why it could not stay, fit to show to the user.
    pub reason: String,
}

/// The state of the profile slot that takes the converted macro, in the target mode.
#[derive(Debug, Clone, Copy)]
pub struct TargetBank<'a> {
    /// The slot's macros other than the one the converted macro takes.
    pub others: &'a [MacroDefinition],
    /// The slot's profile.
    pub profile: &'a CanonicalProfile,
    /// The target mode's default profile.
    pub default: &'a CanonicalProfile,
}

/// Converts `def` into mode `to`, for a slot in the state of `bank`.
///
/// A macro already in `to` comes back unchanged. Otherwise each trigger button that
/// starts another macro of the slot, or has another output than in the default profile,
/// is dropped (a macro may be left with no trigger button), and a trigger glide is
/// dropped when the target mode's triggers are on or off. The converted macro keeps
/// its macro slot.
///
/// # Errors
/// Returns [`Error::Invalid`] if `description` has no entry for `to`, or a step
/// button is unknown or has no label in `to`.
pub fn convert_macro(
    description: &ControllerDescription,
    def: &MacroDefinition,
    to: Mode,
    bank: &TargetBank<'_>,
) -> Result<(MacroDefinition, Vec<MacroLoss>)> {
    let Some(mode) = description.mode(to) else {
        return Err(Error::Invalid(crate::Reason::NoMode(to)));
    };
    if def.mode == to {
        return Ok((def.clone(), Vec::new()));
    }
    let label = description.mode_label(to);
    if let Some(button) = def
        .steps
        .iter()
        .flat_map(|s| &s.pressed_buttons)
        .find(|name| !description.button(name).is_some_and(|b| b.labels.contains_key(&to)))
    {
        let (button, from) = (button.clone(), def.mode);
        return Err(Error::Invalid(crate::Reason::ButtonNotInMode { button, from, mode: label }));
    }
    let mut converted = def.clone();
    converted.mode = to;
    converted.unknown_trigger_bits = 0;
    let mut losses = Vec::new();
    converted.trigger_buttons.retain(|button| {
        let refusal = bank_refusal(button, bank.others, bank.profile, bank.default);
        if let Some(reason) = &refusal {
            losses.push(MacroLoss { item: button.clone(), reason: reason.clone() });
        }
        refusal.is_none()
    });
    if mode.trigger_kind == TriggerKind::OnOff {
        for (index, step) in converted.steps.iter_mut().enumerate() {
            for control in [GlideControl::LeftTrigger, GlideControl::RightTrigger] {
                if step.glide.remove(&control) {
                    losses.push(MacroLoss {
                        item: format!("{} glide in step {}", control.as_str(), index + 1),
                        reason: format!("Triggers are on or off in {label} mode."),
                    });
                }
            }
            for value in [&mut step.trigger_left, &mut step.trigger_right] {
                *value = if *value == 0 { 0 } else { u8::MAX };
            }
        }
    }
    Ok((converted, losses))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::{ControllerSpec, ProtocolCodec};
    use crate::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
    use crate::model::{ButtonMapping, MacroStep};

    fn def(mode: Mode, triggers: &[&str]) -> MacroDefinition {
        MacroDefinition {
            name: "Combo".into(),
            mode,
            trigger_buttons: triggers.iter().map(|&b| b.to_owned()).collect(),
            unknown_trigger_bits: 0,
            repeat_count: 2,
            interval_ms: 10,
            steps: vec![
                MacroStep {
                    duration_ms: 100,
                    pressed_buttons: vec!["bottom face".into()],
                    trigger_left: 128,
                    left_stick_x: 0,
                    glide: [GlideControl::LeftTrigger, GlideControl::LeftStick].into(),
                    ..MacroStep::default()
                },
                MacroStep { duration_ms: 50, trigger_left: 0, ..MacroStep::default() },
            ],
            macro_slot: Some(1),
        }
    }

    /// Converts into `to` for a slot with no other macro and a default profile.
    fn convert(def: &MacroDefinition, to: Mode) -> (MacroDefinition, Vec<MacroLoss>) {
        let default = Pro3.default_profile(to);
        let bank = TargetBank { others: &[], profile: &default, default: &default };
        convert_macro(Pro3.description().unwrap(), def, to, &bank).unwrap()
    }

    #[test]
    fn xinput_to_switch_makes_a_trigger_press_full_and_drops_its_glide() {
        let (out, losses) = convert(&def(XINPUT, &["l1", "r4"]), SWITCH);
        assert_eq!(out.mode, SWITCH);
        assert_eq!(out.trigger_buttons, ["l1", "r4"]);
        assert_eq!((out.steps[0].trigger_left, out.steps[1].trigger_left), (255, 0));
        assert_eq!(out.steps[0].glide, [GlideControl::LeftStick].into(), "stick glides stay");
        assert_eq!(
            losses,
            [MacroLoss {
                item: "left_trigger glide in step 1".into(),
                reason: "Triggers are on or off in Switch mode.".into()
            }]
        );
        assert_eq!((out.name, out.macro_slot), ("Combo".into(), Some(1)));
    }

    #[test]
    fn switch_to_dinput_and_back_keep_the_steps() {
        let mut source = def(SWITCH, &["l1"]);
        source.steps[0].glide.clear();
        source.steps[0].trigger_left = 255;
        let (dinput, losses) = convert(&source, DINPUT);
        assert_eq!((losses, dinput.mode), (vec![], DINPUT));
        assert_eq!(dinput.steps, source.steps);
        let (back, losses) = convert(&dinput, SWITCH);
        assert_eq!((losses, back), (vec![], source));
    }

    #[test]
    fn a_trigger_button_that_starts_another_macro_is_dropped() {
        let default = Pro3.default_profile(DINPUT);
        let other = MacroDefinition { macro_slot: Some(2), ..def(DINPUT, &["l1"]) };
        let bank = TargetBank { others: &[other], profile: &default, default: &default };
        let (out, losses) =
            convert_macro(Pro3.description().unwrap(), &def(XINPUT, &["l1", "r1"]), DINPUT, &bank)
                .unwrap();
        assert_eq!(out.trigger_buttons, ["r1"]);
        assert_eq!(losses.len(), 1);
        assert_eq!(losses[0].item, "l1");
        assert!(losses[0].reason.contains("macro slot 2"), "{}", losses[0].reason);
    }

    #[test]
    fn a_trigger_button_with_another_output_is_dropped() {
        let default = Pro3.default_profile(SWITCH);
        let mut profile = default.clone();
        profile.button_mappings.retain(|m| m.source != "r1");
        profile
            .button_mappings
            .push(ButtonMapping { source: "r1".into(), target: "bottom face".into() });
        let bank = TargetBank { others: &[], profile: &profile, default: &default };
        let (out, losses) =
            convert_macro(Pro3.description().unwrap(), &def(XINPUT, &["l1", "r1"]), SWITCH, &bank)
                .unwrap();
        assert_eq!(out.trigger_buttons, ["l1"]);
        assert_eq!(losses[0].item, "r1");
        assert!(losses[0].reason.contains("mapped to 'bottom face'"), "{}", losses[0].reason);
    }

    #[test]
    fn a_macro_can_lose_every_trigger_button() {
        let default = Pro3.default_profile(DINPUT);
        let others = [MacroDefinition { macro_slot: Some(3), ..def(DINPUT, &["l1", "r1"]) }];
        let bank = TargetBank { others: &others, profile: &default, default: &default };
        let (out, losses) =
            convert_macro(Pro3.description().unwrap(), &def(XINPUT, &["l1", "r1"]), DINPUT, &bank)
                .unwrap();
        assert_eq!(out.trigger_buttons, Vec::<String>::new());
        assert_eq!(losses.len(), 2);
        assert_eq!(out.steps, def(XINPUT, &[]).steps, "the steps stay");
    }

    #[test]
    fn the_same_mode_changes_nothing() {
        let source = def(XINPUT, &["l1"]);
        assert_eq!(convert(&source, XINPUT), (source, vec![]));
    }

    #[test]
    fn a_step_button_the_target_mode_lacks_is_an_error_naming_it() {
        let mut source = def(XINPUT, &[]);
        source.steps[1].pressed_buttons.push("no such button".into());
        let default = Pro3.default_profile(SWITCH);
        let bank = TargetBank { others: &[], profile: &default, default: &default };
        let err = convert_macro(Pro3.description().unwrap(), &source, SWITCH, &bank).unwrap_err();
        assert!(err.to_string().contains("'no such button'"), "{err}");
    }
}
