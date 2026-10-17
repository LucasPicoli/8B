//! The stick pads of the step panel: where the selected step holds each stick and its
//! glide. Also the Macros tab's bar for a stick whose dead zone hides the macro's moves. A pad drawn as a round gate maps to the square of
//! values the way the step table's glyph does ([`disc`]).

use controller_core::model::glide::{axes, set_axes};
use controller_core::model::macros::STICK_CENTER;
use controller_core::model::{GlideControl, MacroStep};
use controller_core::service::validation::{moved_low_sticks, LowStick};
use tr::tr;

use crate::buttons::push_model;
use crate::language::prose;
use crate::state::AppState;
use crate::step_table::{disc, stick};
use crate::ui::{AppWindow, PadStick};

/// The sticks a step holds, in the order the panel shows them, each with its undo key.
const STICKS: [(GlideControl, &str); 2] =
    [(GlideControl::LeftStick, "Left stick"), (GlideControl::RightStick, "Right stick")];

/// How close to a snap point on the rim, in gate radii, a drag lands on it.
const SNAP: f32 = 0.15;

/// The 8 snap points on the rim: each full or rest value on each axis, centre left out.
const RIM: [[u8; 2]; 8] =
    [[127, 0], [255, 0], [255, 127], [255, 255], [127, 255], [0, 255], [0, 127], [0, 0]];

/// `u` from -1 to 1 as a value from 0 to 255, rest at 0.
fn value(u: f32) -> u8 {
    let rest = f32::from(STICK_CENTER);
    let v = if u < 0.0 { rest.mul_add(u, rest) } else { (255.0 - rest).mul_add(u, rest) };
    // Clamped to 0..=255 first.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = v.round().clamp(0.0, 255.0) as u8;
    v
}

/// The values at (`x`, `y`) on the gate, in gate radii from its centre: the inverse of
/// [`disc`]. A point outside the gate goes to its rim, and with `snap` one near a rim
/// snap point lands on it.
#[must_use]
pub fn from_disc(x: f32, y: f32, snap: bool) -> [u8; 2] {
    let reach = x.hypot(y);
    let (x, y) = if reach > 1.0 { (x / reach, y / reach) } else { (x, y) };
    let near = |&[sx, sy]: &[u8; 2]| {
        let (gx, gy) = disc(sx, sy);
        (gx - x).hypot(gy - y) < SNAP
    };
    if let Some(point) = RIM.iter().find(|v| snap && near(v)) {
        return *point;
    }
    // The inverse of the elliptical grid mapping `disc` uses, for one axis `along`.
    let root8 = 8.0_f32.sqrt();
    let axis = |along: f32, across: f32| {
        let base = along.mul_add(along, across.mul_add(-across, 2.0));
        let plus = root8.mul_add(along, base).max(0.0).sqrt();
        let minus = (-root8).mul_add(along, base).max(0.0).sqrt();
        (0.5 * (plus - minus)).clamp(-1.0, 1.0)
    };
    [value(axis(x, y)), value(axis(y, x))]
}

/// The Glide box of `control` in step `i`: its label, whether it can glide, and its
/// tip, which says what a glide does or why it cannot. A stick glides its position, a
/// trigger its pressure.
pub fn glide_box(steps: &[MacroStep], i: usize, control: GlideControl) -> (String, bool, String) {
    let stick = matches!(control, GlideControl::LeftStick | GlideControl::RightStick);
    let next = i + 2;
    let label = tr!("Glide to step {n}", n = next);
    match (steps.get(i), steps.get(i + 1)) {
        (_, None) => (tr!("Glide"), false, tr!("No next step to glide to.")),
        (Some(s), Some(n)) if axes(s, control) == axes(n, control) => (
            label,
            false,
            if stick {
                tr!("Step {n} holds the same position, so there is nothing to glide to.", n = next)
            } else {
                tr!("Step {n} holds the same pressure, so there is nothing to glide to.", n = next)
            },
        ),
        (s, _) => {
            let ms = s.map_or(0, |s| s.duration_ms);
            let tip = if stick {
                tr!(
                    "Moves in a straight line from here to step {n}’s position over this \
                     step’s {ms} ms.",
                    n = next,
                    ms = ms
                )
            } else {
                tr!(
                    "Moves in a straight line from here to step {n}’s pressure over this \
                     step’s {ms} ms.",
                    n = next,
                    ms = ms
                )
            };
            (label, true, tip)
        }
    }
}

