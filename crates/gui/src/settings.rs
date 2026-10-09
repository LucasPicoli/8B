//! The settings tabs after Buttons. The controller description declares each tab, and
//! names each value by its JSON pointer into the profile, such as `/sticks/left_min_pct`,
//! with its range.

use std::rc::Rc;

use controller_core::description as described;
use controller_core::model::{CanonicalProfile, Mode};
use serde_json::Value;
use slint::{Model as _, ModelRc, VecModel};

use crate::state::AppState;
use crate::ui::{AppWindow, Setting, SettingFrame, SettingsPage, Toggle};

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
    /// Sets the number at `pointer` in the selected slot, rounded and kept inside the
    /// range the description gives it. A pointer the slot's tabs do not show changes
    /// nothing.
    pub fn set_number(&mut self, pointer: &str, value: f32) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some(range) =
            self.description().number(mode, pointer).map(described::NumberField::range)
        else {
            return;
        };
        let value = f64::from(value).round().clamp(f64::from(range.min), f64::from(range.max));
        // In range after the clamp.
        #[allow(clippy::cast_possible_truncation)]
        let value = value as i32;
        self.edit(|p| patch(p, pointer, value.into()));
    }

    /// Sets the flag at `pointer` in the selected slot. A pointer that names no flag
    /// changes nothing. Turning a flag on turns off the flags the description says may
    /// not be on with it, such as the D-pad swap and swap sticks on a Pro 3.
    pub fn set_flag(&mut self, pointer: &str, on: bool) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let description = self.description();
        if description.flag(mode, pointer).is_none() {
            return;
        }
        let excluded = description.excluded_by(mode, pointer);
        self.edit(|p| {
            patch(p, pointer, on.into());
            if on {
                for other in &excluded {
                    patch(p, other, false.into());
                }
            }
        });
    }
}

/// The number at `pointer` in `json`.
fn number(json: &Value, pointer: &str) -> Option<i64> {
    json.pointer(pointer).and_then(Value::as_i64)
}

/// `low`, or `low` to `high`, with the slider's unit. A single value with no unit shows
/// as `<low> of <max>`.
fn shown_value(slider: &described::Slider, low: i64, high: Option<i64>) -> String {
    let unit = &slider.unit;
    match high {
        Some(high) => format!("{low} to {high}{unit}"),
        None if unit.is_empty() => format!("{low} of {}", slider.low.max),
        None => format!("{low}{unit}"),
    }
}

/// `spec` filled from `shown`, the profile on screen, with what differs from `pad`,
/// what the controller holds.
// The values are profile percentages and levels, far inside f32's exact range.
#[allow(clippy::cast_precision_loss)]
fn page(spec: &described::SettingsPage, shown: &Value, pad: Option<&Value>) -> SettingsPage {
    let frames = spec.frames.iter().map(|frame| {
        let settings = frame.sliders.iter().filter_map(|s| {
            let values = |json: &Value| -> Option<(i64, Option<i64>)> {
                let low = number(json, &s.low.field)?;
                let high = match &s.high {
                    Some(h) => Some(number(json, &h.field)?),
                    None => None,
                };
                Some((low, high))
            };
            let (low, high) = values(shown)?;
            let was = pad
                .and_then(values)
                .filter(|&old| old != (low, high))
                .map(|(l, h)| format!("was {}", shown_value(s, l, h)))
                .unwrap_or_default();
            Some(Setting {
                label: s.label.as_str().into(),
                low_field: s.low.field.as_str().into(),
                high_field: s.high.as_ref().map_or("", |h| h.field.as_str()).into(),
                low: low as f32,
                high: high.unwrap_or(low) as f32,
                minimum: s.low.min as f32,
                maximum: s.high.as_ref().unwrap_or(&s.low).max as f32,
                text: shown_value(s, low, high).into(),
                was: was.into(),
            })
        });
        let toggles = frame.flags.iter().filter_map(|flag| {
            let pointer = flag.field.as_str();
            let on = shown.pointer(pointer)?.as_bool()?;
            Some(Toggle {
                label: flag.label.as_str().into(),
                field: pointer.into(),
                on,
                changed: pad.and_then(|p| p.pointer(pointer)?.as_bool()).is_some_and(|o| o != on),
            })
        });
        SettingFrame {
            title: frame.title.as_str().into(),
            settings: ModelRc::from(Rc::new(settings.collect::<VecModel<_>>())),
            toggles: ModelRc::from(Rc::new(toggles.collect::<VecModel<_>>())),
        }
    });
    SettingsPage {
        lead: spec.lead.as_str().into(),
        frames: ModelRc::from(Rc::new(frames.collect::<VecModel<_>>())),
    }
}

