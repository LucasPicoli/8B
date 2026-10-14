//! The Macros tab: one row per macro slot of the selected slot with the header of its
//! macro and the buttons that start it. Every limit and every button comes from the
//! controller description.

use std::rc::Rc;

use controller_core::model::{MacroDefinition, MacroStep, REPEAT_CONTINUOUS};
use slint::{Model as _, ModelRc, SharedString, VecModel};

use crate::buttons::{keep_same, output_label, push_model, target};
use crate::review::macro_edits::steps;
use crate::state::{AppState, SlotState};
use crate::ui::{AppWindow, MacroRow};

/// The length of the one step a new macro starts with, in milliseconds.
const NEW_STEP_MS: u16 = 100;

/// The first entry of the "Add a button…" picker, which adds nothing.
const ADD_A_BUTTON: &str = "Add a button…";

/// How many button chips a row of the macro slot list shows before "+n".
const ROW_CHIPS: usize = 1;

/// How often a macro plays, in the order the Plays picker lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Plays {
    /// One play per press.
    Once,
    /// A set number of plays per press, at least two.
    Times,
    /// Plays until the button is pressed again.
    UntilPressed,
}

impl Plays {
    /// How `repeat_count` plays.
    #[must_use]
    pub const fn of(repeat_count: u32) -> Self {
        match repeat_count {
            1 => Self::Once,
            REPEAT_CONTINUOUS => Self::UntilPressed,
            _ => Self::Times,
        }
    }

    /// The entry at `index` of the Plays picker.
    #[must_use]
    pub const fn at(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Once),
            1 => Some(Self::Times),
            2 => Some(Self::UntilPressed),
            _ => None,
        }
    }

    /// The index of this entry in the Plays picker.
    const fn index(self) -> i32 {
        match self {
            Self::Once => 0,
            Self::Times => 1,
            Self::UntilPressed => 2,
        }
    }
}

impl SlotState {
    /// The macro in macro slot `n` as shown: the edited one, else the controller's.
    #[must_use]
    pub fn shown_macro(&self, n: u8) -> Option<&MacroDefinition> {
        self.shown_macros().into_iter().find(|m| m.macro_slot == Some(n))
    }

    /// Whether the macro edits change macro slot `n`.
    #[must_use]
    pub fn macro_unsaved(&self, n: u8) -> bool {
        let held = self.held_macros().into_iter().find(|m| m.macro_slot == Some(n));
        self.edited_macros.is_some() && self.shown_macro(n) != held
    }
}

impl AppState {
    /// What `button` is mapped to in the selected slot, when that differs from the
    /// mode's default profile: such a button cannot start a macro.
    #[must_use]
    pub fn mapped_to(&self, button: &str) -> Option<String> {
        let (mode, number) = self.selected_slot()?;
        let slot = self.slot(mode, number);
        let output = target(slot.shown()?, button);
        let default = self.defaults().get(&mode).map_or(button, |p| target(p, button));
        (output != default).then(|| output_label(self.description(), mode, output))
    }

    /// The buttons the "Add a button…" picker of macro slot `n` offers: each that can
    /// start a macro, is not mapped and does not start this one, in the description's
    /// order. A button that starts another macro moves.
    #[must_use]
    pub fn addable_buttons(&self, n: u8) -> Vec<String> {
        let Some((mode, number)) = self.selected_slot() else { return Vec::new() };
        let slot = self.slot(mode, number);
        let own = slot.shown_macro(n).map(|m| m.trigger_buttons.clone()).unwrap_or_default();
        self.description()
            .buttons
            .iter()
            .filter(|b| b.can_start_macro && !own.contains(&b.id))
            .filter(|b| self.mapped_to(&b.id).is_none())
            .map(|b| b.id.clone())
            .collect()
    }

    /// Applies `change` to the macro in macro slot `n` of the selected slot.
    pub(crate) fn edit_macro(&mut self, n: u8, change: impl FnOnce(&mut MacroDefinition)) {
        self.edit_macros(|macros| {
            if let Some(m) = macros.iter_mut().find(|m| m.macro_slot == Some(n)) {
                change(m);
            }
        });
    }