/// The step before step `i` when it glides `control` here: its number, 1-based, or 0.
pub fn glides_in_from(steps: &[MacroStep], i: usize, control: GlideControl) -> i32 {
    i.checked_sub(1)
        .filter(|&p| steps.get(p).is_some_and(|s| s.glide.contains(&control)))
        .and_then(|p| i32::try_from(p + 1).ok())
        .unwrap_or(0)
}

impl AppState {
    /// Sets stick `stick` (0 left, 1 right) of step `step` of macro slot `n` to
    /// (`x`, `y`), each clamped to 0 to 255. Edits of one stick in a row, such as a drag
    /// or the arrows of a field, are one undo; `apart` makes this one its own.
    pub fn set_stick(&mut self, n: u8, step: usize, stick: usize, [x, y]: [i32; 2], apart: bool) {
        let Some(&(control, label)) = STICKS.get(stick) else { return };
        let v = |a: i32| u8::try_from(a.clamp(0, 255)).unwrap_or(STICK_CENTER);
        let key = (!apart).then_some((step, label));
        self.edit_step_values(n, step, key, |s| set_axes(s, control, [v(x), v(y)]));
    }

    /// Sets stick `stick` of step `step` of macro slot `n` to the point (`x`, `y`) on its
    /// gate, snapping to the rim with `snap`, see [`from_disc`].
    pub fn drag_stick(&mut self, n: u8, step: usize, stick: usize, [x, y]: [f32; 2], snap: bool) {
        let [vx, vy] = from_disc(x, y, snap);
        self.set_stick(n, step, stick, [i32::from(vx), i32::from(vy)], false);
    }

    /// Turns the glide of stick `stick` of step `step` of macro slot `n` on or off. A
    /// glide that cannot play stays off.
    pub fn set_stick_glide(&mut self, n: u8, step: usize, stick: usize, on: bool) {
        let Some(&(control, _)) = STICKS.get(stick) else { return };
        self.edit_step_values(n, step, None, |s| {
            if on {
                s.glide.insert(control);
            } else {
                s.glide.remove(&control);
            }
        });
    }
}

/// Both stick pads of step `step` of macro slot `n`, or none when there is no such
/// step.
#[must_use]
pub fn stick_pads(state: &AppState, n: u8, step: usize) -> Vec<PadStick> {
    let Some(steps) = state.macro_steps(n).filter(|s| step < s.len()) else { return Vec::new() };
    STICKS
        .iter()
        .map(|&(control, _)| {
            let [x, y] = steps.get(step).map_or([STICK_CENTER; 2], |s| axes(s, control));
            let (glide_label, can_glide, tip) = glide_box(&steps, step, control);
            let label = if control == GlideControl::LeftStick {
                tr!("Left stick")
            } else {
                tr!("Right stick")
            };
            PadStick {
                label: label.into(),
                value_x: i32::from(x),
                value_y: i32::from(y),
                at: stick(&steps, step, control),
                glide_label: glide_label.into(),
                can_glide,
                glide_tip: tip.into(),
                glides_in_from: glides_in_from(&steps, step, control),
            }
        })
        .collect()
}

/// The sticks macro slot `n` moves whose dead zone low end is under the macro stick
/// floor of the selected slot's mode, and that floor.
fn low_moved(state: &AppState, n: u8) -> Option<(Vec<LowStick>, i32)> {
    let (mode, number) = state.selected_slot()?;
    let slot = state.slot(mode, number);
    let floor = state.description().mode(mode)?.macro_stick_floor?;
    let profile = serde_json::to_value(slot.shown()?).ok()?;
    let low = moved_low_sticks(state.description(), mode, &profile, slot.shown_macro(n)?);
    (!low.is_empty()).then_some((low, floor))
}

