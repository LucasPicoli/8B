//! The Sticks, Triggers and Vibration tabs. Each value is named by its JSON pointer
//! into the profile, as in `schemas/profile-v1.schema.json`, and kept inside the
//! controller description's limits.

use std::rc::Rc;

use controller_core::description::{LimitRange, Limits};
use controller_core::model::{CanonicalProfile, Triggers};
use serde_json::Value;
use slint::{Model as _, ModelRc, VecModel};

use crate::state::AppState;
use crate::ui::{AppWindow, Setting, SettingFrame, SettingsPage, Toggle};

/// A slider: one value, or the low and high ends of one range.
struct SliderSpec {
    label: &'static str,
    low: &'static str,
    high: Option<&'static str>,
}

/// One frame on a tab: its sliders, then its check boxes as (label, pointer).
struct FrameSpec {
    title: &'static str,
    sliders: &'static [SliderSpec],
    flags: &'static [(&'static str, &'static str)],
}

/// One tab: the line above the frames, and the frames.
struct PageSpec {
    lead: &'static str,
    frames: &'static [FrameSpec],
}

const fn range(label: &'static str, low: &'static str, high: &'static str) -> SliderSpec {
    SliderSpec { label, low, high: Some(high) }
}

const fn single(label: &'static str, value: &'static str) -> SliderSpec {
    SliderSpec { label, low: value, high: None }
}

const STICKS: PageSpec = PageSpec {
    lead: "Inside the low end of the dead zone the stick reads as centred. Past the high end it reads as pushed all the way.",
    frames: &[
        FrameSpec {
            title: "Left stick",
            sliders: &[range("Dead zone", "/sticks/left_min_pct", "/sticks/left_max_pct")],
            flags: &[("Invert X", "/sticks/invert_left_x"), ("Invert Y", "/sticks/invert_left_y")],
        },
        FrameSpec {
            title: "Right stick",
            sliders: &[range("Dead zone", "/sticks/right_min_pct", "/sticks/right_max_pct")],
            flags: &[("Invert X", "/sticks/invert_right_x"), ("Invert Y", "/sticks/invert_right_y")],
        },
        FrameSpec {
            title: "Swaps",
            sliders: &[],
            flags: &[
                ("Swap the left and right sticks", "/sticks/swap_sticks"),
                ("Swap the D-pad and the left stick", "/sticks/swap_dpad_with_left_stick"),
            ],
        },
    ],
};

/// The swap shared by both trigger forms.
const TRIGGER_SWAP: FrameSpec = FrameSpec {
    title: "Swaps",
    sliders: &[],
    flags: &[("Swap the left and right triggers", "/triggers/swap_triggers")],
};

const ANALOG_TRIGGERS: PageSpec = PageSpec {
    lead: "Below the low end of the range the trigger reads as released. Past the high end it reads as pulled all the way.",
    frames: &[
        FrameSpec {
            title: "Left trigger",
            sliders: &[range("Range", "/triggers/left_min_pct", "/triggers/left_max_pct")],
            flags: &[],
        },
        FrameSpec {
            title: "Right trigger",
            sliders: &[range("Range", "/triggers/right_min_pct", "/triggers/right_max_pct")],
            flags: &[],
        },
        TRIGGER_SWAP,
    ],
};

const THRESHOLD_TRIGGERS: PageSpec = PageSpec {
    lead: "In this mode each trigger is a button. It presses once pulled past the press point.",
    frames: &[
        FrameSpec {
            title: "Left trigger",
            sliders: &[single("Press point", "/triggers/left_threshold_pct")],
            flags: &[],
        },
        FrameSpec {
            title: "Right trigger",
            sliders: &[single("Press point", "/triggers/right_threshold_pct")],
            flags: &[],
        },
        TRIGGER_SWAP,
    ],
};

const VIBRATION: PageSpec = PageSpec {
    lead: "How strongly each motor rumbles. Level 0 turns it off.",
    frames: &[FrameSpec {
        title: "Motors",
        sliders: &[
            single("Left motor", "/vibration/left_level"),
            single("Right motor", "/vibration/right_level"),
        ],
        flags: &[],
    }],
};