    /// Stages a macro in the empty macro slot `n`: named "Macro n", one empty step,
    /// played once, started by no button.
    pub fn new_macro(&mut self, n: u8) {
        let Some((mode, _)) = self.selected_slot() else { return };
        if n >= self.description().macro_slot_count {
            return;
        }
        let def = MacroDefinition {
            name: format!("Macro {}", u16::from(n) + 1),
            mode,
            trigger_buttons: Vec::new(),
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![MacroStep { duration_ms: NEW_STEP_MS, ..MacroStep::default() }],
            macro_slot: Some(n),
        };
        self.edit_macros(|macros| {
            if macros.iter().all(|m| m.macro_slot != Some(n)) {
                macros.push(def);
                macros.sort_by_key(|m| m.macro_slot);
            }
        });
    }

    /// Stages the delete of the macro in macro slot `n`.
    pub fn delete_macro(&mut self, n: u8) {
        self.edit_macros(|macros| macros.retain(|m| m.macro_slot != Some(n)));
    }

    /// Renames the macro in macro slot `n`, cut to the controller's name length.
    pub fn rename_macro(&mut self, n: u8, name: &str) {
        let max = usize::try_from(self.description().limits.macro_name_length.max).unwrap_or(0);
        let name: String = name.chars().take(max).collect();
        self.edit_macro(n, |m| m.name = name);
    }

    /// Sets how often the macro in macro slot `n` plays. Once drops the pause, and a
    /// number of times starts at two.
    pub fn set_plays(&mut self, n: u8, plays: Plays) {
        let least = self.least_times();
        self.edit_macro(n, |m| match plays {
            Plays::Once => {
                m.repeat_count = 1;
                m.interval_ms = 0;
            }
            Plays::Times if Plays::of(m.repeat_count) != Plays::Times => m.repeat_count = least,
            Plays::Times => {}
            Plays::UntilPressed => m.repeat_count = REPEAT_CONTINUOUS,
        });
    }

    /// The fewest plays "A number of times" offers: two, or more when the controller
    /// asks for more.
    fn least_times(&self) -> u32 {
        u32::try_from(self.description().limits.macro_repeat_count.min.max(2)).unwrap_or(2)
    }

    /// Sets the number of plays of the macro in macro slot `n`, within the controller's
    /// range. Only a macro that plays a number of times takes it.
    pub fn set_play_count(&mut self, n: u8, count: i32) {
        let max = self.description().limits.macro_repeat_count.max;
        let Ok(count) = u32::try_from(count.min(max)) else { return };
        let count = count.max(self.least_times());
        self.edit_macro(n, |m| {
            if Plays::of(m.repeat_count) == Plays::Times {
                m.repeat_count = count;
            }
        });
    }

    /// Sets the interval between plays of the macro in macro slot `n`, within the
    /// controller's range.
    pub fn set_play_pause(&mut self, n: u8, ms: i32) {
        let range = self.description().limits.macro_interval_ms;
        let Ok(ms) = u32::try_from(ms.clamp(range.min, range.max)) else { return };
        self.edit_macro(n, |m| m.interval_ms = ms);
    }

    /// Makes `button` start the macro in macro slot `n`, out of the macro it started.
    /// A mapped button stays as it is.
    pub fn add_macro_button(&mut self, n: u8, button: &str) {
        if self.mapped_to(button).is_none() {
            self.join_macro(button, n);
        }
    }

    /// Takes `button` off the macro in macro slot `n`.
    pub fn remove_macro_button(&mut self, n: u8, button: &str) {
        self.edit_macro(n, |m| m.trigger_buttons.retain(|b| b != button));
    }
}

/// A list model of strings.
fn strings(items: impl IntoIterator<Item = String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(items.into_iter().map(SharedString::from).collect::<VecModel<_>>()))
}