/// The bar over the step table of macro slot `n` when a stick it moves sits under the
/// floor: its title, its body and its action, or nothing.
#[must_use]
pub fn floor_bar(state: &AppState, n: u8) -> Option<[String; 3]> {
    let (low, floor) = low_moved(state, n)?;
    let mode = state.selected_slot().map(|(m, _)| state.description().mode_label(m))?;
    let title = low
        .iter()
        .map(|l| tr!("{stick}: the dead zone low end is {n} %.", stick = prose(&l.name), n = l.low))
        .collect::<Vec<_>>()
        .join(" ");
    Some([
        title,
        tr!(
            "In {mode} mode, a macro moves a stick only when its dead zone low end is {floor} % or more.",
            mode = mode,
            floor = floor
        ),
        tr!("Set to {floor} %", floor = floor),
    ])
}

impl AppState {
    /// Raises the dead zone low end of each stick macro slot `n` moves to the macro stick
    /// floor, so the moves play. A profile edit: Discard or the Sticks tab takes it back.
    pub fn raise_to_stick_floor(&mut self, n: u8) {
        let Some((low, floor)) = low_moved(self, n) else { return };
        for l in low {
            #[allow(clippy::cast_precision_loss)]
            self.set_number(&l.field, floor as f32);
        }
    }
}

