//! Converts a profile made for one mode into another mode, for an import from a file.
//!
//! Buttons map by position, so a mapping keeps its canonical names and only the
//! printed labels differ. A button still at its default takes the target mode's
//! default (Switch turbo takes a screenshot, `XInput` turbo is turbo). Two things can
//! be lost: an output the target mode lacks becomes [`DISABLED_OUTPUT`], and a settings
//! group whose declared fields or ranges differ in the target mode, such as the Pro 3's
//! triggers between `XInput` and Switch, starts from the target mode's defaults. Each
//! loss is listed for the import warning. A group the target mode lacks is dropped.

use std::collections::{BTreeMap, BTreeSet};

use crate::description::{ControllerDescription, SettingsPage, DISABLED_OUTPUT};
use crate::error::{Error, Result};
use crate::model::CanonicalProfile;
use crate::model::Mode;

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
    /// The target mode declares other fields for this settings group, such as
    /// `triggers`, so the group starts from the target mode's defaults.
    SettingsReset {
        /// The group's name in the profile.
        group: String,
    },
}

/// One declared field: its pointer, and its range if it is a number.
type Field<'a> = (&'a str, Option<(i32, i32)>);

/// The fields `description` declares for `mode`, by settings group.
fn groups(description: &ControllerDescription, mode: Mode) -> BTreeMap<&str, BTreeSet<Field<'_>>> {
    let mut groups: BTreeMap<&str, BTreeSet<Field<'_>>> = BTreeMap::new();
    for field in description.pages(mode).flat_map(SettingsPage::fields) {
        let group = field.trim_start_matches('/').split('/').next().unwrap_or_default();
        let range = description.number(mode, field).map(|n| (n.min, n.max));
        groups.entry(group).or_default().insert((field, range));
    }
    groups
}

/// Converts `profile` into the mode of `target_default`.
///
/// `target_default` is the profile a new slot of that mode starts from, and
/// `source_default` is the same for `profile`'s mode. A profile already in the
/// target mode comes back unchanged.
///
/// # Errors
/// Returns [`Error::Validation`] if `description` has no entry for the target mode.
pub fn convert_profile(
    description: &ControllerDescription,
    profile: &CanonicalProfile,
    source_default: &CanonicalProfile,
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
        if default_target(source_default, &mapping.source) == Some(mapping.target.as_str()) {
            if let Some(target) = default_target(target_default, &mapping.source) {
                target.clone_into(&mut mapping.target);
                continue;
            }
        }
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
    let from = groups(description, profile.mode);
    let target = groups(description, to);
    // A group the target mode does not declare has no setting there to keep.
    converted.settings.retain(|group, _| target.contains_key(group.as_str()));
    for (group, fields) in target {
        if from.get(group) == Some(&fields) {
            continue;
        }
        if let Some(default) = target_default.settings.get(group) {
            converted.settings.insert(group.to_owned(), default.clone());
            losses.push(ConversionLoss::SettingsReset { group: group.to_owned() });
        }
    }
    Ok((converted, losses))
}

