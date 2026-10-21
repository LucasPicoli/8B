//! The settings tabs after Buttons. The controller description declares each tab, and
//! names each value by its JSON pointer into the profile, such as `/sticks/left_min_pct`,
//! with its range.

use std::rc::Rc;

use controller_core::description::{self as described, firmware_allows, FirmwareVersion};
use controller_core::model::{CanonicalProfile, Mode};
use serde_json::Value;
use slint::{Model as _, ModelRc, VecModel};
use tr::tr;

use crate::language::prose;
use crate::render::and_list;
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

/// `low`, or `low` to `high`, with the slider's unit. A single value shows its label
/// when the field names its values, and otherwise, with no unit, as `<low> of <max>`.
fn shown_value(slider: &described::Slider, low: i64, high: Option<i64>) -> String {
    let unit = &slider.unit;
    match (high, slider.low.label(low)) {
        (Some(high), _) => tr!("{low} to {high}{unit}", low = low, high = high, unit = unit),
        (None, Some(label)) => prose(label),
        (None, None) if unit.is_empty() => {
            tr!("{low} of {max}", low = low, max = slider.low.max)
        }
        (None, None) => format!("{low}{unit}"),
    }
}

/// The oldest firmware that has `slider`: the slider and each of its fields may name
/// one. `None` for every firmware.
fn slider_gate(slider: &described::Slider) -> Option<FirmwareVersion> {
    [Some(&slider.low), slider.high.as_ref()]
        .into_iter()
        .flatten()
        .map(|f| f.min_firmware)
        .chain([slider.min_firmware])
        .max()
        .flatten()
}

/// Every item of `spec` with a firmware gate, as (its name, its gate), in tab order.
fn gated(spec: &described::SettingsPage) -> Vec<(String, FirmwareVersion)> {
    spec.frames
        .iter()
        .flat_map(|f| {
            let sliders = f.sliders.iter().map(|s| (&s.label, slider_gate(s)));
            sliders.chain(f.flags.iter().map(|f| (&f.label, f.min_firmware)))
        })
        .filter_map(|(label, gate)| Some((prose(label), gate?)))
        .collect()
}

