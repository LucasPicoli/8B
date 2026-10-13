//! The macro stick floor: a macro's stick move that does not play under a low dead zone.
//!
//! Some pads drop a macro's stick moves in some modes when that stick's dead zone low
//! end is under a floor. The description gives the floor per mode
//! ([`ModeDescription::macro_stick_floor`](crate::description::ModeDescription)) and
//! names the stick each dead zone slider governs ([`Slider::macro_stick`]). The write
//! still goes ahead: this only warns.

use serde_json::Value;

use crate::description::{ControllerDescription, MacroStick};
use crate::model::macros::STICK_CENTER;
use crate::model::{MacroDefinition, MacroStep, Mode};

/// A stick whose dead zone low end is under its mode's floor, from [`low_sticks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LowStick {
    /// The stick.
    pub stick: MacroStick,
    /// The title of the slider's frame, such as `Left stick`.
    pub name: String,
    /// The low end, in percent.
    pub low: i64,
}

/// The sticks whose dead zone low end in `profile` is under the macro stick floor of
/// `mode`. Empty when `mode` has no floor.
///
/// `profile` is a profile's JSON, read at each slider's `low` pointer. A slider whose
/// value is missing is skipped.
#[must_use]
pub fn low_sticks(
    description: &ControllerDescription,
    mode: Mode,
    profile: &Value,
) -> Vec<LowStick> {
    let Some(floor) = floor(description, mode) else { return Vec::new() };
    let mut low = Vec::new();
    for frame in description.pages(mode).flat_map(|p| &p.frames) {
        for slider in &frame.sliders {
            let Some(stick) = slider.macro_stick else { continue };
            let value = profile.pointer(&slider.low.field).and_then(Value::as_i64);
            if let Some(value) = value.filter(|&v| v < i64::from(floor)) {
                low.push(LowStick { stick, name: frame.title.clone(), low: value });
            }
        }
    }
    low
}

/// One warning per macro of `macros` and stick it moves whose dead zone low end in
/// `profile` is under the floor of `mode`, see [`low_sticks`].
#[must_use]
pub fn macro_stick_warnings(
    description: &ControllerDescription,
    mode: Mode,
    profile: &Value,
    macros: &[MacroDefinition],
) -> Vec<String> {
    let Some(floor) = floor(description, mode) else { return Vec::new() };
    let label = description.mode_label(mode);
    let mut warnings = Vec::new();
    for LowStick { stick, name, low } in low_sticks(description, mode, profile) {
        for m in macros.iter().filter(|m| m.steps.iter().any(|s| moves(s, stick))) {
            warnings.push(format!(
                "Macro '{}' moves the {}, but its dead zone low end is {low} %. In {label}, a \
                 macro cannot move a stick whose low end is below {floor} %.",
                m.name,
                name.to_lowercase()
            ));
        }
    }
    warnings
}

/// The macro stick floor of `mode`, if it has one.
fn floor(description: &ControllerDescription, mode: Mode) -> Option<i32> {
    description.mode(mode).and_then(|m| m.macro_stick_floor)
}

/// Whether `step` holds `stick` away from centre.
const fn moves(step: &MacroStep, stick: MacroStick) -> bool {
    let (x, y) = match stick {
        MacroStick::LeftStick => (step.left_stick_x, step.left_stick_y),
        MacroStick::RightStick => (step.right_stick_x, step.right_stick_y),
    };
    x != STICK_CENTER || y != STICK_CENTER
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::device::ControllerSpec as _;
    use crate::devices::pro3::{Pro3, DINPUT, SWITCH, XINPUT};
    use crate::devices::test_pad::TestPad;

    /// No warnings.
    const NONE: [String; 0] = [];

    fn left_stick_macro() -> MacroDefinition {
        let step = MacroStep { left_stick_x: 255, ..MacroStep::default() };
        MacroDefinition {
            name: "Dash".to_owned(),
            mode: XINPUT,
            trigger_buttons: vec!["l4".to_owned()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![MacroStep::default(), step],
            macro_slot: Some(0),
        }
    }

    fn profile(left: i64, right: i64) -> Value {
        json!({ "sticks": { "left_min_pct": left, "right_min_pct": right } })
    }

    #[test]
    fn a_moved_stick_under_the_pro3_floor_warns_in_xinput_and_switch() {
        let description = Pro3.description().unwrap();
        let macros = [left_stick_macro()];
        for mode in [XINPUT, SWITCH] {
            let warnings = macro_stick_warnings(description, mode, &profile(1, 0), &macros);
            assert_eq!(warnings.len(), 1, "{warnings:?}");
            assert!(warnings[0].contains("'Dash' moves the left stick"), "{}", warnings[0]);
            assert!(warnings[0].contains("is 1 %"), "{}", warnings[0]);
            assert!(warnings[0].contains("below 2 %"), "{}", warnings[0]);
        }
        assert_eq!(macro_stick_warnings(description, DINPUT, &profile(0, 0), &macros), NONE);
    }

    #[test]
    fn the_floor_itself_and_an_unmoved_stick_do_not_warn() {
        let description = Pro3.description().unwrap();
        let macros = [left_stick_macro()];
        assert_eq!(macro_stick_warnings(description, XINPUT, &profile(2, 0), &macros), NONE);
        let still = MacroDefinition { steps: vec![MacroStep::default()], ..left_stick_macro() };
        assert_eq!(macro_stick_warnings(description, XINPUT, &profile(0, 0), &[still]), NONE);
    }

    #[test]
    fn a_description_with_no_floor_never_warns() {
        let description = TestPad.description().unwrap();
        let macros = [left_stick_macro()];
        for mode in description.modes.iter().map(|m| m.id) {
            assert_eq!(macro_stick_warnings(description, mode, &profile(0, 0), &macros), NONE);
        }
    }
}
