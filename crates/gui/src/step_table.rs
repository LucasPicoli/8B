//! The step table of the Macros tab: one row per step of the selected macro, a lane
//! per button the macro holds, the stick glyphs, and the toolbar's step edits. Which
//! buttons can be in a step, which of them a trigger value holds, and every limit come
//! from the controller description.

use std::rc::Rc;

use controller_core::description::{Button, TriggerKind, TriggerSide};
use controller_core::model::glide::expanded_len;
use controller_core::model::{GlideControl, MacroStep, Mode};
use slint::{Model as _, ModelRc, SharedString, VecModel};
use tr::tr;

use crate::buttons::{keep_same, push_model};
use crate::language::prose;
use crate::macro_steps::{add_after, delete, duplicate, move_step, split};
use crate::render::and_list;
use crate::review::count;
use crate::state::AppState;
use crate::ui::{AppWindow, LaneInfo, StepCount, StepInfo, StickInfo};

/// A stick at rest, on each axis.
const REST: u8 = 127;

/// The width of a held button's bar, and the narrowest and widest trigger bar, in
/// logical pixels.
const BAR: f32 = 10.0;
const TRIGGER_BAR_MIN: f32 = 3.0;
const TRIGGER_BAR_MAX: f32 = 12.0;

/// A toolbar edit of the step table, in the order the markup numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepEdit {
    /// Add step, after the selected one.
    Add,
    /// Duplicate the selected step.
    Duplicate,
    /// Split the selected step in two halves.
    Split,
    /// Move the selected step up.
    Up,
    /// Move the selected step down.
    Down,
    /// Delete the selected step.
    Delete,
}

impl StepEdit {
    /// The edit the markup numbers `index`.
    #[must_use]
    pub const fn at(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Add),
            1 => Some(Self::Duplicate),
            2 => Some(Self::Split),
            3 => Some(Self::Up),
            4 => Some(Self::Down),
            5 => Some(Self::Delete),
            _ => None,
        }
    }

    /// Whether the edit adds steps, so a full macro refuses it.
    const fn grows(self) -> bool {
        matches!(self, Self::Add | Self::Duplicate | Self::Split)
    }
}

/// The trigger value of `side` in `step`.
const fn trigger(step: &MacroStep, side: TriggerSide) -> u8 {
    match side {
        TriggerSide::Left => step.trigger_left,
        TriggerSide::Right => step.trigger_right,
    }
}

/// The glide control of trigger `side`.
pub const fn trigger_control(side: TriggerSide) -> GlideControl {
    match side {
        TriggerSide::Left => GlideControl::LeftTrigger,
        TriggerSide::Right => GlideControl::RightTrigger,
    }
}

/// Whether `step` holds `button`: among its pressed buttons, or in its trigger value. A
/// trigger that glides from 0 holds it too, as it presses on the way.
fn holds(step: &MacroStep, button: &Button) -> bool {
    button.step_trigger_value.map_or_else(
        || step.pressed_buttons.contains(&button.id),
        |side| trigger(step, side) > 0 || step.glide.contains(&trigger_control(side)),
    )
}

/// How many step edits Ctrl+Z can take back, over every macro.
// ponytail: whole step lists per edit, about 30 KB each at 408 steps; store diffs if
// memory ever matters.
const UNDO_DEPTH: usize = 100;

/// A step edit, so Ctrl+Z can take it back: the macro's steps before and after it, and
/// the step that was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepUndo {
    /// The slot and the macro slot of the macro.
    slot: (Mode, u8),
    macro_slot: u8,
    before: Vec<MacroStep>,
    after: Vec<MacroStep>,
    selected: usize,
    /// Edits with the same key in a row are one undo, such as typing a duration.
    key: Option<(usize, &'static str)>,
}

impl AppState {
    /// The steps of the macro in macro slot `n` of the selected slot, once they are read.
    #[must_use]
    pub fn macro_steps(&self, n: u8) -> Option<Vec<MacroStep>> {
        let (mode, number) = self.selected_slot()?;
        let slot = self.slot(mode, number);
        let loaded = slot.edited_macros.is_some()
            || slot.pad_macros.iter().filter(|m| m.def.macro_slot == Some(n)).all(|m| m.loaded);
        loaded.then(|| slot.shown_macro(n).map(|m| m.steps.clone())).flatten()
    }

    /// How many steps `steps` take on the controller, once each glide is expanded.
    #[must_use]
    pub fn pad_steps(&self, steps: &[MacroStep]) -> usize {
        expanded_len(steps, self.description().limits.glide_step_ms)
    }

