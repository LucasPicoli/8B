//! The macros a write would put that the controller refuses. Core's `validate_macro`
//! judges each one; the Macros tab marks it and the write buttons stay off.

use std::cell::RefCell;

use controller_core::model::macros::macro_to_json;
use controller_core::model::{MacroDefinition, Mode};
use controller_core::service::validation::{validate_macro, ValidationError};
use tr::tr;

use crate::reasons::worded;
use crate::state::AppState;
use crate::ui::{AppWindow, Blocked};

/// How many checked macros [`CHECKED`] keeps.
const CHECKED_MAX: usize = 16;

thread_local! {
    /// The last macros checked, with the device they were checked for and the result:
    /// one render asks about the same macros several times.
    static CHECKED: RefCell<Vec<(String, MacroDefinition, Vec<ValidationError>)>> =
        const { RefCell::new(Vec::new()) };
}

/// A macro the write of a slot would put that the controller refuses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invalid {
    /// The macro slot, 0-based.
    pub macro_slot: u8,
    /// The validator's first reason, a sentence.
    pub reason: String,
}

impl AppState {
    /// The validator's errors for the macro the write of `slot` puts in macro slot `n`.
    /// Empty when the write puts no macro there, or puts a valid one.
    #[must_use]
    pub fn macro_errors(&self, (mode, number): (Mode, u8), n: u8) -> Vec<ValidationError> {
        let Some(model) = self.model() else { return Vec::new() };
        let puts = self.slot(mode, number).macro_puts();
        let Some(def) = puts.iter().find(|d| d.macro_slot == Some(n)) else { return Vec::new() };
        let device = model.default_profile(mode).device;
        CHECKED.with_borrow_mut(|checked| {
            if let Some((.., errors)) = checked.iter().find(|(d, m, _)| *d == device && m == def) {
                return errors.clone();
            }
            // Compiles the macro schema each time, about 0.2 ms.
            let errors = validate_macro(model, &macro_to_json(def, &device)).unwrap_or_default();
            if checked.len() == CHECKED_MAX {
                checked.remove(0);
            }
            checked.push((device, def.clone(), errors.clone()));
            errors
        })
    }

    /// Each macro the write of `slot` puts that the controller refuses, by macro slot.
    #[must_use]
    pub fn invalid_macros(&self, (mode, number): (Mode, u8)) -> Vec<Invalid> {
        let puts = self.slot(mode, number).macro_puts();
        let mut slots: Vec<u8> = puts.iter().filter_map(|d| d.macro_slot).collect();
        slots.sort_unstable();
        slots
            .into_iter()
            .filter_map(|n| {
                let errors = self.macro_errors((mode, number), n);
                let reason = worded(self.description(), &errors.first()?.reason);
                Some(Invalid { macro_slot: n, reason })
            })
            .collect()
    }

    /// The first macro the write of `slot` puts that the controller refuses.
    #[must_use]
    pub fn invalid_macro(&self, key: (Mode, u8)) -> Option<Invalid> {
        self.invalid_macros(key).into_iter().next()
    }

    /// Where the "Show" link of a refused macro leads, and what it says: `text` names
    /// macro slot `n` of `slot`, and `mode` and `slot` are the sidebar's indexes.
    fn blocked(&self, (mode, number): (Mode, u8), n: u8, text: String) -> Blocked {
        let modes = &self.description().modes;
        Blocked {
            text: text.into(),
            mode: modes
                .iter()
                .position(|m| m.id == mode)
                .map_or(0, |i| i32::try_from(i).unwrap_or(0)),
            slot: i32::from(number) - 1,
            macro_slot: i32::from(n),
        }
    }
}

/// Why the footer's write button is off, "Macro 2 can't be written." or "3 macros can't
/// be written.", and where Show leads: the first one. Empty text while the selected
/// slot's macros are all valid.
#[must_use]
pub fn write_blocked(state: &AppState) -> Blocked {
    let Some(key) = state.selected_slot() else { return Blocked::default() };
    let invalid = state.invalid_macros(key);
    let Some(first) = invalid.first() else { return Blocked::default() };
    let text = match invalid.len() {
        1 => tr!("Macro {n} can't be written.", n = u16::from(first.macro_slot) + 1),
        n => tr!("{n} macro can't be written." | "{n} macros can't be written." % n),
    };
    state.blocked(key, first.macro_slot, text)
}

/// Why Write all is off, "Macro 2 of `XInput` slot 2 can't be written." or "3 macros can't
/// be written.", and where Show leads: the first one. Empty text while every edited
/// slot's macros are valid.
#[must_use]
pub fn write_all_blocked(state: &AppState) -> Blocked {
    let d = state.description();
    let invalid: Vec<_> = state
        .edited_slots()
        .into_iter()
        .flat_map(|key| state.invalid_macros(key).into_iter().map(move |i| (key, i)))
        .collect();
    let Some(((mode, number), first)) = invalid.first() else { return Blocked::default() };
    let text = match invalid.len() {
        1 => tr!(
            "Macro {n} of {mode} slot {slot} can't be written.",
            n = u16::from(first.macro_slot) + 1,
            mode = d.mode_label(*mode),
            slot = number
        ),
        n => tr!("{n} macro can't be written." | "{n} macros can't be written." % n),
    };
    state.blocked((*mode, *number), first.macro_slot, text)
}

