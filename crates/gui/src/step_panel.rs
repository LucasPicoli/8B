//! The selected step's duration and the buttons it holds, which the controller views of
//! the Macros tab show and toggle. Which buttons can be in a
//! step, and which of them a trigger value holds, come from the controller description.

use std::rc::Rc;

use controller_core::description::{Button, TriggerSide};
use controller_core::model::{GlideControl, MacroStep, Mode};
use slint::{ModelRc, VecModel};

use crate::buttons::{hit, output_label};
use crate::macro_steps::prune_glides;
use crate::state::AppState;
use crate::ui::{AppWindow, ChipInfo, MacroSpot};

/// A trigger value pressed fully.
const FULL: u8 = u8::MAX;

/// The buttons that can be in a macro step, in the description's order.
fn step_buttons(state: &AppState) -> Vec<&Button> {
    state.description().buttons.iter().filter(|b| b.can_be_in_macro_step).collect()
}

/// The trigger value of `side` in `step`, and its glide control.
const fn trigger_mut(step: &mut MacroStep, side: TriggerSide) -> (&mut u8, GlideControl) {
    match side {
        TriggerSide::Left => (&mut step.trigger_left, GlideControl::LeftTrigger),
        TriggerSide::Right => (&mut step.trigger_right, GlideControl::RightTrigger),
    }
}

/// Whether `step` holds `button`: among its pressed buttons, or in its trigger value.
fn holds(step: &MacroStep, button: &Button) -> bool {
    match button.step_trigger_value {
        Some(TriggerSide::Left) => step.trigger_left > 0,
        Some(TriggerSide::Right) => step.trigger_right > 0,
        None => step.pressed_buttons.contains(&button.id),
    }
}