    /// Whether `steps` fill the controller's step limit.
    fn steps_full(&self, steps: &[MacroStep]) -> bool {
        let max = usize::try_from(self.description().limits.macro_steps.max).unwrap_or(0);
        self.pad_steps(steps) >= max
    }

    /// Applies `edit` to step `step` of the macro in macro slot `n`. Returns the step to
    /// select then, or `None` when nothing changed: the steps are still being read, or
    /// the macro is full and the edit adds steps.
    pub fn edit_step(&mut self, n: u8, step: usize, edit: StepEdit) -> Option<usize> {
        let steps = self.macro_steps(n)?;
        if edit.grows() && self.steps_full(&steps) {
            return None;
        }
        let mut selected = None;
        self.edit_macro(n, |m| {
            let s = &mut m.steps;
            selected = Some(match edit {
                StepEdit::Add => add_after(s, step),
                StepEdit::Duplicate => duplicate(s, step),
                StepEdit::Split => split(s, step),
                StepEdit::Up => move_step(s, step, false),
                StepEdit::Down => move_step(s, step, true),
                StepEdit::Delete => delete(s, step),
            });
        });
        self.record_step_edit(n, steps, step, None);
        selected
    }

    /// Puts the edit that turned `before` into the steps macro slot `n` holds now into
    /// the undo history, with step `selected` to select again. An edit with the `key`
    /// of the latest one, right after it, joins it.
    pub(crate) fn record_step_edit(
        &mut self,
        n: u8,
        before: Vec<MacroStep>,
        selected: usize,
        key: Option<(usize, &'static str)>,
    ) {
        let (Some(after), Some(slot)) = (self.macro_steps(n), self.selected_slot()) else {
            return;
        };
        if after == before {
            return;
        }
        if let Some(last) = self.step_undo.last_mut().filter(|u| {
            key.is_some()
                && u.key == key
                && u.slot == slot
                && u.macro_slot == n
                && u.after == before
        }) {
            last.after = after;
            return;
        }
        if self.step_undo.len() >= UNDO_DEPTH {
            self.step_undo.remove(0);
        }
        self.step_undo.push(StepUndo { slot, macro_slot: n, before, after, selected, key });
    }

    /// Where the latest step edit of macro slot `n` of the selected slot sits in the
    /// history, when the steps are still as it left them.
    fn undoable(&self, n: u8) -> Option<usize> {
        let slot = self.selected_slot()?;
        let at = self.step_undo.iter().rposition(|u| u.slot == slot && u.macro_slot == n)?;
        let steps = self.macro_steps(n)?;
        self.step_undo.get(at).filter(|u| u.after == steps).map(|_| at)
    }

    /// Whether Ctrl+Z can take back a step edit of macro slot `n`: its latest one, while
    /// the steps are still as that edit left them. Another change to the steps, such as
    /// Discard, ends the history of that macro.
    #[must_use]
    pub fn can_undo_step(&self, n: u8) -> bool {
        self.undoable(n).is_some()
    }

    /// Takes back the latest step edit of macro slot `n`, so each Ctrl+Z goes one edit
    /// further back. Returns the step to select, or `None` when there is nothing to take
    /// back.
    pub fn undo_step(&mut self, n: u8) -> Option<usize> {
        let undo = self.step_undo.remove(self.undoable(n)?);
        self.edit_macro(n, |m| m.steps = undo.before);
        Some(undo.selected)
    }
}

/// `x` from 0 to 255 as -1 to 1, rest at 0.
fn unit(x: u8) -> f32 {
    let d = f32::from(x) - f32::from(REST);
    if x < REST {
        d / f32::from(REST)
    } else {
        d / (255.0 - f32::from(REST))
    }
}

/// A stick position on the disc: the square of values mapped to the round gate, so a
/// corner such as (255, 0) sits on the rim.
pub fn disc(x: u8, y: u8) -> (f32, f32) {
    let (u, v) = (unit(x), unit(y));
    (u * (1.0 - v * v / 2.0).sqrt(), v * (1.0 - u * u / 2.0).sqrt())
}

/// The stick cell of `control` in step `i` of `steps`.
pub fn stick(steps: &[MacroStep], i: usize, control: GlideControl) -> StickInfo {
    let Some(step) = steps.get(i) else { return StickInfo::default() };
    let [x, y] = controller_core::model::glide::axes(step, control);
    let next = steps.get(i + 1).filter(|_| step.glide.contains(&control));
    let to = next.map(|n| controller_core::model::glide::axes(n, control));
    let (dx, dy) = disc(x, y);
    let (tx, ty) = to.map_or((dx, dy), |[a, b]| disc(a, b));
    StickInfo {
        shown: [x, y] != [REST, REST] || to.is_some(),
        x: dx,
        y: dy,
        glide: to.is_some(),
        to_x: tx,
        to_y: ty,
        text: to
            .map_or_else(
                || tr!("x {x}, y {y}", x = x, y = y),
                |[a, b]| tr!("x {x}, y {y} to x {a}, y {b}", x = x, y = y, a = a, b = b),
            )
            .into(),
    }
}

/// The width of a held bar for `button` in `step` of a mode whose triggers read as
/// `kind`: a trigger's pressure in an analog mode, else the full bar.
fn bar(step: &MacroStep, button: &Button, kind: TriggerKind) -> f32 {
    match (button.step_trigger_value, kind) {
        (Some(side), TriggerKind::Analog) => {
            let v = f32::from(trigger(step, side)) / 255.0;
            (TRIGGER_BAR_MAX - TRIGGER_BAR_MIN).mul_add(v, TRIGGER_BAR_MIN)
        }
        _ => BAR,
    }
}

/// How `step` holds `button`, named `label`: the label, with the pressure in an analog
/// mode, and where a gliding pressure goes in `next`. Such as "A" or "LT 128 to 255".
fn held_text(
    step: &MacroStep,
    next: Option<&MacroStep>,
    button: &Button,
    label: &str,
    kind: TriggerKind,
) -> String {
    let Some(side) = button.step_trigger_value.filter(|_| kind == TriggerKind::Analog) else {
        return label.to_owned();
    };
    let now = trigger(step, side);
    next.filter(|_| step.glide.contains(&trigger_control(side))).map_or_else(
        || tr!("{button} {now}", button = label, now = now),
        |n| tr!("{button} {now} to {next}", button = label, now = now, next = trigger(n, side)),
    )
}

/// Whether a step holds nothing: no button, no trigger, both sticks at rest, and nothing
/// gliding away from there.
fn is_pause(step: &MacroStep) -> bool {
    step.glide.is_empty()
        && step.pressed_buttons.is_empty()
        && step.trigger_left == 0
        && step.trigger_right == 0
        && [step.left_stick_x, step.left_stick_y, step.right_stick_x, step.right_stick_y]
            .iter()
            .all(|&v| v == REST)
}

/// The step table of `steps` in `mode`: the lane buttons, in the description's order,
/// and one row per step.
#[must_use]
pub fn step_rows(
    state: &AppState,
    mode: Mode,
    steps: &[MacroStep],
) -> (Vec<String>, Vec<StepInfo>) {
    let description = state.description();
    let kind = description.mode(mode).map_or(TriggerKind::OnOff, |m| m.trigger_kind);
    let lanes: Vec<&Button> = description
        .buttons
        .iter()
        .filter(|b| b.can_be_in_macro_step && steps.iter().any(|s| holds(s, b)))
        .collect();
    let mut at: u32 = 0;
    let rows = steps
        .iter()
        .enumerate()
        .map(|(i, step)| {
            let start = at;
            at += u32::from(step.duration_ms);
            let next = steps.get(i + 1);
            let held = |s: Option<&MacroStep>, b: &Button| s.is_some_and(|s| holds(s, b));
            let cells: Vec<LaneInfo> = lanes
                .iter()
                .map(|b| {
                    let top = bar(step, b, kind);
                    let glides = b
                        .step_trigger_value
                        .is_some_and(|side| step.glide.contains(&trigger_control(side)));
                    let on = holds(step, b);
                    LaneInfo {
                        held: on,
                        joins_up: held(i.checked_sub(1).and_then(|p| steps.get(p)), b),
                        joins_down: held(next, b),
                        top,
                        bottom: next.filter(|_| glides).map_or(top, |n| bar(n, b, kind)),
                        tip: if on {
                            let label = b.labels.get(&mode).map_or("", String::as_str);
                            held_text(step, next, b, label, kind).into()
                        } else {
                            "".into()
                        },
                    }
                })
                .collect();
            let held_list: Vec<String> = lanes
                .iter()
                .filter(|b| holds(step, b))
                .map(|b| held_text(step, next, b, &prose(b.spoken_label(mode)), kind))
                .collect();
            StepInfo {
                number: i32::try_from(i + 1).unwrap_or(i32::MAX),
                starts_at: tr!("{n} ms", n = start).into(),
                duration: tr!("{n} ms", n = step.duration_ms).into(),
                duration_ms: i32::from(step.duration_ms),
                pause: is_pause(step),
                held: and_list(&held_list).into(),
                lanes: ModelRc::from(Rc::new(VecModel::from(cells))),
                left: stick(steps, i, GlideControl::LeftStick),
                right: stick(steps, i, GlideControl::RightStick),
            }
        })
        .collect();
    let labels = lanes.iter().map(|b| b.labels.get(&mode).cloned().unwrap_or_default()).collect();
    (labels, rows)
}

/// The numbers of the line under the step table. `.slint` words them, as
/// "12/408 steps · 1.20 s", and writes the seconds in the language's decimal form.
#[must_use]
pub fn step_count(state: &AppState, steps: &[MacroStep]) -> StepCount {
    let total: u32 = steps.iter().map(|s| u32::from(s.duration_ms)).sum();
    StepCount {
        steps: count(steps.len()),
        pad_steps: count(state.pad_steps(steps)),
        max: state.description().limits.macro_steps.max,
        total_ms: i32::try_from(total).unwrap_or(i32::MAX),
    }
}

/// Pushes the step table of the macro the Macros tab shows.
pub fn render_steps(state: &AppState, ui: &AppWindow) {
    let n = u8::try_from(ui.get_selected_macro()).unwrap_or(0);
    let steps = state.macro_steps(n);
    let mode = state.selected_slot().map(|(m, _)| m);
    let (lanes, rows) = match (&steps, mode) {
        (Some(steps), Some(mode)) => step_rows(state, mode, steps),
        _ => (Vec::new(), Vec::new()),
    };
    ui.set_steps_ready(steps.is_some());
    ui.set_steps_full(steps.as_ref().is_some_and(|s| state.steps_full(s)));
    ui.set_step_count(steps.as_ref().map(|s| step_count(state, s)).unwrap_or_default());
    let lanes = lanes.into_iter().map(SharedString::from).collect();
    push_model(&ui.get_step_lanes(), lanes, |m| ui.set_step_lanes(m));
    // A row keeps its lanes model while the lanes hold, so only a changed row is redone.
    let current = ui.get_step_rows();
    let mut rows = rows;
    for (old, row) in current.iter().zip(&mut rows) {
        keep_same(&old.lanes, &mut row.lanes);
    }
    push_model(&current, rows, |m| ui.set_step_rows(m));
    ui.set_can_undo_step(state.can_undo_step(n));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::float_cmp)]