/// The line under a tab that names what `firmware` hides of it. Empty when it hides
/// nothing.
fn hidden_note(spec: &described::SettingsPage, firmware: &str) -> String {
    let hidden: Vec<_> = gated(spec)
        .into_iter()
        .filter(|(_, gate)| !firmware_allows(Some(*gate), firmware))
        .collect();
    if hidden.is_empty() {
        return String::new();
    }
    if FirmwareVersion::parse(firmware).is_none() {
        let items: Vec<String> = hidden
            .iter()
            .map(|(item, gate)| tr!("{item} (firmware {min}+)", item = item, min = gate))
            .collect();
        return tr!(
            "8B cannot read the firmware version and hides: {items}.",
            items = and_list(&items)
        );
    }
    let mut gates: Vec<FirmwareVersion> = hidden.iter().map(|(_, gate)| *gate).collect();
    gates.sort_unstable();
    gates.dedup();
    gates
        .into_iter()
        .map(|gate| {
            let items: Vec<String> =
                hidden.iter().filter(|(_, g)| *g == gate).map(|(item, _)| item.clone()).collect();
            tr!("Firmware {min}+ adds: {items}.", min = gate, items = and_list(&items))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `spec` filled from `shown`, the profile on screen, with what differs from `pad`,
/// what the controller holds. An item `firmware` lacks is left out, and so is a frame
/// left with nothing; the page's note names them.
// The values are profile percentages and levels, far inside f32's exact range.
#[allow(clippy::cast_precision_loss)]
fn page(
    spec: &described::SettingsPage,
    shown: &Value,
    pad: Option<&Value>,
    firmware: &str,
) -> SettingsPage {
    let frames = spec.frames.iter().map(|frame| {
        let sliders = frame.sliders.iter().filter(|s| firmware_allows(slider_gate(s), firmware));
        let settings = sliders.filter_map(|s| {
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
                .map(|(l, h)| shown_value(s, l, h))
                .unwrap_or_default();
            Some(Setting {
                label: prose(&s.label).into(),
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
        let flags = frame.flags.iter().filter(|f| firmware_allows(f.min_firmware, firmware));
        let toggles = flags.filter_map(|flag| {
            let pointer = flag.field.as_str();
            let on = shown.pointer(pointer)?.as_bool()?;
            Some(Toggle {
                label: prose(&flag.label).into(),
                field: pointer.into(),
                on,
                changed: pad.and_then(|p| p.pointer(pointer)?.as_bool()).is_some_and(|o| o != on),
            })
        });
        let (settings, toggles): (Vec<_>, Vec<_>) = (settings.collect(), toggles.collect());
        (!settings.is_empty() || !toggles.is_empty()).then(|| SettingFrame {
            title: prose(&frame.title).into(),
            settings: ModelRc::from(Rc::new(VecModel::from(settings))),
            toggles: ModelRc::from(Rc::new(VecModel::from(toggles))),
        })
    });
    SettingsPage {
        lead: prose(&spec.lead).into(),
        note: hidden_note(spec, firmware).into(),
        frames: ModelRc::from(Rc::new(frames.flatten().collect::<VecModel<_>>())),
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
    let firmware = state.firmware();
    state.description().pages(mode).map(|spec| page(spec, &json, pad.as_ref(), firmware)).collect()
}

/// Copies `new` into the models `old` already shows, when both have the same frames
/// with the same row counts, so a slider being dragged keeps its drag. False when
/// the shape differs and nothing was copied.
fn update(old: &SettingsPage, new: &SettingsPage) -> bool {
    let same = old.lead == new.lead
        && old.note == new.note
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
    // but keeps its tabs, so count the tabs the description gives the mode. The pages
    // follow Buttons, and Macros on a model with macro slots (`first-page` in `app.slint`).
    let description = state.description();
    let first_page = 1 + usize::from(description.macro_slot_count > 0);
    let pages = state.selected_slot().map_or(0, |(mode, _)| description.pages(mode).count());
    if usize::try_from(ui.get_tab()).is_ok_and(|tab| tab >= first_page + pages) {
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
    fn a_named_value_shows_its_name() {
        let slider = |labels: serde_json::Value| -> described::Slider {
            serde_json::from_value(serde_json::json!({
                "label": "Rumble effect",
                "low": { "field": "/rumble/effect", "min": 1, "max": 3, "labels": labels },
            }))
            .unwrap()
        };
        let named = slider(serde_json::json!(["Pulse", "Wave", "Burst"]));
        assert_eq!(shown_value(&named, 2, None), "Wave");
        assert_eq!(shown_value(&named, 9, None), "9 of 3", "a value past the names");
        assert_eq!(shown_value(&slider(serde_json::json!([])), 2, None), "2 of 3");
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
        assert_eq!(left.was, old, "the old value, worded by `.slint`");
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

    /// The toggle fields of each frame of the Sticks tab of the Ultimate on `firmware`
    /// in `mode`, by frame title.
    fn stick_toggles(firmware: &str, mode: Mode) -> Vec<(String, Vec<String>)> {
        let mut s = crate::state::tests::ultimate(firmware);
        let index = s.description().modes.iter().position(|m| m.id == mode).unwrap();
        s.select(index, 0);
        let page = pages(&s).into_iter().next().unwrap();
        page.frames
            .iter()
            .map(|f| (f.title.to_string(), f.toggles.iter().map(|t| t.field.to_string()).collect()))
            .collect()
    }

    #[test]
    fn a_gated_item_shows_from_its_firmware_on() {
        use controller_core::devices::ultimate::{SWITCH, XINPUT};
        let last = |firmware, mode| stick_toggles(firmware, mode).pop().unwrap();
        let both = ["/sticks/no_dead_zone", "/sticks/dpad_four_way"].map(String::from);
        assert_eq!(last("1.09", XINPUT).1, both);
        assert_eq!(last("1.03", XINPUT).1, both);
        assert_eq!(last("1.02", XINPUT).1, ["/sticks/no_dead_zone"]);
        assert_eq!(last("", XINPUT).1, ["/sticks/no_dead_zone"], "unknown firmware");
        assert_eq!(last("1.09", SWITCH).0, "D-pad");
        assert!(
            stick_toggles("1.02", SWITCH).iter().all(|(t, _)| t != "D-pad"),
            "an empty frame goes"
        );
    }

    #[test]
    fn a_tab_names_what_the_firmware_hides() {
        use controller_core::devices::ultimate::{SWITCH, XINPUT};
        let note = |firmware: &str, mode, tab: usize| {
            let mut s = crate::state::tests::ultimate(firmware);
            let index = s.description().modes.iter().position(|m| m.id == mode).unwrap();
            s.select(index, 0);
            pages(&s)[tab].note.to_string()
        };
        assert_eq!(note("1.02", XINPUT, 0), "Firmware 1.03+ adds: D-pad four-way only.");
        assert_eq!(note("1.02", SWITCH, 0), "Firmware 1.03+ adds: D-pad four-way only.");
        assert_eq!(
            note("", XINPUT, 0),
            "8B cannot read the firmware version and hides: D-pad four-way only (firmware 1.03+)."
        );
        assert_eq!(note("1.03", XINPUT, 0), "", "nothing hidden");
        assert_eq!(note("", XINPUT, 1), "", "a tab with no gate");
    }

    #[test]
    fn each_mode_shows_its_own_settings() {
        use controller_core::devices::ultimate::{SWITCH, XINPUT};
        let fields =
            |mode| stick_toggles("1.09", mode).into_iter().flat_map(|(_, f)| f).collect::<Vec<_>>();
        let (xinput, switch) = (fields(XINPUT), fields(SWITCH));
        assert!(xinput.contains(&"/sticks/no_dead_zone".to_owned()));
        assert!(!switch.contains(&"/sticks/no_dead_zone".to_owned()));
        assert!(switch.contains(&"/sticks/swap_right_stick_with_triggers".to_owned()));
        assert!(!xinput.contains(&"/sticks/swap_right_stick_with_triggers".to_owned()));
    }
}
