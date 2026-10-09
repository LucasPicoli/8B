//! Closing the window: what a close request does, and the question it asks when
//! some controller holds edits that are not written yet.

use std::rc::Rc;

use slint::{ModelRc, VecModel};

use crate::controllers::Controller;
use crate::review::count;
use crate::state::{AppState, SlotState};
use crate::ui::{AppWindow, CloseGroup, CloseInfo, CloseSlot};

/// What a close request does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseOutcome {
    /// Nothing is lost: the window closes.
    Close,
    /// Edits would be lost: the window stays and asks first.
    Ask,
    /// A write runs: the window stays, and the writing dialog with it.
    Ignore,
}

impl AppState {
    /// What a close request does now.
    #[must_use]
    pub fn close_outcome(&self) -> CloseOutcome {
        if self.write.running.is_some() {
            CloseOutcome::Ignore
        } else if self.controllers.iter().any(Controller::has_edits) {
            CloseOutcome::Ask
        } else {
            CloseOutcome::Close
        }
    }

    /// The window was asked to close. An answer of [`CloseOutcome::Ask`] opens the
    /// question.
    pub fn close_requested(&mut self) -> CloseOutcome {
        let outcome = self.close_outcome();
        self.closing = outcome == CloseOutcome::Ask;
        outcome
    }

    /// The user went back to the window.
    pub const fn cancel_close(&mut self) {
        self.closing = false;
    }

    /// The edits went while the question was open, such as after a read: it closes,
    /// so it cannot come back with the next edit.
    pub fn settle_close(&mut self) {
        if self.close_outcome() != CloseOutcome::Ask {
            self.closing = false;
        }
    }

    /// Whether the question offers "Write n slots…": Write all is on, and no other
    /// dialog is open under the question.
    #[must_use]
    pub fn can_write_before_close(&self) -> bool {
        self.can_write_all()
            && self.pending_import.is_none()
            && self.changed_slots().is_empty()
            && self.move_candidates().is_empty()
    }

    /// "Write n slots…" in the question: it closes, and Write all starts. Returns the
    /// controller to read again before the review opens.
    pub fn write_before_close(&mut self) -> Option<String> {
        if !self.closing || !self.can_write_before_close() {
            return None;
        }
        self.closing = false;
        self.begin_batch()
    }

    /// Each listed controller with edits, with its header label. The shown
    /// controller comes first; the others keep the header's order.
    #[must_use]
    pub fn close_groups(&self) -> Vec<(String, &Controller)> {
        let shown = self.active().map(|c| &c.port);
        let mut groups: Vec<_> = self
            .listed()
            .zip(self.controller_labels())
            .filter(|(c, _)| c.has_edits())
            .map(|(c, label)| (label, c))
            .collect();
        groups.sort_by_key(|(_, c)| Some(&c.port) != shown);
        groups
    }

    /// What the question shows now, if it shows.
    #[must_use]
    pub fn close_info(&self) -> CloseInfo {
        if !self.closing {
            return CloseInfo::default();
        }
        let mut total = 0;
        let groups: Vec<CloseGroup> = self
            .close_groups()
            .into_iter()
            .map(|(label, c)| {
                let slots: Vec<CloseSlot> = self
                    .edited_of(c)
                    .into_iter()
                    .map(|(mode, n)| CloseSlot {
                        title: format!("{} slot {n}", self.description_of(c).mode_label(mode))
                            .into(),
                        name: c
                            .slots
                            .get(&(mode, n))
                            .and_then(SlotState::shown)
                            .map(|p| p.name.as_str())
                            .unwrap_or_default()
                            .into(),
                    })
                    .collect();
                total += slots.len();
                CloseGroup {
                    label: label.into(),
                    slots: ModelRc::from(Rc::new(VecModel::from(slots))),
                }
            })
            .collect();
        CloseInfo {
            shown: !groups.is_empty(),
            total: count(total),
            write: if self.can_write_before_close() { count(self.edited_slots().len()) } else { 0 },
            groups: ModelRc::from(Rc::new(VecModel::from(groups))),
        }
    }
}