mod tests {
    use controller_core::devices::pro3::{SWITCH, XINPUT};

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
    fn a_glide_from_rest_is_held_and_not_a_pause() {
        let up = MacroStep { glide: [GlideControl::LeftTrigger].into(), ..step(100) };
        let full = MacroStep { trigger_left: 255, ..step(100) };
        let away = MacroStep { glide: [GlideControl::RightStick].into(), ..step(100) };
        let moved = MacroStep { right_stick_x: 255, ..step(100) };
        let s = holding(vec![up, full, away, moved]);
        let (labels, rows) = step_rows(&s, XINPUT, &s.macro_steps(0).unwrap());
        assert_eq!(labels, ["LT"]);
        let lt = rows[0].lanes.row_data(0).unwrap();
        assert!(lt.held, "LT glides up from 0");
        assert!((lt.top - TRIGGER_BAR_MIN).abs() < f32::EPSILON && lt.bottom > lt.top);
        assert_eq!(lt.tip, "LT 0 to 255");
        assert_eq!(rows[0].held, "LT 0 to 255");
        assert!(!rows[0].pause && !rows[2].pause, "both glide away from rest");
    }

    #[test]
    fn a_row_speaks_symbol_labels_and_draws_the_symbols() {
        let held = |b: &[&str]| MacroStep {
            pressed_buttons: b.iter().map(|&b| b.to_owned()).collect(),
            ..step(100)
        };
        let s = holding(vec![held(&["select/back", "d-pad up"])]);
        let (lanes, rows) = step_rows(&s, SWITCH, &s.macro_steps(0).unwrap());
        assert_eq!(rows[0].held, "Minus and D-pad up");
        assert_eq!(lanes.iter().filter(|l| ["−", "↑"].contains(&l.as_str())).count(), 2);
        assert_eq!(rows[0].lanes.row_data(0).unwrap().tip.as_str(), lanes[0]);
    }