/// Pushes why each write button is off.
pub fn render_blocks(state: &AppState, ui: &AppWindow) {
    ui.set_write_blocked(write_blocked(state));
    ui.set_write_all_blocked(write_all_blocked(state));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::XINPUT;
    use controller_core::model::{GlideControl, MacroStep};

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// `XInput` slot 2 selected, holding "Jump" in macro slot 0 on PR.
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        hold_macros(&mut s, (XINPUT, 2), &[macro_def(0, "Jump", &["rp"], 3)]);
        s.select(0, 1);
        s
    }

    #[test]
    fn a_held_or_new_macro_is_valid() {
        let mut s = held();
        assert_eq!(s.invalid_macro((XINPUT, 2)), None);
        s.new_macro(1);
        assert_eq!(s.macro_errors((XINPUT, 2), 1), []);
        assert_eq!(s.invalid_macro((XINPUT, 2)), None);
    }

    #[test]
    fn an_empty_name_is_written_as_macro_n() {
        let mut s = held();
        s.rename_macro(0, "");
        assert_eq!(s.invalid_macro((XINPUT, 2)), None);
        let puts = s.slot(XINPUT, 2).macro_puts();
        assert_eq!(puts[0].name, "Macro 1");
        assert!(s.can_write());
        let file = crate::embedded::export_file(&s.slot(XINPUT, 2)).unwrap().unwrap();
        assert!(file.contains("\"name\": \"Macro 1\""), "the export names it too");
    }

    #[test]
    fn glides_past_the_step_limit_are_refused_at_the_steps() {
        let mut s = held();
        let max = s.description().limits.macro_steps.max;
        // Three long glides from one edge to the other, one short step per value.
        s.edit_macro(0, |m| {
            m.steps = (0..4_u8)
                .map(|i| MacroStep {
                    duration_ms: u16::MAX,
                    left_stick_x: if i % 2 == 0 { 0 } else { u8::MAX },
                    glide: (i < 3).then_some(GlideControl::LeftStick).into_iter().collect(),
                    ..MacroStep::default()
                })
                .collect();
        });
        let errors = s.macro_errors((XINPUT, 2), 0);
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert_eq!(errors[0].path, "/steps");
        assert!(errors[0].reason.to_string().contains(&max.to_string()), "{}", errors[0].reason);
        let invalid = s.invalid_macro((XINPUT, 2)).unwrap();
        assert!(invalid.reason.ends_with("Shorten a glide or remove steps."), "{}", invalid.reason);
        assert!(!s.can_write() && !s.can_write_all());
        let footer = write_blocked(&s);
        assert_eq!(footer.text, "Macro 1 can't be written.");
        assert_eq!((footer.mode, footer.slot, footer.macro_slot), (0, 1, 0));
        assert_eq!(write_all_blocked(&s).text, "Macro 1 of XInput slot 2 can't be written.");
        s.select(1, 0);
        assert!(write_blocked(&s).text.is_empty(), "another slot is selected");
        let all = write_all_blocked(&s);
        assert_eq!((all.mode, all.slot, all.macro_slot), (0, 1, 0), "Show leads back");
    }

    #[test]
    fn several_refused_macros_are_counted_and_show_leads_to_the_first() {
        let mut s = held();
        let long = |m: &mut MacroDefinition| m.name = "A name far too long".to_owned();
        s.edit_macro(0, long);
        s.new_macro(1);
        s.edit_macro(1, long);
        let footer = write_blocked(&s);
        assert_eq!(footer.text, "2 macros can't be written.");
        assert_eq!((footer.mode, footer.slot, footer.macro_slot), (0, 1, 0));
        let all = write_all_blocked(&s);
        assert_eq!(all.text, "2 macros can't be written.");
        assert_eq!((all.mode, all.slot, all.macro_slot), (0, 1, 0));
        // One more in another slot counts in Write all, not in the footer.
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(0, "Jump", &["lp"], 3)]);
        s.select(0, 0);
        s.edit_macro(0, long);
        s.select(0, 1);
        assert_eq!(write_blocked(&s).text, "2 macros can't be written.");
        assert_eq!(write_all_blocked(&s).text, "3 macros can't be written.");
    }

    #[test]
    fn only_the_macros_the_write_puts_are_checked() {
        let mut s = held();
        // The controller's own macro is not written, so its empty name blocks nothing.
        hold_macros(&mut s, (XINPUT, 2), &[macro_def(0, "", &["rp"], 3)]);
        assert_eq!(s.invalid_macro((XINPUT, 2)), None);
        s.delete_macro(0);
        assert_eq!(s.invalid_macro((XINPUT, 2)), None, "a delete puts nothing");
    }
}