/// One row per macro slot of the selected slot, with the header of its macro. Empty
/// for an empty slot.
#[must_use]
pub fn macro_rows(state: &AppState) -> Vec<MacroRow> {
    let Some((mode, number)) = state.selected_slot() else { return Vec::new() };
    let slot = state.slot(mode, number);
    if slot.shown().is_none() {
        return Vec::new();
    }
    let description = state.description();
    let label = |b: &String| output_label(description, mode, b);
    (0..description.macro_slot_count)
        .map(|n| {
            let Some(m) = slot.shown_macro(n) else {
                return MacroRow {
                    number: i32::from(n) + 1,
                    unsaved: slot.macro_unsaved(n),
                    ..MacroRow::default()
                };
            };
            let addable = state.addable_buttons(n).into_iter().map(|b| {
                slot.macro_started_by(&b).map_or_else(
                    || label(&b),
                    |other| format!("{} (Macro {})", label(&b), u16::from(other) + 1),
                )
            });
            let plays = Plays::of(m.repeat_count);
            MacroRow {
                number: i32::from(n) + 1,
                filled: true,
                name: m.name.as_str().into(),
                steps: steps(m.steps.len()).into(),
                buttons: strings(m.trigger_buttons.iter().map(label)),
                first_buttons: strings(m.trigger_buttons.iter().take(ROW_CHIPS).map(label)),
                more: i32::try_from(m.trigger_buttons.len().saturating_sub(ROW_CHIPS))
                    .unwrap_or(i32::MAX),
                unsaved: slot.macro_unsaved(n),
                plays: plays.index(),
                count: if plays == Plays::Times {
                    i32::try_from(m.repeat_count).unwrap_or(i32::MAX)
                } else {
                    i32::try_from(state.least_times()).unwrap_or(2)
                },
                pause: i32::try_from(m.interval_ms).unwrap_or(i32::MAX),
                addable: strings(std::iter::once(ADD_A_BUTTON.to_owned()).chain(addable)),
            }
        })
        .collect()
}

/// The button at entry `choice` of the "Add a button…" picker of macro slot `n`.
#[must_use]
pub fn added_button(state: &AppState, n: u8, choice: usize) -> Option<String> {
    state.addable_buttons(n).into_iter().nth(choice.checked_sub(1)?)
}

/// The button at chip `chip` of macro slot `n`.
#[must_use]
pub fn chip_button(state: &AppState, n: u8, chip: usize) -> Option<String> {
    let (mode, number) = state.selected_slot()?;
    state.slot(mode, number).shown_macro(n)?.trigger_buttons.get(chip).cloned()
}