/// The settings tabs of the selected slot, in the description's order. Empty for an
/// empty slot.
#[must_use]
pub fn pages(state: &AppState) -> Vec<SettingsPage> {
    state.selected_slot().map(|slot| pages_of(state, slot)).unwrap_or_default()
}

/// The settings tabs of slot `(mode, number)`. Empty for an empty slot.
#[must_use]
pub fn pages_of(state: &AppState, (mode, number): (Mode, u8)) -> Vec<SettingsPage> {
    let slot = state.slot(mode, number);
    let Some(shown) = slot.shown() else { return Vec::new() };
    let Ok(json) = serde_json::to_value(shown) else { return Vec::new() };
    let pad = slot.pad.as_ref().and_then(|p| serde_json::to_value(p).ok());
    state.description().pages(mode).map(|spec| page(spec, &json, pad.as_ref())).collect()
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

/// Pushes the settings tabs, in place where their shape holds.
pub fn render_settings(state: &AppState, ui: &AppWindow) {
    let new = pages(state);
    // Another model can have fewer tabs: fall back to Buttons. An empty slot has no pages
    // but keeps its tabs, so count the tabs the description gives the mode.
    let tabs = state.selected_slot().map_or(0, |(mode, _)| state.description().pages(mode).count());
    if usize::try_from(ui.get_tab()).is_ok_and(|tab| tab > tabs) {
        ui.set_tab(0);
    }
    let old = ui.get_pages();
    let same = old.row_count() == new.len() && old.iter().zip(&new).all(|(o, n)| update(&o, n));
    if !same {
        ui.set_pages(ModelRc::from(Rc::new(VecModel::from(new))));
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::settings::{Settings, Sticks, Triggers};
    use controller_core::devices::pro3::{SWITCH, XINPUT};

    use super::*;
    use crate::state::tests::connected;
    use crate::state::Dirty;

    fn sticks(s: &AppState) -> Sticks {
        Settings::of(s.slot(XINPUT, 1).shown().unwrap()).unwrap().sticks
    }

    #[test]
    fn numbers_are_rounded_and_kept_in_the_limits() {
        let mut s = connected(XINPUT);
        s.set_number("/sticks/left_min_pct", 95.0);
        assert_eq!(sticks(&s).left_min_pct, 90, "dead zone tops out at 90");
        s.set_number("/sticks/left_max_pct", 2.0);
        assert_eq!(sticks(&s).left_max_pct, 10, "outer range starts at 10");
        s.set_number("/sticks/right_min_pct", 12.6);
        assert_eq!(sticks(&s).right_min_pct, 13);
        s.set_number("/vibration/left_level", 9.0);
        let level = |s: &AppState, side: &str| {
            s.slot(XINPUT, 1).shown().unwrap().setting(&format!("/vibration/{side}_level")).cloned()
        };
        assert_eq!(level(&s, "left"), Some(5.into()));
        s.set_number("/vibration/right_level", -3.0);
        assert_eq!(level(&s, "right"), Some(0.into()));
    }

    #[test]
    fn a_threshold_is_kept_in_its_own_limit() {
        let mut s = connected(XINPUT);
        s.select(1, 0);
        let default = s.defaults()[&SWITCH].clone();
        let slot = s.active_mut().unwrap().slots.get_mut(&(SWITCH, 1)).unwrap();
        slot.pad = Some(default);
        s.set_number("/triggers/left_threshold_pct", 100.0);
        let triggers = Settings::of(&s.slot(SWITCH, 1).edited.unwrap()).unwrap().triggers;
        assert!(matches!(triggers, Triggers::Switch(t) if t.left_threshold_pct == 90));
    }

    #[test]
    fn unknown_pointers_and_wrong_types_change_nothing() {
        let mut s = connected(XINPUT);
        let before = s.slot(XINPUT, 1).shown().unwrap().clone();
        s.set_number("/sticks/invert_left_x", 1.0);
        s.set_number("/name", 1.0);
        s.set_flag("/sticks/left_min_pct", true);
        s.set_flag("/triggers/left_threshold_pct", true);
        s.set_number("/triggers/left_threshold_pct", 50.0);
        assert_eq!(s.slot(XINPUT, 1).shown().unwrap(), &before);
    }

    #[test]
    fn the_dpad_swap_and_the_left_stick_flags_turn_each_other_off() {
        let mut s = connected(XINPUT);
        s.set_flag("/sticks/swap_dpad_with_left_stick", true);
        s.set_flag("/sticks/swap_sticks", true);
        let st = sticks(&s);
        assert!(st.swap_sticks && !st.swap_dpad_with_left_stick);
        s.set_flag("/sticks/invert_left_x", true);
        s.set_flag("/sticks/invert_left_y", true);
        s.set_flag("/sticks/invert_right_x", true);
        s.set_flag("/sticks/swap_dpad_with_left_stick", true);
        let st = sticks(&s);
        assert!(st.swap_dpad_with_left_stick);
        assert!(!st.swap_sticks && !st.invert_left_x && !st.invert_left_y);
        assert!(st.invert_right_x, "the right inverts have no rule");
        s.set_flag("/sticks/invert_left_y", true);
        assert!(!sticks(&s).swap_dpad_with_left_stick);
        s.set_flag("/sticks/swap_sticks", false);
        assert!(sticks(&s).invert_left_y, "turning a flag off touches no other");
    }

    #[test]
    fn each_tab_sets_only_its_own_dirty_flag() {
        let tab = |id: &str| Dirty { tabs: [id.to_owned()].into(), ..Dirty::default() };
        let cases = [
            ("/sticks/left_min_pct", "sticks"),
            ("/triggers/right_max_pct", "triggers"),
            ("/vibration/left_level", "vibration"),
        ];
        for (pointer, id) in cases {
            let mut s = connected(XINPUT);
            s.set_number(pointer, 1.0);
            assert_eq!(s.slot_dirty(XINPUT, 1), tab(id), "{pointer}");
        }
        let mut s = connected(XINPUT);
        s.set_flag("/triggers/swap_triggers", true);
        assert_eq!(s.slot_dirty(XINPUT, 1), tab("triggers"));
        s.set_flag("/triggers/swap_triggers", false);
        assert_eq!(s.slot_dirty(XINPUT, 1), Dirty::default(), "back to the pad");
    }

    #[test]
    fn pages_follow_the_trigger_form_and_show_what_changed() {
        let mut s = connected(XINPUT);
        let tabs = pages(&s);
        let (st, tr, vi) = (&tabs[0], &tabs[1], &tabs[2]);
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
        let frame = pages(&s)[0].frames.row_data(0).unwrap();
        let left = frame.settings.row_data(0).unwrap();
        assert_eq!(left.was, format!("was {old}"));
        assert!(left.text.starts_with("40 to "));
        let toggles: Vec<_> = frame.toggles.iter().map(|t| (t.on, t.changed)).collect();
        assert_eq!(toggles, [(false, false), (true, true)]);

        s.select(1, 2);
        s.start_from_default();
        let point = pages(&s)[1].frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert_eq!((point.high_field.as_str(), point.maximum), ("", 90.0));
        assert_eq!(point.was, "", "an empty slot has nothing to compare with");
    }

    #[test]
    fn an_update_keeps_the_models_when_the_shape_holds() {
        let mut s = connected(XINPUT);
        let old = pages(&s).remove(0);
        s.set_number("/sticks/left_min_pct", 40.0);
        let new = pages(&s).remove(0);
        assert!(update(&old, &new));
        let left = old.frames.row_data(0).unwrap().settings.row_data(0).unwrap();
        assert!((left.low - 40.0).abs() < f32::EPSILON);
        let analog = pages(&s).remove(1);
        s.select(1, 2);
        s.start_from_default();
        let threshold = pages(&s).remove(1);
        assert!(!update(&analog, &threshold), "another lead");
    }
}