/// The limits of the number at `pointer`. `None` for anything the tabs do not edit.
fn limit(limits: &Limits, pointer: &str) -> Option<LimitRange> {
    Some(match pointer {
        "/sticks/left_min_pct" | "/sticks/right_min_pct" => limits.stick_min_pct,
        "/sticks/left_max_pct" | "/sticks/right_max_pct" => limits.stick_max_pct,
        "/triggers/left_min_pct"
        | "/triggers/left_max_pct"
        | "/triggers/right_min_pct"
        | "/triggers/right_max_pct" => limits.trigger_pct,
        "/triggers/left_threshold_pct" | "/triggers/right_threshold_pct" => {
            limits.trigger_threshold_pct
        }
        "/vibration/left_level" | "/vibration/right_level" => limits.vibration_level,
        _ => return None,
    })
}

/// Replaces the value at `pointer` in `profile` with `new`, when the old value is of
/// the same JSON type.
fn patch(profile: &mut CanonicalProfile, pointer: &str, new: Value) {
    let Ok(mut json) = serde_json::to_value(&*profile) else { return };
    let Some(old) = json.pointer_mut(pointer) else { return };
    if std::mem::discriminant(old) != std::mem::discriminant(&new) {
        return;
    }
    *old = new;
    if let Ok(p) = serde_json::from_value(json) {
        *profile = p;
    }
}

impl AppState {
    /// Sets the number at `pointer` in the selected slot, rounded and kept inside
    /// the description's limits. A pointer the tabs do not edit changes nothing.
    pub fn set_number(&mut self, pointer: &str, value: f32) {
        let Some(range) = limit(&self.description.limits, pointer) else { return };
        let value = f64::from(value).round().clamp(f64::from(range.min), f64::from(range.max));
        // In range after the clamp.
        #[allow(clippy::cast_possible_truncation)]
        let value = value as i32;
        self.edit(|p| patch(p, pointer, value.into()));
    }

    /// Sets the flag at `pointer` in the selected slot. A pointer that names no flag
    /// changes nothing.
    pub fn set_flag(&mut self, pointer: &str, on: bool) {
        self.edit(|p| patch(p, pointer, on.into()));
    }
}

/// The number at `pointer` in `json`.
fn number(json: &Value, pointer: &str) -> Option<i64> {
    json.pointer(pointer).and_then(Value::as_i64)
}

/// `low`, or `low` to `high`, with its unit.
fn shown_value(pointer: &str, low: i64, high: Option<i64>, range: LimitRange) -> String {
    match high {
        Some(high) => format!("{low} to {high}%"),
        None if pointer.ends_with("_pct") => format!("{low}%"),
        None => format!("{low} of {}", range.max),
    }
}

/// `spec` filled from `shown`, the profile on screen, with what differs from `pad`,
/// what the controller holds.
// The values are profile percentages and levels, far inside f32's exact range.
#[allow(clippy::cast_precision_loss)]
fn page(limits: &Limits, spec: &PageSpec, shown: &Value, pad: Option<&Value>) -> SettingsPage {
    let frames = spec.frames.iter().map(|frame| {
        let settings = frame.sliders.iter().filter_map(|s| {
            let low_range = limit(limits, s.low)?;
            let high_range = s.high.map_or(Some(low_range), |h| limit(limits, h))?;
            let values = |json: &Value| -> Option<(i64, Option<i64>)> {
                let low = number(json, s.low)?;
                let high = match s.high {
                    Some(h) => Some(number(json, h)?),
                    None => None,
                };
                Some((low, high))
            };
            let (low, high) = values(shown)?;
            let was = pad
                .and_then(values)
                .filter(|&old| old != (low, high))
                .map(|(l, h)| format!("was {}", shown_value(s.low, l, h, low_range)))
                .unwrap_or_default();
            Some(Setting {
                label: s.label.into(),
                low_field: s.low.into(),
                high_field: s.high.unwrap_or_default().into(),
                low: low as f32,
                high: high.unwrap_or(low) as f32,
                minimum: low_range.min as f32,
                maximum: high_range.max as f32,
                text: shown_value(s.low, low, high, low_range).into(),
                was: was.into(),
            })
        });
        let toggles = frame.flags.iter().filter_map(|&(label, pointer)| {
            let on = shown.pointer(pointer)?.as_bool()?;
            Some(Toggle {
                label: label.into(),
                field: pointer.into(),
                on,
                changed: pad.and_then(|p| p.pointer(pointer)?.as_bool()).is_some_and(|o| o != on),
            })
        });
        SettingFrame {
            title: frame.title.into(),
            settings: ModelRc::from(Rc::new(settings.collect::<VecModel<_>>())),
            toggles: ModelRc::from(Rc::new(toggles.collect::<VecModel<_>>())),
        }
    });
    SettingsPage {
        lead: spec.lead.into(),
        frames: ModelRc::from(Rc::new(frames.collect::<VecModel<_>>())),
    }
}

