//! The step edits of the Macros tab's step table: add, duplicate, split, move and
//! delete. Pure: no Slint, no state. Each edit returns the step to select after it.

use controller_core::model::glide::{axes, set_axes};
use controller_core::model::{GlideControl, MacroStep};

/// The length of a step that Add step makes, in milliseconds.
pub const NEW_STEP_MS: u16 = 100;

/// Every control a step can glide.
const CONTROLS: [GlideControl; 4] = [
    GlideControl::LeftStick,
    GlideControl::RightStick,
    GlideControl::LeftTrigger,
    GlideControl::RightTrigger,
];

/// `a` moved the fraction `t` of the way to `b`, rounded.
fn lerp(a: u8, b: u8, t: f64) -> u8 {
    let v = (f64::from(b) - f64::from(a)).mul_add(t.clamp(0.0, 1.0), f64::from(a));
    // In 0..=255, as a and b are.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = v.round() as u8;
    v
}

/// The value of `control` the fraction `t` through step `i`: its own value, or on the
/// way to the next step's while it glides.
#[must_use]
pub fn value_at(steps: &[MacroStep], i: usize, control: GlideControl, t: f64) -> [u8; 2] {
    let Some(step) = steps.get(i) else { return [0; 2] };
    let from = axes(step, control);
    steps.get(i + 1).filter(|_| step.glide.contains(&control)).map_or(from, |next| {
        let to = axes(next, control);
        [lerp(from[0], to[0], t), lerp(from[1], to[1], t)]
    })
}

/// Drops each glide that cannot play: on the last step, or to an unchanged value.
pub fn prune_glides(steps: &mut [MacroStep]) {
    for i in 0..steps.len() {
        let next = steps.get(i + 1).cloned();
        let Some(step) = steps.get_mut(i) else { continue };
        let from = step.clone();
        step.glide.retain(|&c| next.as_ref().is_some_and(|n| axes(n, c) != axes(&from, c)));
    }
}

/// Puts an empty step after step `i`: no buttons, triggers released, sticks centred,
/// no glide.
pub fn add_after(steps: &mut Vec<MacroStep>, i: usize) -> usize {
    let at = (i + 1).min(steps.len());
    steps.insert(at, MacroStep { duration_ms: NEW_STEP_MS, ..MacroStep::default() });
    prune_glides(steps);
    at
}

/// Puts a copy of step `i` after it. The first copy holds; the second glides on.
pub fn duplicate(steps: &mut Vec<MacroStep>, i: usize) -> usize {
    let Some(step) = steps.get(i).cloned() else { return i };
    steps.insert(i + 1, step);
    prune_glides(steps);
    i + 1
}

/// Cuts step `i` in two halves. The second half starts where each glide is at the cut,
/// and both halves glide on. A step shorter than 2 ms stays whole.
pub fn split(steps: &mut Vec<MacroStep>, i: usize) -> usize {
    let Some(step) = steps.get(i).filter(|s| s.duration_ms >= 2).cloned() else { return i };
    let half = step.duration_ms / 2;
    let t = f64::from(half) / f64::from(step.duration_ms);
    let mut second = MacroStep { duration_ms: step.duration_ms - half, ..step };
    for c in CONTROLS {
        set_axes(&mut second, c, value_at(steps, i, c, t));
    }
    if let Some(first) = steps.get_mut(i) {
        first.duration_ms = half;
    }
    steps.insert(i + 1, second);
    prune_glides(steps);
    i + 1
}

/// Swaps step `i` with the one above (`down` false) or below it.
pub fn move_step(steps: &mut [MacroStep], i: usize, down: bool) -> usize {
    let j = if down { i + 1 } else { i.wrapping_sub(1) };
    if i >= steps.len() || j >= steps.len() {
        return i;
    }
    steps.swap(i, j);
    prune_glides(steps);
    j
}