/// Pushes the stick pads of the step the table selects, in place while the pads stay,
/// so a drag or a typed number survives the render after each edit.
pub fn render_sticks(state: &AppState, ui: &AppWindow) {
    let n = u8::try_from(ui.get_selected_macro()).unwrap_or(0);
    let at = usize::try_from(ui.get_selected_step()).unwrap_or(0);
    let [title, body, action] = floor_bar(state, n).unwrap_or_default();
    ui.set_floor_title(title.into());
    ui.set_floor_body(body.into());
    ui.set_floor_action(action.into());
    push_model(&ui.get_step_sticks(), stick_pads(state, n, at), |m| ui.set_step_sticks(m));
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

    #[test]
    fn a_point_on_the_gate_gives_back_its_values() {
        for x in (0..=255).step_by(15) {
            for y in (0..=255).step_by(15) {
                let (dx, dy) = disc(x, y);
                let [a, b] = from_disc(dx, dy, true);
                let near_rim = RIM.contains(&[a, b]);
                assert!(
                    near_rim || (a.abs_diff(x) <= 1 && b.abs_diff(y) <= 1),
                    "{x}, {y} came back as {a}, {b}"
                );
            }
        }
        assert_eq!(from_disc(0.0, 0.0, true), [127, 127]);
    }

    #[test]
    fn a_drag_past_the_rim_or_near_a_snap_point_lands_on_it_while_snapping() {
        assert_eq!(from_disc(3.0, 0.0, false), [255, 127], "past the rim, east");
        assert_eq!(from_disc(0.68, -0.75, true), [255, 0], "near the north-east corner");
        assert_ne!(from_disc(0.68, -0.75, false), [255, 0], "snapping off");
        assert_eq!(from_disc(-0.05, 0.95, true), [127, 255], "near south");
        let [a, _] = from_disc(0.5, 0.0, true);
        assert!((180..=200).contains(&a), "halfway east is {a}");
    }

    #[test]
    fn a_stick_edit_sets_its_values_prunes_glides_and_joins_undo() {
        let glide =
            MacroStep { left_stick_x: 0, glide: [GlideControl::LeftStick].into(), ..step(100) };
        let mut s = holding(vec![glide, step(100)]);
        s.drag_stick(0, 1, 0, [0.5, 0.0], true);
        s.set_stick(0, 1, 0, [0, 127], false);
        let steps = s.macro_steps(0).unwrap();
        assert_eq!((steps[1].left_stick_x, steps[1].left_stick_y), (0, 127));
        assert!(steps[0].glide.is_empty(), "step 2 now holds the same position");
        s.set_stick(0, 1, 1, [300, -4], true);
        let right = &s.macro_steps(0).unwrap()[1];
        assert_eq!((right.right_stick_x, right.right_stick_y), (255, 0), "clamped");
        assert_eq!(s.undo_step(0), Some(1), "the right stick, on its own");
        assert_eq!(s.undo_step(0), Some(1), "the drag and the field, as one");
        assert_eq!(s.macro_steps(0).unwrap()[0].glide.len(), 1);
        assert_eq!(s.undo_step(0), None);
    }

    #[test]
    fn a_glide_turns_on_only_where_it_can_play() {
        let moved = MacroStep { right_stick_x: 255, ..step(100) };
        let mut s = holding(vec![step(100), moved, step(100)]);
        s.set_stick_glide(0, 0, 1, true);
        s.set_stick_glide(0, 0, 0, true);
        s.set_stick_glide(0, 2, 1, true);
        let steps = s.macro_steps(0).unwrap();
        assert_eq!(steps[0].glide, [GlideControl::RightStick].into());
        assert!(steps[2].glide.is_empty(), "the last step");
        s.set_stick_glide(0, 0, 1, false);
        assert!(s.macro_steps(0).unwrap()[0].glide.is_empty());
    }

    #[test]
    fn pads_show_values_and_glide_tips() {
        let moved = MacroStep { left_stick_x: 255, left_stick_y: 0, ..step(100) };
        let s = holding(vec![step(100), moved.clone(), moved]);
        let pads = stick_pads(&s, 0, 1);
        assert_eq!(pads.len(), 2);
        assert_eq!(
            (pads[0].value_x, pads[0].value_y, pads[0].label.as_str()),
            (255, 0, "Left stick")
        );
        assert!(!pads[0].can_glide && !pads[1].can_glide);
        assert_eq!(pads[0].glide_label, "Glide to step 3");
        assert_eq!(
            pads[0].glide_tip,
            "Step 3 holds the same position, so there is nothing to glide to."
        );
        let last = &stick_pads(&s, 0, 2)[0];
        assert_eq!((last.glide_label.as_str(), last.can_glide), ("Glide", false));
        assert_eq!(last.glide_tip, "No next step to glide to.");
        let first = &stick_pads(&s, 0, 0)[0];
        assert!(first.can_glide);
        assert_eq!(
            first.glide_tip,
            "Moves in a straight line from here to step 2’s position over this step’s 100 ms."
        );
        assert!(stick_pads(&s, 0, 3).is_empty(), "no step 4");
    }

    #[test]
    fn the_arriving_step_names_the_step_that_glides_in() {
        let glide =
            MacroStep { left_stick_x: 0, glide: [GlideControl::LeftStick].into(), ..step(100) };
        let s = holding(vec![step(100), glide, step(100)]);
        let arriving = stick_pads(&s, 0, 2);
        assert_eq!((arriving[0].glides_in_from, arriving[1].glides_in_from), (2, 0));
        assert_eq!(stick_pads(&s, 0, 1)[0].glides_in_from, 0, "step 1 does not glide");
        assert_eq!(stick_pads(&s, 0, 0)[0].glides_in_from, 0, "no step before");
    }

    #[test]
    fn the_floor_bar_names_each_low_stick_and_its_action_raises_them() {
        let moved = MacroStep { left_stick_x: 255, ..step(100) };
        let mut s = holding(vec![step(100), moved]);
        assert_eq!(floor_bar(&s, 0), None);
        s.set_number("/sticks/left_min_pct", 0.0);
        s.set_number("/sticks/right_min_pct", 1.0);
        let [title, body, action] = floor_bar(&s, 0).unwrap();
        assert_eq!(
            title, "Left stick: the dead zone low end is 0 %.",
            "the right one does not move"
        );
        assert_eq!(
            body,
            "In XInput mode, a macro moves a stick only when its dead zone low end is 2 % or more."
        );
        assert_eq!(action, "Set to 2 %");
        s.raise_to_stick_floor(0);
        assert_eq!(floor_bar(&s, 0), None);
        let pct = |f: &str| {
            s.slot(XINPUT, 2).shown().unwrap().setting(f).and_then(serde_json::Value::as_i64)
        };
        assert_eq!((pct("/sticks/left_min_pct"), pct("/sticks/right_min_pct")), (Some(2), Some(1)));
    }
}