/// The Sticks, Triggers and Vibration tabs of the selected slot. Empty for an
/// empty slot.
#[must_use]
pub fn pages(state: &AppState) -> [SettingsPage; 3] {
    let Some((mode, number)) = state.selected_slot() else { return Default::default() };
    let slot = state.slot(mode, number);
    let Some(shown) = slot.shown() else { return Default::default() };
    let Ok(json) = serde_json::to_value(shown) else { return Default::default() };
    let pad = slot.pad.as_ref().and_then(|p| serde_json::to_value(p).ok());
    let triggers = match shown.triggers {
        Triggers::Analog(_) => &ANALOG_TRIGGERS,
        Triggers::Switch(_) => &THRESHOLD_TRIGGERS,
    };
    let limits = &state.description.limits;
    [&STICKS, triggers, &VIBRATION].map(|spec| page(limits, spec, &json, pad.as_ref()))
}

/// Copies `new` into the models `old` already shows, when both have the same frames
/// with the same row counts, so a slider being dragged keeps its drag. False when
/// the shape differs and nothing was copied.
fn update(old: &SettingsPage, new: &SettingsPage) -> bool {
    let same = old.lead == new.lead
        && old.frames.row_count() == new.frames.row_count()
        && old.frames.iter().zip(new.frames.iter()).all(|(o, n)| {
            o.title == n.title
                && o.settings.row_count() == n.settings.row_count()
                && o.toggles.row_count() == n.toggles.row_count()
        });
    if same {
        for (o, n) in old.frames.iter().zip(new.frames.iter()) {
            copy_rows(&o.settings, &n.settings);
            copy_rows(&o.toggles, &n.toggles);
        }
    }
    same
}

/// Sets each row of `old` that differs from the same row of `new`.
fn copy_rows<T: Clone + PartialEq + 'static>(old: &ModelRc<T>, new: &ModelRc<T>) {
    for (i, row) in new.iter().enumerate() {
        if old.row_data(i).as_ref() != Some(&row) {
            old.set_row_data(i, row);
        }
    }
}