/// Pushes the close question.
pub fn render_close(state: &AppState, ui: &AppWindow) {
    ui.set_closing(state.close_info());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {

    use slint::Model as _;

    use super::*;
    use crate::state::tests::{connected, full_read, PORT};

    use controller_core::devices::pro3::{DINPUT, XINPUT};

    /// The rows of each group in the question, as `title · name`, by label.
    fn rows(s: &AppState) -> Vec<(String, Vec<String>)> {
        s.close_info()
            .groups
            .iter()
            .map(|g| {
                let slots = g.slots.iter().map(|r| format!("{} · {}", r.title, r.name)).collect();
                (g.label.to_string(), slots)
            })
            .collect()
    }

    #[test]
    fn no_edits_close_at_once() {
        let mut s = connected(XINPUT);
        assert_eq!(s.close_requested(), CloseOutcome::Close);
        assert!(!s.closing && !s.close_info().shown);
    }

    #[test]
    fn one_edited_slot_asks_and_cancel_keeps_it() {
        let mut s = connected(XINPUT);
        s.select(0, 1);
        s.set_name("Drift");
        assert_eq!(s.close_requested(), CloseOutcome::Ask);
        let info = s.close_info();
        assert!(info.shown);
        assert_eq!((info.total, info.write), (1, 1));
        assert_eq!(
            rows(&s),
            [("Pro 3 · XInput".to_owned(), vec!["XInput slot 2 · Drift".to_owned()])]
        );
        s.cancel_close();
        assert!(!s.close_info().shown);
        assert!(s.slot(XINPUT, 2).unsaved(), "Cancel keeps the edit");
    }

    #[test]
    fn edits_on_an_unplugged_controller_ask_too() {
        let mut s = connected(XINPUT);
        s.set_name("Kept");
        s.presence(PORT, None);
        s.presence("3-2", Some(DINPUT));
        s.read_finished("3-2", Ok(full_read()));
        s.answer_move(None);
        s.pick_controller(1);
        assert!(s.edited_slots().is_empty(), "the shown controller holds no edits");
        assert_eq!(s.close_requested(), CloseOutcome::Ask);
        let info = s.close_info();
        assert_eq!((info.total, info.write), (1, 0), "nothing on the shown pad to write");
        assert_eq!(
            rows(&s),
            [("Pro 3 · Disconnected".to_owned(), vec!["XInput slot 1 · Kept".to_owned()])]
        );
    }

    #[test]
    fn a_running_write_ignores_the_close() {
        let mut s = connected(XINPUT);
        s.set_name("Solo");
        s.begin_review().unwrap();
        s.review_read(PORT, true);
        s.confirm_review().unwrap();
        assert_eq!(s.close_requested(), CloseOutcome::Ignore);
        assert!(!s.close_info().shown);
    }

    #[test]
    fn the_changed_dialog_hides_write_but_still_asks() {
        let mut s = connected(XINPUT);
        s.set_name("Mine");
        let mut read = full_read();
        for p in &mut read.profiles {
            if (p.mode, p.source_slot) == (XINPUT, 1) {
                p.canonical.name = "Theirs".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        assert!(!s.changed_slots().is_empty() && s.can_write_all());
        assert_eq!(s.close_requested(), CloseOutcome::Ask);
        assert_eq!(s.close_info().write, 0);
        assert_eq!(s.write_before_close(), None);
    }

    #[test]
    fn the_shown_controller_comes_first_and_write_counts_only_it() {
        let mut s = connected(XINPUT);
        s.presence("3-2", Some(DINPUT));
        s.read_finished("3-2", Ok(full_read()));
        s.set_name("First");
        s.pick_controller(1);
        s.select(2, 0);
        s.set_name("Second");
        s.select(2, 1);
        s.set_name("Third");
        s.close_requested();
        let info = s.close_info();
        assert_eq!((info.total, info.write), (3, 2));
        assert_eq!(
            rows(&s),
            [
                (
                    "Pro 3 · DInput".to_owned(),
                    vec!["DInput slot 1 · Second".to_owned(), "DInput slot 2 · Third".to_owned()]
                ),
                ("Pro 3 · XInput".to_owned(), vec!["XInput slot 1 · First".to_owned()]),
            ]
        );
    }

    #[test]
    fn write_closes_the_question_and_starts_write_all() {
        let mut s = connected(XINPUT);
        s.set_name("Solo");
        assert_eq!(s.write_before_close(), None, "only from the question");
        s.close_requested();
        assert_eq!(s.write_before_close().as_deref(), Some(PORT));
        assert!(!s.closing && s.checking_batch());
    }

    #[test]
    fn edits_that_go_while_the_question_is_open_close_it() {
        let mut s = connected(XINPUT);
        s.set_name("Solo");
        s.close_requested();
        s.discard();
        s.settle_close();
        assert!(!s.closing);
        s.set_name("Again");
        assert!(!s.close_info().shown, "a new edit does not reopen it");
    }
}