impl AppState {
    /// Applies `change` to step `step` of the macro in macro slot `n`, then drops the
    /// glides that no longer move, and records the edit for undo under `key`.
    pub(crate) fn edit_step_values(
        &mut self,
        n: u8,
        step: usize,
        key: Option<(usize, &'static str)>,
        change: impl FnOnce(&mut MacroStep),
    ) {
        let Some(before) = self.macro_steps(n).filter(|s| step < s.len()) else { return };
        self.edit_macro(n, |m| {
            if let Some(s) = m.steps.get_mut(step) {
                change(s);
            }
            prune_glides(&mut m.steps);
        });
        self.record_step_edit(n, before, step, key);
    }

    /// Sets the duration of step `step` of macro slot `n`, within the controller's range.
    /// Typing a number in a row is one undo.
    pub fn set_step_duration(&mut self, n: u8, step: usize, ms: i32) {
        let range = self.description().limits.macro_step_ms;
        let Ok(ms) = u16::try_from(ms.clamp(range.min, range.max)) else { return };
        self.edit_step_values(n, step, Some((step, "duration")), |s| s.duration_ms = ms);
    }

    /// Turns chip `chip` of "Buttons held" on or off in step `step` of macro slot `n`. A
    /// button held as a trigger value goes to full or to 0, and off drops its glide.
    pub fn toggle_step_button(&mut self, n: u8, step: usize, chip: usize) {
        let Some(button) = step_buttons(self).get(chip).map(|b| (*b).clone()) else { return };
        let order: Vec<String> = step_buttons(self).iter().map(|b| b.id.clone()).collect();
        self.edit_step_values(n, step, None, |s| {
            let on = !holds(s, &button);
            match button.step_trigger_value {
                Some(side) => {
                    let (value, glide) = trigger_mut(s, side);
                    *value = if on { FULL } else { 0 };
                    if !on {
                        s.glide.remove(&glide);
                    }
                }
                None if on => {
                    s.pressed_buttons.push(button.id.clone());
                    s.pressed_buttons.sort_by_key(|b| order.iter().position(|o| o == b));
                }
                None => s.pressed_buttons.retain(|b| *b != button.id),
            }
        });
    }
}

/// "Buttons held" of `step` in `mode`: every button that can be in a step, on when the
/// step holds it.
#[must_use]
pub fn chips(state: &AppState, mode: Mode, step: &MacroStep) -> Vec<ChipInfo> {
    step_buttons(state)
        .into_iter()
        .map(|b| ChipInfo {
            label: b.labels.get(&mode).map_or_else(|| b.id.as_str().into(), Into::into),
            on: holds(step, b),
        })
        .collect()
}

/// The shape of every button on the controller views, with its chip in "Buttons held"
/// and its label. Empty for an empty slot.
#[must_use]
pub fn macro_spots(state: &AppState) -> Vec<MacroSpot> {
    let description = state.description();
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    if state.slot(mode, number).shown().is_none() {
        return Vec::new();
    }
    let chips = step_buttons(state);
    let mut out = Vec::new();
    for (vi, view) in (0_i32..).zip(&description.views) {
        for h in &view.hotspots {
            let Some(row) = description.buttons.iter().position(|b| b.id == h.button) else {
                continue;
            };
            let chip = chips.iter().position(|b| b.id == h.button);
            #[allow(clippy::cast_possible_truncation)]
            let [x0, y0, x1, y1] = h.bounds().map(|v| v as f32);
            out.push(MacroSpot {
                row: i32::try_from(row).unwrap_or(-1),
                view: vi,
                commands: h.path.as_str().into(),
                chip: chip.and_then(|c| i32::try_from(c).ok()).unwrap_or(-1),
                label: output_label(description, mode, &h.button).into(),
                x0,
                y0,
                x1,
                y1,
            });
        }
    }
    out
}

/// The chip a click on the button of row `row` toggles in step `step` of macro slot
/// `n`. `None` for a button a step cannot hold, for a step that is not there, and
/// while the slot's macros are still being read.
#[must_use]
pub fn view_chip(state: &AppState, n: u8, step: usize, row: usize) -> Option<usize> {
    let (mode, number) = state.selected_slot()?;
    if !state.slot(mode, number).macros_editable() {
        return None;
    }
    let button = state.description().buttons.get(row)?;
    state.macro_steps(n).filter(|s| step < s.len())?;
    step_buttons(state).iter().position(|b| b.id == button.id)
}

/// The chip under (`x`, `y`) in view `view`, in viewBox units, when a click there
/// toggles it in step `step` of macro slot `n`, or -1.
#[must_use]
pub fn action_hit(state: &AppState, view: usize, n: u8, step: usize, x: f32, y: f32) -> i32 {
    usize::try_from(hit(state.description(), view, x, y))
        .ok()
        .and_then(|row| view_chip(state, n, step, row))
        .and_then(|c| i32::try_from(c).ok())
        .unwrap_or(-1)
}

/// Pushes the buttons the step the table selects holds, for the controller views, and
/// the range of a step's duration. A step past the end holds none.
pub fn render_panel(state: &AppState, ui: &AppWindow) {
    let n = u8::try_from(ui.get_selected_macro()).unwrap_or(0);
    let at = usize::try_from(ui.get_selected_step()).unwrap_or(0);
    let steps = state.macro_steps(n).unwrap_or_default();
    let mode = state.selected_slot().map(|(m, _)| m);
    let list = match (steps.get(at), mode) {
        (Some(step), Some(mode)) => chips(state, mode, step),
        _ => Vec::new(),
    };
    ui.set_panel_chips(ModelRc::from(Rc::new(VecModel::from(list))));
    ui.set_macro_spots(ModelRc::from(Rc::new(VecModel::from(macro_spots(state)))));
    let range = state.description().limits.macro_step_ms;
    ui.set_step_ms_min(range.min);
    ui.set_step_ms_max(range.max);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::XINPUT;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// `XInput` slot 2 selected, holding macro slot 0 with `steps`.
    fn holding(steps: Vec<MacroStep>) -> AppState {
        let mut s = connected(XINPUT);
        let mut def = macro_def(0, "Jump", &["rp"], 0);
        def.steps = steps;
        hold_macros(&mut s, (XINPUT, 2), &[def]);
        s.select(0, 1);
        s
    }

    fn step(ms: u16) -> MacroStep {
        MacroStep { duration_ms: ms, ..MacroStep::default() }
    }

    /// The chip of button `id`.
    fn chip(s: &AppState, id: &str) -> usize {
        step_buttons(s).iter().position(|b| b.id == id).unwrap()
    }

    #[test]
    fn chips_list_every_step_button_and_light_the_held_ones() {
        let held = MacroStep {
            pressed_buttons: vec!["bottom face".to_owned()],
            trigger_right: 40,
            ..step(100)
        };
        let s = holding(vec![held]);
        let list = chips(&s, XINPUT, &s.macro_steps(0).unwrap()[0]);
        assert_eq!(list.len(), 16);
        let on: Vec<_> = list.iter().filter(|c| c.on).map(|c| c.label.to_string()).collect();
        assert_eq!(on, ["A", "RT"], "a trigger value lights its chip");
    }

    #[test]
    fn a_chip_toggles_a_button_or_a_full_trigger() {
        let glide =
            MacroStep { trigger_left: 100, glide: [GlideControl::LeftTrigger].into(), ..step(100) };
        let mut s = holding(vec![glide, step(100)]);
        let (a, b, lt) = (chip(&s, "bottom face"), chip(&s, "right face"), chip(&s, "l2"));
        s.toggle_step_button(0, 1, a);
        s.toggle_step_button(0, 1, b);
        assert_eq!(s.macro_steps(0).unwrap()[1].pressed_buttons, ["right face", "bottom face"]);
        s.toggle_step_button(0, 1, a);
        assert_eq!(s.macro_steps(0).unwrap()[1].pressed_buttons, ["right face"]);
        s.toggle_step_button(0, 0, lt);
        let first = &s.macro_steps(0).unwrap()[0];
        assert_eq!(first.trigger_left, 0);
        assert!(first.glide.is_empty(), "off drops its glide");
        s.toggle_step_button(0, 0, lt);
        assert_eq!(s.macro_steps(0).unwrap()[0].trigger_left, 255);
        assert_eq!(s.undo_step(0), Some(0), "each toggle is an undo");
    }

    /// The description row of button `id`.
    fn row(s: &AppState, id: &str) -> usize {
        s.description().buttons.iter().position(|b| b.id == id).unwrap()
    }

    #[test]
    fn spots_carry_every_buttons_chip_and_label() {
        let s = holding(vec![step(100)]);
        let spots = macro_spots(&s);
        let of = |id: &str| {
            let row = i32::try_from(row(&s, id)).unwrap();
            spots.iter().find(|p| p.row == row).unwrap().clone()
        };
        let lt = i32::try_from(chip(&s, "l2")).unwrap();
        assert_eq!((of("l2").chip, of("l2").label.as_str()), (lt, "LT"));
        assert_eq!((of("r4").chip, of("r4").label.as_str()), (-1, "R4"), "not a step button");
        assert_eq!(of("home/guide").chip, -1);
    }

    #[test]
    fn a_view_click_acts_on_a_step_button_of_a_shown_step() {
        let mut s = holding(vec![step(100), step(100)]);
        assert_eq!(view_chip(&s, 0, 1, row(&s, "l2")), Some(chip(&s, "l2")));
        assert_eq!(view_chip(&s, 0, 1, row(&s, "r4")), None, "not a step button");
        assert_eq!(view_chip(&s, 0, 2, row(&s, "l2")), None, "no step 3");
        assert_eq!(view_chip(&s, 1, 0, row(&s, "l2")), None, "macro slot 2 is empty");
        s.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().pad_macros[0].loaded = false;
        assert_eq!(view_chip(&s, 0, 0, row(&s, "l2")), None, "its steps are being read");
    }

    #[test]
    fn a_duration_stays_in_range_and_typing_is_one_undo() {
        let mut s = holding(vec![step(100), step(100)]);
        for ms in [3, 30, 300] {
            s.set_step_duration(0, 1, ms);
        }
        s.set_step_duration(0, 0, 70_000);
        assert_eq!(s.macro_steps(0).unwrap()[0].duration_ms, u16::MAX);
        assert_eq!(s.undo_step(0), Some(0));
        assert_eq!(s.macro_steps(0).unwrap()[1].duration_ms, 300);
        assert_eq!(s.undo_step(0), Some(1));
        assert_eq!(s.macro_steps(0).unwrap()[1].duration_ms, 100, "3, 30 and 300 were one");
        assert!(!s.slot(XINPUT, 2).unsaved());
    }
}