/// The output `source` has in `default`.
fn default_target<'a>(default: &'a CanonicalProfile, source: &str) -> Option<&'a str> {
    default.button_mappings.iter().find(|m| m.source == source).map(|m| m.target.as_str())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::description::UNRECOGNISED_OUTPUT;
    use crate::device::{ControllerSpec, ProtocolCodec};
    use crate::devices::pro3::{Pro3, DINPUT, MODES, SWITCH, XINPUT};

    fn triggers_reset() -> ConversionLoss {
        ConversionLoss::SettingsReset { group: "triggers".to_owned() }
    }
    use crate::model::{ButtonMapping, Mode};

    fn convert(profile: &CanonicalProfile, to: Mode) -> (CanonicalProfile, Vec<ConversionLoss>) {
        let d = Pro3.description().unwrap();
        let from = Pro3.default_profile(profile.mode);
        convert_profile(d, profile, &from, &Pro3.default_profile(to)).unwrap()
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
        for from in MODES {
            for to in MODES {
                let source = with_mapping(from, "r4", "bottom face");
                let (p, losses) = convert(&source, to);
                assert_eq!(p.mode, to, "{from} to {to}");
                // Only Switch tunes the triggers by a threshold.
                let reset = (from == SWITCH) != (to == SWITCH);
                assert_eq!(
                    losses,
                    if reset { vec![triggers_reset()] } else { vec![] },
                    "{from} to {to}"
                );
                let default = Pro3.default_profile(to);
                let expected = if reset { &default } else { &source }.settings.get("triggers");
                assert_eq!(p.settings.get("triggers"), expected, "{from} to {to}");
                // Every other button stays at its default, so it takes the new mode's.
                let want = with_mapping(to, "r4", "bottom face").button_mappings;
                assert_eq!(p.button_mappings, want, "{from} to {to}");
                let kept = |q: &CanonicalProfile| {
                    (q.settings.get("sticks").cloned(), q.settings.get("vibration").cloned())
                };
                assert_eq!((p.name.as_str(), kept(&p)), ("Mine", kept(&source)));
            }
        }
    }

    #[test]
    fn xinput_and_dinput_round_trip_unchanged() {
        let source = with_mapping(XINPUT, "lp", "right face");
        let (dinput, losses) = convert(&source, DINPUT);
        assert_eq!(losses, []);
        let (back, losses) = convert(&dinput, XINPUT);
        assert_eq!(losses, []);
        assert_eq!(back, source);
    }

    #[test]
    fn switch_screenshot_into_xinput_loses_the_output_and_the_thresholds() {
        let (p, losses) = convert(&with_mapping(SWITCH, "r4", "screenshot"), XINPUT);
        assert_eq!(
            losses,
            [
                ConversionLoss::OutputDisabled {
                    source: "r4".to_owned(),
                    target: "screenshot".to_owned()
                },
                triggers_reset(),
            ]
        );
        assert!(p.button_mappings.iter().any(|m| m.source == "r4" && m.target == DISABLED_OUTPUT));
    }

    #[test]
    fn an_unrecognised_entry_into_switch_is_disabled() {
        let source = with_mapping(DINPUT, "d-pad left", UNRECOGNISED_OUTPUT);
        let (_, losses) = convert(&source, SWITCH);
        assert_eq!(
            losses,
            [
                ConversionLoss::OutputDisabled {
                    source: "d-pad left".to_owned(),
                    target: UNRECOGNISED_OUTPUT.to_owned()
                },
                triggers_reset(),
            ]
        );
        let (same, losses) = convert(&source, DINPUT);
        assert!(losses.is_empty(), "same mode keeps the entry");
        assert_eq!(same, source);
    }

    #[test]
    fn a_group_the_target_lacks_goes_and_a_narrower_range_resets() {
        let mut d = Pro3.description().unwrap().clone();
        let vibration = d.settings.iter().position(|p| p.id == "vibration").unwrap();
        d.settings[vibration].modes = vec![XINPUT, DINPUT];
        let source = Pro3.default_profile(XINPUT);
        let switch = Pro3.default_profile(SWITCH);
        let (p, losses) = convert_profile(&d, &source, &source, &switch).unwrap();
        assert!(p.settings.get("vibration").is_none(), "Switch has no vibration here");
        assert_eq!(losses, [triggers_reset()]);

        // Switch gets its own vibration tab with a narrower range.
        let mut narrow = d.settings[vibration].clone();
        narrow.modes = vec![SWITCH];
        for slider in narrow.frames.iter_mut().flat_map(|f| &mut f.sliders) {
            slider.low.max = 3;
        }
        d.settings.push(narrow);
        let (p, losses) = convert_profile(&d, &source, &source, &switch).unwrap();
        let reset = ConversionLoss::SettingsReset { group: "vibration".to_owned() };
        assert!(losses.contains(&reset), "{losses:?}");
        assert_eq!(p.settings.get("vibration"), switch.settings.get("vibration"));
    }
}