    #[test]
    fn rows_have_lanes_for_held_buttons_and_trigger_values_only() {
        let a = MacroStep { pressed_buttons: vec!["bottom face".to_owned()], ..step(100) };
        let lt = MacroStep { trigger_left: 255, ..a.clone() };
        let half =
            MacroStep { trigger_left: 51, glide: [GlideControl::LeftTrigger].into(), ..step(200) };
        let s = holding(vec![a, lt, half, step(50)]);
        let steps = s.macro_steps(0).unwrap();
        let (lanes, rows) = step_rows(&s, XINPUT, &steps);
        assert_eq!(lanes, ["A", "LT"], "the description's order, the mode's labels");
        assert_eq!(
            rows.iter().map(|r| r.starts_at.to_string()).collect::<Vec<_>>(),
            ["0 ms", "100 ms", "200 ms", "400 ms"]
        );
        let a_lane: Vec<_> = rows.iter().map(|r| r.lanes.row_data(0).unwrap()).collect();
        assert!(a_lane[0].held && a_lane[0].joins_down && !a_lane[0].joins_up);
        assert!(a_lane[1].joins_up && !a_lane[1].joins_down && !a_lane[2].held);
        let lt_lane = rows[2].lanes.row_data(1).unwrap();
        assert!((lt_lane.top - (TRIGGER_BAR_MIN + 1.8)).abs() < 1e-4, "51 of 255");
        assert_eq!(lt_lane.bottom, TRIGGER_BAR_MIN, "it glides to 0");
        assert!(rows[3].pause && !rows[2].pause);
        assert_eq!((rows[1].held.as_str(), rows[2].held.as_str()), ("A and LT 255", "LT 51 to 0"));
        assert_eq!(rows[2].lanes.row_data(1).unwrap().tip, "LT 51 to 0");
        let (_, switch) = step_rows(&s, SWITCH, &steps);
        assert_eq!(switch[2].lanes.row_data(1).unwrap().top, BAR, "on or off");
    }

