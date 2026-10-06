//! Converts a profile made for one mode into another mode, for an import from a file.
//!
//! Buttons map by position, so a mapping keeps its canonical names and only the
//! printed labels differ. Two things can be lost: an output the target mode lacks
//! becomes [`DISABLED_OUTPUT`], and triggers of another form start from the target
//! mode's defaults. Each loss is listed for the import warning.

use std::mem::discriminant;

use crate::description::{ControllerDescription, DISABLED_OUTPUT};
use crate::error::{Error, Result};
use crate::model::CanonicalProfile;

/// One change a conversion made that the user may not want.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionLoss {
    /// The button's output does not exist in the target mode, so it is now disabled.
    OutputDisabled {
        /// Canonical name of the button.
        source: String,
        /// The output it had.
        target: String,
    },
    /// The target mode tunes triggers in another form, so they start from its defaults.
    TriggersReset,
}

/// Converts `profile` into the mode of `target_default`, the profile a new slot of
/// that mode starts from. A profile already in that mode comes back unchanged.
///
/// # Errors
/// Returns [`Error::Validation`] if `description` has no entry for the target mode.
pub fn convert_profile(
    description: &ControllerDescription,
    profile: &CanonicalProfile,
    target_default: &CanonicalProfile,
) -> Result<(CanonicalProfile, Vec<ConversionLoss>)> {
    let to = target_default.mode;
    let Some(mode) = description.mode(to) else {
        return Err(Error::Validation(format!("This controller has no {to} mode.")));
    };
    let mut converted = profile.clone();
    let mut losses = Vec::new();
    if profile.mode == to {
        return Ok((converted, losses));
    }
    converted.mode = to;
    for mapping in &mut converted.button_mappings {
        let target = mapping.target.as_str();
        let exists = target == DISABLED_OUTPUT
            || description.button(target).is_some()
            || mode.extra_outputs.iter().any(|o| o.id == target);
        if !exists {
            losses.push(ConversionLoss::OutputDisabled {
                source: mapping.source.clone(),
                target: std::mem::replace(&mut mapping.target, DISABLED_OUTPUT.to_owned()),
            });
        }
    }
    if discriminant(&profile.triggers) != discriminant(&target_default.triggers) {
        converted.triggers = target_default.triggers.clone();
        losses.push(ConversionLoss::TriggersReset);
    }
    Ok((converted, losses))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::description::UNRECOGNISED_OUTPUT;
    use crate::device::{ControllerSpec, ProtocolCodec};
    use crate::devices::pro3::Pro3;
    use crate::model::{ButtonMapping, Mode};

    fn convert(profile: &CanonicalProfile, to: Mode) -> (CanonicalProfile, Vec<ConversionLoss>) {
        convert_profile(Pro3.description().unwrap(), profile, &Pro3.default_profile(to)).unwrap()
    }

    fn with_mapping(mode: Mode, source: &str, target: &str) -> CanonicalProfile {
        let mut p = Pro3.default_profile(mode);
        p.name = "Mine".to_owned();
        p.button_mappings.retain(|m| m.source != source);
        p.button_mappings
            .push(ButtonMapping { source: source.to_owned(), target: target.to_owned() });
        p
    }

    #[test]
    fn every_mode_pair_gives_a_valid_profile_of_the_target_mode() {
        let d = Pro3.description().unwrap();
        for from in Mode::ALL {
            for to in Mode::ALL {
                let source = with_mapping(from, "r4", "bottom face");
                let (p, losses) = convert(&source, to);
                assert_eq!(p.mode, to, "{from} to {to}");
                let reset = from != to
                    && d.mode(from).unwrap().trigger_kind != d.mode(to).unwrap().trigger_kind;
                assert_eq!(
                    losses,
                    if reset { vec![ConversionLoss::TriggersReset] } else { vec![] },
                    "{from} to {to}"
                );
                let expected =
                    if reset { &Pro3.default_profile(to).triggers } else { &source.triggers };
                assert_eq!(&p.triggers, expected, "{from} to {to}");
                assert_eq!(p.button_mappings, source.button_mappings, "{from} to {to}");
                assert_eq!(
                    (p.name.as_str(), &p.sticks, &p.vibration),
                    ("Mine", &source.sticks, &source.vibration)
                );
            }
        }
    }

    #[test]
    fn xinput_and_dinput_round_trip_unchanged() {
        let source = with_mapping(Mode::XInput, "lp", "right face");
        let (dinput, losses) = convert(&source, Mode::DInput);
        assert_eq!(losses, []);
        let (back, losses) = convert(&dinput, Mode::XInput);
        assert_eq!(losses, []);
        assert_eq!(back, source);
    }

    #[test]
    fn switch_screenshot_into_xinput_loses_the_output_and_the_thresholds() {
        let (p, losses) = convert(&with_mapping(Mode::Switch, "r4", "screenshot"), Mode::XInput);
        assert_eq!(
            losses,
            [
                ConversionLoss::OutputDisabled {
                    source: "r4".to_owned(),
                    target: "screenshot".to_owned()
                },
                ConversionLoss::TriggersReset,
            ]
        );
        assert!(p.button_mappings.iter().any(|m| m.source == "r4" && m.target == DISABLED_OUTPUT));
    }

    #[test]
    fn an_unrecognised_entry_into_switch_is_disabled() {
        let source = with_mapping(Mode::DInput, "d-pad left", UNRECOGNISED_OUTPUT);
        let (_, losses) = convert(&source, Mode::Switch);
        assert_eq!(
            losses,
            [
                ConversionLoss::OutputDisabled {
                    source: "d-pad left".to_owned(),
                    target: UNRECOGNISED_OUTPUT.to_owned()
                },
                ConversionLoss::TriggersReset,
            ]
        );
        let (same, losses) = convert(&source, Mode::DInput);
        assert!(losses.is_empty(), "same mode keeps the entry");
        assert_eq!(same, source);
    }
}
