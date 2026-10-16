//! The trigger pressure of the step panel: how far the selected step presses each
//! button that holds a trigger value, and its glide. Only in a mode whose triggers are
//! analog; an on/off trigger is the controller views' to set.

use controller_core::description::{Button, TriggerKind, TriggerSide};
use controller_core::model::glide::{axes, set_axes};
use controller_core::model::GlideControl;

use crate::buttons::push_model;
use crate::state::AppState;
use crate::stick_pad::{glide_box, glides_in_from};
use crate::ui::{AppWindow, PadTrigger};

/// The glide control of a trigger value, and the key that joins its edits in one undo.
const fn control(side: TriggerSide) -> (GlideControl, &'static str) {
    match side {
        TriggerSide::Left => (GlideControl::LeftTrigger, "left trigger"),
        TriggerSide::Right => (GlideControl::RightTrigger, "right trigger"),
    }
}

/// The buttons that hold a trigger value in a step, with their side, in the
/// description's order. None unless the selected slot's mode has analog triggers.
fn trigger_buttons(state: &AppState) -> Vec<(&Button, TriggerSide)> {
    let description = state.description();
    let analog = state
        .selected_slot()
        .and_then(|(mode, _)| description.mode(mode))
        .is_some_and(|m| m.trigger_kind == TriggerKind::Analog);
    if !analog {
        return Vec::new();
    }
    description.buttons.iter().filter_map(|b| b.step_trigger_value.map(|side| (b, side))).collect()
}

impl AppState {
    /// Sets trigger `trigger` (its place in the panel) of step `step` of macro slot `n`
    /// to `value`, clamped to 0 to 255. Edits of one trigger in a row, such as a slider
    /// drag, are one undo; `apart` makes this one its own.
    pub fn set_trigger(&mut self, n: u8, step: usize, trigger: usize, value: i32, apart: bool) {
        let Some(&(_, side)) = trigger_buttons(self).get(trigger) else { return };
        let (control, label) = control(side);
        let v = u8::try_from(value.clamp(0, 255)).unwrap_or(0);
        let key = (!apart).then_some((step, label));
        self.edit_step_values(n, step, key, |s| set_axes(s, control, [v, v]));
    }

    /// Turns the glide of trigger `trigger` of step `step` of macro slot `n` on or off.
    /// A glide that cannot play stays off.
    pub fn set_trigger_glide(&mut self, n: u8, step: usize, trigger: usize, on: bool) {
        let Some(&(_, side)) = trigger_buttons(self).get(trigger) else { return };
        let (control, _) = control(side);
        self.edit_step_values(n, step, None, |s| {
            if on {
                s.glide.insert(control);
            } else {
                s.glide.remove(&control);
            }
        });
    }
}

/// The trigger pressure of step `step` of macro slot `n`: one per button that holds a
/// trigger value, or none when there is no such step or the mode's triggers are on/off.
#[must_use]
pub fn trigger_pads(state: &AppState, n: u8, step: usize) -> Vec<PadTrigger> {
    let Some(steps) = state.macro_steps(n).filter(|s| step < s.len()) else { return Vec::new() };
    let Some((mode, _)) = state.selected_slot() else { return Vec::new() };
    trigger_buttons(state)
        .into_iter()
        .map(|(button, side)| {
            let (control, _) = control(side);
            let (glide_label, can_glide, tip) = glide_box(&steps, step, control);
            let value = |i: usize| i32::from(steps.get(i).map_or(0, |s| axes(s, control)[0]));
            PadTrigger {
                label: button.labels.get(&mode).map_or(button.id.as_str(), String::as_str).into(),
                value: value(step),
                glide: steps.get(step).is_some_and(|s| s.glide.contains(&control)),
                to_value: value(step + 1),
                glide_label: glide_label.into(),
                can_glide,
                glide_tip: tip.into(),
                glides_in_from: glides_in_from(&steps, step, control),
            }
        })
        .collect()
}