    #[test]
    fn a_stick_shows_where_it_is_and_where_it_glides() {
        let corner = MacroStep {
            left_stick_x: 255,
            left_stick_y: 0,
            glide: [GlideControl::LeftStick].into(),
            ..step(100)
        };
        let s = holding(vec![corner, step(100)]);
        let (_, rows) = step_rows(&s, XINPUT, &s.macro_steps(0).unwrap());
        let left = &rows[0].left;
        assert!(left.shown && left.glide);
        assert!((left.x - 0.707).abs() < 0.01 && (left.y + 0.707).abs() < 0.01, "on the rim");
        assert_eq!((left.to_x, left.to_y), (0.0, 0.0));
        assert_eq!(left.text, "x 255, y 0 to x 127, y 127");
        assert!(!rows[0].right.shown && !rows[1].left.shown, "at rest");
    }

    #[test]
    fn the_count_shows_the_controller_steps_once_a_glide_expands() {
        let glide =
            MacroStep { left_stick_x: 0, glide: [GlideControl::LeftStick].into(), ..step(100) };
        let s = holding(vec![step(100), step(100)]);
        let counts = |c: StepCount| (c.steps, c.pad_steps, c.max, c.total_ms);
        assert_eq!(counts(step_count(&s, &s.macro_steps(0).unwrap())), (2, 2, 408, 200));
        let s = holding(vec![glide, step(100)]);
        assert_eq!(counts(step_count(&s, &s.macro_steps(0).unwrap())), (2, 6, 408, 200));
    }

    #[test]
    fn edits_wait_for_the_steps_and_a_full_macro_refuses_to_grow() {
        let mut s = holding(vec![step(150), step(200)]);
        assert_eq!(s.edit_step(0, 0, StepEdit::Add), Some(1));
        assert_eq!(s.macro_steps(0).unwrap().len(), 3);
        assert_eq!(s.edit_step(0, 1, StepEdit::Up), Some(0));
        assert!(s.slot(XINPUT, 2).macro_unsaved(0));
        // Ctrl+Z takes back the move, then the add, then nothing.
        assert!(s.can_undo_step(0) && !s.can_undo_step(1));
        assert_eq!(s.undo_step(0), Some(1));
        assert_eq!(s.macro_steps(0).unwrap().len(), 3, "the add stays");
        assert_eq!(s.undo_step(0), Some(0));
        assert_eq!(s.macro_steps(0).unwrap(), [step(150), step(200)]);
        assert_eq!(s.undo_step(0), None);
        assert!(!s.slot(XINPUT, 2).macro_unsaved(0), "back to what the controller holds");
        // Discard ends the history: the steps no longer match it.
        s.edit_step(0, 0, StepEdit::Add);
        s.discard();
        assert!(!s.can_undo_step(0));
        let mut full = holding(vec![step(1); 408]);
        assert_eq!(full.edit_step(0, 0, StepEdit::Duplicate), None);
        assert_eq!(full.edit_step(0, 0, StepEdit::Delete), Some(0));
        full.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().edited_macros = None;
        full.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().pad_macros[0].loaded =
            false;
        assert_eq!(full.macro_steps(0), None, "still being read");
        assert_eq!(full.edit_step(0, 0, StepEdit::Delete), None);
    }
}