/// Pushes the three settings tabs, in place where their shape holds.
pub fn render_settings(state: &AppState, ui: &AppWindow) {
    let [sticks, triggers, vibration] = pages(state);
    if !update(&ui.get_sticks(), &sticks) {
        ui.set_sticks(sticks);
    }
    if !update(&ui.get_triggers(), &triggers) {
        ui.set_triggers(triggers);
    }
    if !update(&ui.get_vibration(), &vibration) {
        ui.set_vibration(vibration);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::model::Mode;

    use super::*;
    use crate::state::tests::connected;
    use crate::state::Dirty;

    fn sticks(s: &AppState) -> controller_core::model::Sticks {
        s.slot(Mode::XInput, 1).shown().unwrap().sticks.clone()
    }

    #[test]
    fn numbers_are_rounded_and_kept_in_the_limits() {
        let mut s = connected(Mode::XInput);
        s.set_number("/sticks/left_min_pct", 95.0);
        assert_eq!(sticks(&s).left_min_pct, 90, "dead zone tops out at 90");
        s.set_number("/sticks/left_max_pct", 2.0);
        assert_eq!(sticks(&s).left_max_pct, 10, "outer range starts at 10");
        s.set_number("/sticks/right_min_pct", 12.6);
        assert_eq!(sticks(&s).right_min_pct, 13);
        s.set_number("/vibration/left_level", 9.0);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().vibration.left_level, 5);
        s.set_number("/vibration/right_level", -3.0);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().vibration.right_level, 0);
    }

    #[test]
    fn a_threshold_is_kept_in_its_own_limit() {
        let mut s = connected(Mode::XInput);
        s.select(1, 0);
        let default = s.defaults[&Mode::Switch].clone();
        let slot = s.active_mut().unwrap().slots.get_mut(&(Mode::Switch, 1)).unwrap();
        slot.pad = Some(default);
        s.set_number("/triggers/left_threshold_pct", 100.0);
        let triggers = s.slot(Mode::Switch, 1).edited.unwrap().triggers;
        assert!(matches!(triggers, Triggers::Switch(t) if t.left_threshold_pct == 90));
    }

    #[test]
    fn unknown_pointers_and_wrong_types_change_nothing() {
        let mut s = connected(Mode::XInput);
        let before = s.slot(Mode::XInput, 1).shown().unwrap().clone();
        s.set_number("/sticks/invert_left_x", 1.0);
        s.set_number("/name", 1.0);
        s.set_flag("/sticks/left_min_pct", true);
        s.set_flag("/triggers/left_threshold_pct", true);
        s.set_number("/triggers/left_threshold_pct", 50.0);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap(), &before);
    }

    #[test]
    fn each_tab_sets_only_its_own_dirty_flag() {
        let cases: [(&str, Dirty); 3] = [
            ("/sticks/left_min_pct", Dirty { sticks: true, ..Dirty::default() }),
            ("/triggers/right_max_pct", Dirty { triggers: true, ..Dirty::default() }),
            ("/vibration/left_level", Dirty { vibration: true, ..Dirty::default() }),
        ];
        for (pointer, dirty) in cases {
            let mut s = connected(Mode::XInput);
            s.set_number(pointer, 1.0);
            assert_eq!(s.slot(Mode::XInput, 1).dirty(), dirty, "{pointer}");
        }
        let mut s = connected(Mode::XInput);
        s.set_flag("/triggers/swap_triggers", true);
        assert_eq!(s.slot(Mode::XInput, 1).dirty(), Dirty { triggers: true, ..Dirty::default() });
        s.set_flag("/triggers/swap_triggers", false);
        assert_eq!(s.slot(Mode::XInput, 1).dirty(), Dirty::default(), "back to the pad");
    }

    #[test]
    fn pages_follow_the_trigger_form_and_show_what_changed() {
        let mut s = connected(Mode::XInput);
        let [st, tr, vi] = pages(&s);
        assert_eq!(st.frames.row_count(), 3);
        let left = st.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert_eq!((left.minimum, left.maximum), (0.0, 100.0));
        assert_eq!(left.was, "");
        let range = tr.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert_eq!(range.high_field, "/triggers/left_max_pct");
        let motor = vi.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert!(motor.text.ends_with(" of 5"), "{}", motor.text);

        let old = left.text;
        s.set_number("/sticks/left_min_pct", 40.0);
        s.set_flag("/sticks/invert_left_y", true);
        let [st, ..] = pages(&s);
        let frame = st.frames.row_data(0).unwrap();
        let left = frame.settings.row_data(0).unwrap();
        assert_eq!(left.was, format!("was {old}"));
        assert!(left.text.starts_with("40 to "));
        let toggles: Vec<_> = frame.toggles.iter().map(|t| (t.on, t.changed)).collect();
        assert_eq!(toggles, [(false, false), (true, true)]);

        s.select(1, 2);
        s.start_from_default();
        let [_, tr, _] = pages(&s);
        let point = tr.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert_eq!((point.high_field.as_str(), point.maximum), ("", 90.0));
        assert_eq!(point.was, "", "an empty slot has nothing to compare with");
    }

    #[test]
    fn an_update_keeps_the_models_when_the_shape_holds() {
        let mut s = connected(Mode::XInput);
        let [old, ..] = pages(&s);
        s.set_number("/sticks/left_min_pct", 40.0);
        let [new, ..] = pages(&s);
        assert!(update(&old, &new));
        let left = old.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert!((left.low - 40.0).abs() < f32::EPSILON);
        let [_, analog, _] = pages(&s);
        s.select(1, 2);
        s.start_from_default();
        let [_, threshold, _] = pages(&s);
        assert!(!update(&analog, &threshold), "another lead");
    }
}