/// Deletes step `i`. The last step stays: a macro has at least one.
pub fn delete(steps: &mut Vec<MacroStep>, i: usize) -> usize {
    if steps.len() <= 1 || i >= steps.len() {
        return i.min(steps.len().saturating_sub(1));
    }
    steps.remove(i);
    prune_glides(steps);
    i.min(steps.len() - 1)
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// A step of `ms` with the left stick at `x` and the left trigger at `lt`, gliding
    /// the left stick when `glide`.
    fn step(ms: u16, x: u8, lt: u8, glide: bool) -> MacroStep {
        MacroStep {
            duration_ms: ms,
            left_stick_x: x,
            trigger_left: lt,
            pressed_buttons: vec!["bottom face".to_owned()],
            glide: if glide { [GlideControl::LeftStick].into() } else { BTreeSet::default() },
            ..MacroStep::default()
        }
    }

    #[test]
    fn add_puts_an_empty_step_with_centred_sticks() {
        let mut steps = vec![step(100, 0, 200, true), step(100, 200, 0, false)];
        assert_eq!(add_after(&mut steps, 0), 1);
        assert_eq!(steps[1], MacroStep { duration_ms: NEW_STEP_MS, ..MacroStep::default() });
        assert!(steps[0].glide.contains(&GlideControl::LeftStick), "it glides to the new step");
        assert_eq!(add_after(&mut steps, 2), 3, "after the last");
    }

    #[test]
    fn duplicate_holds_then_glides_on() {
        let mut steps = vec![step(100, 0, 0, true), step(100, 200, 0, false)];
        assert_eq!(duplicate(&mut steps, 0), 1);
        assert_eq!(steps.len(), 3);
        assert!(steps[0].glide.is_empty() && steps[1].glide.contains(&GlideControl::LeftStick));
    }

    #[test]
    fn split_cuts_a_glide_at_its_value_and_both_halves_glide() {
        let mut steps = vec![step(101, 0, 0, true), step(100, 200, 0, false)];
        assert_eq!(split(&mut steps, 0), 1);
        assert_eq!((steps[0].duration_ms, steps[1].duration_ms), (50, 51));
        assert_eq!(steps[1].left_stick_x, 99, "50/101 of the way to 200");
        assert!(steps[0].glide.contains(&GlideControl::LeftStick));
        assert!(steps[1].glide.contains(&GlideControl::LeftStick));
        let mut short = vec![step(1, 0, 0, false)];
        assert_eq!(split(&mut short, 0), 0);
        assert_eq!(short.len(), 1);
    }

    #[test]
    fn move_swaps_and_drops_glides_that_no_longer_move() {
        let mut steps =
            vec![step(100, 0, 0, true), step(100, 200, 0, false), step(50, 0, 0, false)];
        assert_eq!(move_step(&mut steps, 0, false), 0, "the first cannot go up");
        assert_eq!(move_step(&mut steps, 0, true), 1);
        assert_eq!(steps[1].duration_ms, 100);
        assert!(steps[1].glide.is_empty(), "the next step holds 0, as it does");
        assert_eq!(move_step(&mut steps, 2, true), 2, "the last cannot go down");
    }

    #[test]
    fn delete_keeps_one_step_and_the_last_step_never_glides() {
        let mut steps = vec![step(100, 0, 0, true), step(100, 200, 0, false)];
        assert_eq!(delete(&mut steps, 1), 0);
        assert!(steps[0].glide.is_empty(), "nothing left to glide to");
        assert_eq!(delete(&mut steps, 0), 0);
        assert_eq!(steps.len(), 1);
    }

    #[test]
    fn value_at_follows_a_glide_only() {
        let steps = vec![step(100, 0, 0, true), step(100, 200, 100, false)];
        assert_eq!(value_at(&steps, 0, GlideControl::LeftStick, 0.5), [100, 127]);
        assert_eq!(value_at(&steps, 0, GlideControl::LeftTrigger, 0.5), [0, 0]);
        assert_eq!(value_at(&steps, 9, GlideControl::LeftStick, 0.5), [0, 0]);
    }
}