/// Pushes the trigger pressure of the step the table selects, in place while the
/// triggers stay, so a slider drag survives the render after each move.
pub fn render_triggers(state: &AppState, ui: &AppWindow) {
    let n = u8::try_from(ui.get_selected_macro()).unwrap_or(0);
    let at = usize::try_from(ui.get_selected_step()).unwrap_or(0);
    push_model(&ui.get_step_triggers(), trigger_pads(state, n, at), |m| ui.set_step_triggers(m));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use controller_core::model::Mode;

    use controller_core::model::MacroStep;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// Slot 2 of `mode` selected, holding macro slot 0 with `steps`.
    fn holding(mode: Mode, steps: Vec<MacroStep>) -> AppState {
        let mut s = connected(mode);
        let mut def = macro_def(0, "Jump", &["rp"], 0);
        def.mode = mode;
        def.steps = steps;
        hold_macros(&mut s, (mode, 2), &[def]);
        let group = s.description().modes.iter().position(|m| m.id == mode).unwrap();
        s.select(group, 1);
        s
    }

    fn step(ms: u16, lt: u8) -> MacroStep {
        MacroStep { duration_ms: ms, trigger_left: lt, ..MacroStep::default() }
    }

    #[test]
    fn analog_modes_show_each_trigger_with_its_label_and_value() {
        let s = holding(XINPUT, vec![step(100, 128), step(100, 0)]);
        let pads = trigger_pads(&s, 0, 0);
        let shown: Vec<_> = pads.iter().map(|p| (p.label.as_str(), p.value)).collect();
        assert_eq!(shown, [("LT", 128), ("RT", 0)]);
        assert_eq!(trigger_pads(&s, 0, 2), [], "no step 3");
        let d = holding(DINPUT, vec![step(100, 9)]);
        assert_eq!(trigger_pads(&d, 0, 0).len(), 2, "DInput triggers are analog");
    }

    #[test]
    fn on_off_modes_show_no_pressure_and_take_no_edit() {
        let mut s = holding(SWITCH, vec![step(100, 255), step(100, 0)]);
        assert_eq!(trigger_pads(&s, 0, 0), []);
        s.set_trigger(0, 0, 0, 40, true);
        s.set_trigger_glide(0, 0, 0, true);
        let first = &s.macro_steps(0).unwrap()[0];
        assert_eq!((first.trigger_left, first.glide.len()), (255, 0));
    }

    #[test]
    fn a_pressure_edit_clamps_prunes_glides_and_joins_undo() {
        let glide = MacroStep { glide: [GlideControl::LeftTrigger].into(), ..step(100, 0) };
        let mut s = holding(XINPUT, vec![glide, step(100, 200)]);
        for v in [50, 120, 0] {
            s.set_trigger(0, 1, 0, v, false);
        }
        assert!(s.macro_steps(0).unwrap()[0].glide.is_empty(), "step 2 now holds the same");
        s.set_trigger(0, 1, 1, 300, true);
        assert_eq!(s.macro_steps(0).unwrap()[1].trigger_right, 255, "clamped");
        assert_eq!(s.undo_step(0), Some(1), "RT, on its own");
        assert_eq!(s.undo_step(0), Some(1), "the LT drag, as one");
        assert_eq!(s.macro_steps(0).unwrap()[1].trigger_left, 200);
        assert_eq!(s.undo_step(0), None);
    }

    #[test]
    fn a_glide_turns_on_only_where_it_can_play_and_names_its_steps() {
        let mut s = holding(XINPUT, vec![step(100, 0), step(80, 255), step(100, 255)]);
        s.set_trigger_glide(0, 0, 0, true);
        s.set_trigger_glide(0, 1, 0, true);
        let steps = s.macro_steps(0).unwrap();
        assert_eq!(steps[0].glide, [GlideControl::LeftTrigger].into());
        assert!(steps[1].glide.is_empty(), "step 3 holds the same pressure");
        let [lt, _] = <[PadTrigger; 2]>::try_from(trigger_pads(&s, 0, 0)).unwrap();
        assert!(lt.glide && lt.can_glide);
        assert_eq!((lt.value, lt.to_value), (0, 255), "it glides to step 2's pressure");
        assert_eq!(lt.glide_label, "Glide to step 2");
        assert_eq!(
            lt.glide_tip,
            "Moves in a straight line from here to step 2's pressure over this step's 100 ms."
        );
        let arriving = &trigger_pads(&s, 0, 1)[0];
        assert_eq!((arriving.glides_in_from, arriving.can_glide), (1, false));
        assert_eq!(
            arriving.glide_tip,
            "Step 3 holds the same pressure, so there is nothing to glide to."
        );
        let last = &trigger_pads(&s, 0, 2)[0];
        assert_eq!(
            (last.glide_label.as_str(), last.glide_tip.as_str()),
            ("Glide", "No next step to glide to.")
        );
    }
}