/// Pushes the Macros tab of the selected slot.
pub fn render_macros(state: &AppState, ui: &AppWindow) {
    let description = state.description();
    let limits = description.limits;
    ui.set_has_macros(description.macro_slot_count > 0);
    // A row keeps its lists while they hold, so only a changed row is redone.
    let current = ui.get_macro_rows();
    let mut rows = macro_rows(state);
    for (old, row) in current.iter().zip(&mut rows) {
        keep_same(&old.buttons, &mut row.buttons);
        keep_same(&old.first_buttons, &mut row.first_buttons);
        keep_same(&old.addable, &mut row.addable);
    }
    push_model(&current, rows, |m| ui.set_macro_rows(m));
    ui.set_macros_editable(
        state.selected_slot().is_some_and(|(m, n)| state.slot(m, n).macros_editable()),
    );
    ui.set_macro_name_max(limits.macro_name_length.max);
    ui.set_macro_count_min(i32::try_from(state.least_times()).unwrap_or(2));
    ui.set_macro_count_max(limits.macro_repeat_count.max);
    ui.set_macro_pause_max(limits.macro_interval_ms.max);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::XINPUT;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// `XInput` slot 2 selected, holding "Jump" in macro slot 0 on PR and PL, and "Dash"
    /// in macro slot 2 on R4.
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp", "lp"], 12), macro_def(2, "Dash", &["r4"], 1)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        s.select(0, 1);
        s
    }

    fn row(s: &AppState, n: usize) -> MacroRow {
        macro_rows(s)[n].clone()
    }

    fn list(m: &ModelRc<SharedString>) -> Vec<String> {
        m.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn one_row_per_macro_slot_of_the_description() {
        let s = held();
        let rows = macro_rows(&s);
        assert_eq!(rows.len(), usize::from(s.description().macro_slot_count));
        assert_eq!((rows[0].name.as_str(), rows[0].steps.as_str()), ("Jump", "12 steps"));
        assert_eq!(list(&rows[0].buttons), ["PR", "PL"]);
        assert!(!rows[1].filled && rows[1].number == 2);
        assert_eq!((rows[2].steps.as_str(), rows[2].plays), ("1 step", 0));
        assert!(rows.iter().all(|r| !r.unsaved));
    }

    #[test]
    fn a_new_macro_fills_the_empty_row_with_a_name_one_step_and_no_button() {
        let mut s = held();
        s.new_macro(1);
        let r = row(&s, 1);
        assert!(r.filled && r.unsaved);
        assert_eq!((r.name.as_str(), r.steps.as_str(), r.plays), ("Macro 2", "1 step", 0));
        assert_eq!(r.buttons.row_count(), 0);
        let slots: Vec<_> = s.slot(XINPUT, 2).shown_macros().iter().map(|m| m.macro_slot).collect();
        assert_eq!(slots, [Some(0), Some(1), Some(2)], "in macro slot order");
        assert!(s.slot_dirty(XINPUT, 2).macros);
        s.new_macro(1);
        s.new_macro(9);
        assert_eq!(s.slot(XINPUT, 2).shown_macros().len(), 3, "a full or unknown slot stays");
    }

    #[test]
    fn delete_stages_the_removal() {
        let mut s = held();
        s.delete_macro(0);
        assert!(!row(&s, 0).filled && row(&s, 0).unsaved);
        assert_eq!(
            s.review_info((XINPUT, 2)).macro_changes.row_data(0).unwrap(),
            "Jump (Macro 1) removed"
        );
    }

    #[test]
    fn a_rename_is_cut_to_the_limit() {
        let mut s = held();
        s.rename_macro(0, "A name far longer than sixteen");
        assert_eq!(row(&s, 0).name, "A name far longe");
    }

    #[test]
    fn plays_count_and_pause_stay_in_range_and_read_as_the_review_says() {
        let mut s = held();
        s.set_play_count(0, 5);
        assert_eq!(row(&s, 0).count, 2, "once takes no count");
        s.set_plays(0, Plays::Times);
        assert_eq!((row(&s, 0).plays, row(&s, 0).count), (1, 2));
        s.set_play_count(0, 3);
        s.set_play_pause(0, 100);
        let line = s.review_info((XINPUT, 2)).macro_changes.row_data(0).unwrap();
        assert_eq!(line, "Jump (Macro 1) changed: plays once to 3 times, interval 0 to 100 ms");
        s.set_play_count(0, 1000);
        s.set_play_pause(0, -5);
        assert_eq!((row(&s, 0).count, row(&s, 0).pause), (99, 0));
        s.set_plays(0, Plays::UntilPressed);
        assert_eq!(row(&s, 0).plays, 2);
        s.set_plays(0, Plays::Once);
        assert_eq!((row(&s, 0).plays, row(&s, 0).pause), (0, 0));
        assert!(!s.slot(XINPUT, 2).unsaved(), "back to what the controller holds");
    }

    #[test]
    fn add_a_button_offers_free_and_moving_buttons_never_mapped_ones() {
        let mut s = held();
        s.set_output("l4", "bottom face");
        let offered = s.addable_buttons(0);
        assert!(!offered.iter().any(|b| b == "rp" || b == "lp"), "its own buttons");
        assert!(!offered.contains(&"l4".to_owned()), "a mapped button");
        assert!(offered.contains(&"r4".to_owned()) && offered.contains(&"r3".to_owned()));
        let labels = list(&row(&s, 0).addable);
        assert_eq!(labels[0], "Add a button…");
        assert!(labels.contains(&"R4 (Macro 3)".to_owned()));
        assert_eq!(s.mapped_to("l4").as_deref(), Some("A"));
        s.add_macro_button(0, "l4");
        assert_eq!(list(&row(&s, 0).buttons), ["PR", "PL"], "a mapped button stays out");
        let at = labels.iter().position(|l| l == "R4 (Macro 3)").unwrap();
        let r4 = added_button(&s, 0, at).unwrap();
        s.add_macro_button(0, &r4);
        assert_eq!(list(&row(&s, 0).buttons).len(), 3);
        assert_eq!(row(&s, 2).buttons.row_count(), 0, "R4 left Dash");
        assert_eq!(added_button(&s, 0, 0), None, "the first entry adds nothing");
    }

    #[test]
    fn a_chip_removes_its_button() {
        let mut s = held();
        let pl = chip_button(&s, 0, 1).unwrap();
        s.remove_macro_button(0, &pl);
        assert_eq!(list(&row(&s, 0).buttons), ["PR"]);
        assert_eq!(s.slot(XINPUT, 2).macro_started_by("lp"), None);
        assert!(row(&s, 0).unsaved);
    }

    #[test]
    fn a_row_shows_one_chip_then_counts_the_rest() {
        let mut s = held();
        for b in ["r3", "l3", "turbo"] {
            s.add_macro_button(0, b);
        }
        let r = row(&s, 0);
        assert_eq!((r.buttons.row_count(), r.first_buttons.row_count(), r.more), (5, 1, 4));
    }

    #[test]
    fn edits_wait_for_the_steps() {
        let mut s = held();
        s.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().pad_macros[0].loaded = false;
        s.new_macro(1);
        s.rename_macro(0, "Run");
        assert!(!s.slot(XINPUT, 2).unsaved());
    }
}
