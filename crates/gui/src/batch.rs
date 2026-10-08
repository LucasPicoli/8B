//! Writing every edited slot of the shown controller in one run: the review of all
//! of them, then one worker command that writes them together. The controller is read
//! again before the review and once after the run.

use std::collections::BTreeSet;
use std::rc::Rc;

use controller_core::model::Mode;
use slint::{Model as _, ModelRc, VecModel};

use crate::controllers::Controller;
use crate::review::{count, WriteFailure};
use crate::state::{AppState, Notice, SlotState};
use crate::ui::{AppWindow, BatchInfo, BatchSlot};
use crate::writes::WriteJob;

/// A slot of the shown controller: its mode and 1-based number.
type Key = (Mode, u8);

/// The values of `BatchInfo.stage`; 0 is no dialog.
const STAGE_REVIEW: i32 = 1;
const STAGE_WRITING: i32 = 2;
const STAGE_FAILED: i32 = 3;

/// A write of every edited slot, from the press of Write all to its end.
#[derive(Debug)]
pub struct Batch {
    /// The USB port path of the controller.
    pub port: String,
    /// How far the run is.
    pub stage: Stage,
}

/// Where a [`Batch`] stands.
#[derive(Debug)]
pub enum Stage {
    /// The controller is being read again. The review opens when the read is back.
    Reading,
    /// The review waits for a yes. It lists the slots that are still edited when it
    /// renders, so dropping edits shrinks it. `flipped` holds the rows the user opened
    /// or closed from how they started.
    Review {
        /// The rows whose state is the opposite of their starting one.
        flipped: BTreeSet<Key>,
    },
    /// The slots are being written in one run. The list is fixed.
    Running {
        /// The write of each slot, in order.
        jobs: Vec<WriteJob>,
        /// Where the run starts: an index into `jobs`. The slots before it were
        /// written by an earlier run, and a retry starts at the slot that failed.
        at: usize,
    },
    /// The write at `at` failed, and the slots after it were not written.
    Failed {
        /// The write of each slot, in order.
        jobs: Vec<WriteJob>,
        /// The write that failed: an index into `jobs`.
        at: usize,
        /// What failed.
        failure: WriteFailure,
    },
}

/// `1 slot` or `n slots`.
fn slots(n: usize) -> String {
    if n == 1 {
        "1 slot".to_owned()
    } else {
        format!("{n} slots")
    }
}

/// The title of the slot `job` writes, such as `XInput slot 2`.
fn title(job: &WriteJob) -> String {
    format!("{} slot {}", job.mode.label(), job.slot.get())
}

impl AppState {
    /// Every slot of the shown controller with unsaved edits, in the sidebar's order:
    /// by mode, then by number.
    #[must_use]
    pub fn edited_slots(&self) -> Vec<Key> {
        self.active().map(|c| self.edited_of(c)).unwrap_or_default()
    }

    /// Every slot of controller `c` with unsaved edits, in the sidebar's order.
    #[must_use]
    pub fn edited_of(&self, c: &Controller) -> Vec<Key> {
        let d = self.description();
        d.modes
            .iter()
            .flat_map(|m| (1..=d.slot_count).map(move |n| (m.id, n)))
            .filter(|key| c.slots.get(key).is_some_and(SlotState::unsaved))
            .collect()
    }

    /// Whether Write all is on: the shown controller can take a write and holds edits.
    #[must_use]
    pub fn can_write_all(&self) -> bool {
        self.idle().is_some() && !self.edited_slots().is_empty()
    }

    /// Whether the controller is being read again for the batch.
    #[must_use]
    pub const fn checking_batch(&self) -> bool {
        matches!(self.write.batch, Some(Batch { stage: Stage::Reading, .. }))
    }

    /// Write all was pressed. Returns the controller to read again before the review
    /// opens.
    pub fn begin_batch(&mut self) -> Option<String> {
        let port = self.idle()?.port.clone();
        if self.edited_slots().is_empty() {
            return None;
        }
        self.write.batch = Some(Batch { port: port.clone(), stage: Stage::Reading });
        Some(port)
    }

    /// A read of `port` came back. If the batch waits on it, its review opens now, or
    /// the batch ends if the read failed.
    pub fn batch_read(&mut self, port: &str, ok: bool) {
        let waiting = self
            .write
            .batch
            .as_ref()
            .is_some_and(|b| b.port == port && matches!(b.stage, Stage::Reading));
        if waiting {
            self.write.batch = ok.then(|| Batch {
                port: port.to_owned(),
                stage: Stage::Review { flipped: BTreeSet::new() },
            });
            self.settle_batch();
        }
    }

    /// The rows of the review the user flipped, once nothing else needs an answer
    /// first: no slot changed under its edits, and some slot still holds edits.
    fn batch_review(&self) -> Option<&BTreeSet<Key>> {
        let Some(Batch { port, stage: Stage::Review { flipped } }) = &self.write.batch else {
            return None;
        };
        let shown = self.active().is_some_and(|c| &c.port == port);
        (shown && self.changed_slots().is_empty() && !self.edited_slots().is_empty())
            .then_some(flipped)
    }

    /// Whether the batch review is open for an answer.
    pub fn batch_review_open(&self) -> bool {
        self.batch_review().is_some()
    }

    /// The edits are gone, such as after "Take the controller's": a waiting review
    /// ends.
    pub fn settle_batch(&mut self) {
        let waiting = matches!(self.write.batch, Some(Batch { stage: Stage::Review { .. }, .. }));
        if waiting && self.changed_slots().is_empty() && self.edited_slots().is_empty() {
            self.write.batch = None;
        }
    }

    /// Another controller was picked: a batch that only waits for a read or a yes
    /// is about the one that left.
    pub fn drop_waiting_batch(&mut self) {
        if matches!(
            self.write.batch,
            Some(Batch { stage: Stage::Reading | Stage::Review { .. }, .. })
        ) {
            self.write.batch = None;
        }
    }

    /// The user closed the review or the failure. The edits that were not written stay.
    pub fn end_batch(&mut self) {
        if matches!(
            self.write.batch,
            Some(Batch { stage: Stage::Review { .. } | Stage::Failed { .. }, .. })
        ) {
            self.write.batch = None;
        }
    }

    /// Opens or closes row `row` of the review.
    pub fn toggle_batch_slot(&mut self, row: usize) {
        let Some(key) = self.edited_slots().get(row).copied() else { return };
        if let Some(Batch { stage: Stage::Review { flipped }, .. }) = &mut self.write.batch {
            if !flipped.remove(&key) {
                flipped.insert(key);
            }
        }
    }

    /// The user said yes to the review. Fixes the list of slots and returns the writes
    /// to send.
    pub fn confirm_batch(&mut self) -> Option<(String, Vec<WriteJob>)> {
        self.batch_review()?;
        let port = self.write.batch.take()?.port;
        let jobs: Vec<WriteJob> = self
            .edited_slots()
            .into_iter()
            .map(|key| self.upload_job(key))
            .collect::<Option<_>>()?;
        if jobs.is_empty() {
            return None;
        }
        self.write.batch =
            Some(Batch { port: port.clone(), stage: Stage::Running { jobs: jobs.clone(), at: 0 } });
        Some(self.start(port, jobs))
    }

    /// The writes came back and [`AppState::write_finished`] took them in. A failure
    /// moves the batch to its failure, at the first slot that failed, and a success
    /// ends it. Then the caller reads the controller.
    pub fn batch_after_write(&mut self) {
        if !matches!(self.write.batch, Some(Batch { stage: Stage::Running { .. }, .. })) {
            return;
        }
        let Some(Batch { port, stage: Stage::Running { jobs, at } }) = self.write.batch.take()
        else {
            return;
        };
        if let Some(failure) = self.write.failed.take() {
            // The notice of the slots written before is not the whole story.
            self.notice = None;
            let failed_at = jobs
                .iter()
                .skip(at)
                .position(|j| (j.mode, j.slot) == (failure.job.mode, failure.job.slot))
                .map_or(at, |i| at.saturating_add(i));
            self.write.batch =
                Some(Batch { port, stage: Stage::Failed { jobs, at: failed_at, failure } });
            return;
        }
        self.notice = Some(Notice {
            error: false,
            title: format!("Wrote {} to the controller.", slots(jobs.len())),
            body: "The controller uses a profile while its slot’s light is on.".to_owned(),
        });
    }

    /// "Write again" after a failure: the failed slot and the slots after it go in one
    /// run, with no new review. Returns the writes to send.
    pub fn retry_batch(&mut self) -> Option<(String, Vec<WriteJob>)> {
        if !matches!(self.write.batch, Some(Batch { stage: Stage::Failed { .. }, .. })) {
            return None;
        }
        let Batch { port, stage: Stage::Failed { jobs, at, .. } } = self.write.batch.take()? else {
            return None;
        };
        let rest = jobs.get(at..).filter(|r| !r.is_empty())?.to_vec();
        self.write.batch = Some(Batch { port: port.clone(), stage: Stage::Running { jobs, at } });
        Some(self.start(port, rest))
    }

    /// The review of the slots that are still edited. A row starts open for the first
    /// slot and for each slot whose write removes a macro, so the user never misses
    /// what a write destroys.
    fn batch_review_info(&self, flipped: &BTreeSet<Key>) -> BatchInfo {
        let mut info = BatchInfo { stage: STAGE_REVIEW, ..BatchInfo::default() };
        let mut rows = Vec::new();
        for (i, key) in self.edited_slots().into_iter().enumerate() {
            let review = self.review_info(key);
            let removes = review.removed.row_count() > 0 || review.leftover > 0;
            info.any_new |= review.is_new;
            info.any_removed |= removes;
            if !review.is_new {
                info.total_changes =
                    info.total_changes.saturating_add(count(review.changes.row_count()));
            }
            rows.push(BatchSlot {
                title: review.slot_title,
                name: review.name,
                changes: review.changes,
                is_new: review.is_new,
                removed: review.removed,
                leftover: review.leftover,
                open: flipped.contains(&key) != (i == 0 || removes),
            });
        }
        info.slots = ModelRc::from(Rc::new(VecModel::from(rows)));
        info
    }

    /// What the batch dialog shows now, if any does.
    #[must_use]
    pub fn batch_info(&self) -> BatchInfo {
        let listed = |jobs: &[WriteJob]| {
            let rows: Vec<BatchSlot> = jobs
                .iter()
                .map(|j| BatchSlot { title: title(j).into(), ..BatchSlot::default() })
                .collect();
            ModelRc::from(Rc::new(VecModel::from(rows)))
        };
        match &self.write.batch {
            Some(Batch { stage: Stage::Running { jobs, at }, .. }) => BatchInfo {
                stage: STAGE_WRITING,
                slots: listed(jobs),
                at: count(*at),
                ..BatchInfo::default()
            },
            Some(Batch { stage: Stage::Failed { jobs, at, failure }, .. }) => BatchInfo {
                stage: STAGE_FAILED,
                slots: listed(jobs),
                at: count(*at),
                detail: failure.detail.as_str().into(),
                backup_path: failure.backup.as_deref().unwrap_or_default().into(),
                ..BatchInfo::default()
            },
            _ => self
                .batch_review()
                .map(|flipped| self.batch_review_info(flipped))
                .unwrap_or_default(),
        }
    }
}

/// Pushes the sidebar bar and the batch dialog.
pub fn render_batch(state: &AppState, ui: &AppWindow) {
    ui.set_edited_count(count(state.edited_slots().len()));
    ui.set_can_write_all(state.can_write_all());
    ui.set_checking_all(state.checking_batch());
    ui.set_batch(state.batch_info());
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use controller_core::model::WriteResult;

    use super::*;
    use crate::state::tests::{connected, full_read, PORT};
    use crate::writes::WriteOp;

    /// A pad in `XInput` with edits in `XInput` slot 2, `XInput` slot 3 (empty on the
    /// pad, started from default) and `DInput` slot 1, in the order the sidebar lists
    /// them.
    fn edited() -> AppState {
        let mut s = connected(Mode::XInput);
        s.select(0, 1);
        s.set_name("Two");
        s.select(0, 2);
        s.start_from_default();
        s.select(2, 0);
        s.set_name("Retro");
        s.select(0, 0);
        s
    }

    /// `edited`, with Write all pressed and the controller read again.
    fn reviewing() -> AppState {
        let mut s = edited();
        s.begin_batch().unwrap();
        s.read_started(PORT);
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        s
    }

    /// The slot a job writes, as `mode slot`.
    fn target(job: &WriteJob) -> (Mode, u8) {
        (job.mode, job.slot.get())
    }

    fn targets(jobs: &[WriteJob]) -> Vec<(Mode, u8)> {
        jobs.iter().map(target).collect()
    }

    fn ok(job: &WriteJob) -> WriteResult {
        WriteResult::success(job.mode, job.slot, "done")
    }

    fn oks(jobs: &[WriteJob]) -> Vec<WriteResult> {
        jobs.iter().map(ok).collect()
    }

    #[test]
    fn a_batch_of_empty_slots_skips_its_review_and_a_mixed_one_keeps_it() {
        let mut s = connected(Mode::XInput);
        s.select(0, 2);
        s.start_from_default();
        s.select(2, 2);
        s.start_from_default();
        s.begin_batch().unwrap();
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        let (_, jobs) = s.skip_empty_review().unwrap();
        assert_eq!(targets(&jobs), [(Mode::XInput, 3), (Mode::DInput, 3)]);
        assert_eq!(s.batch_info().stage, STAGE_WRITING);

        let mut s = reviewing();
        assert!(s.skip_empty_review().is_none(), "occupied slots among them");
        assert_eq!(s.batch_info().stage, STAGE_REVIEW);
    }

    #[test]
    fn the_edited_slots_come_in_the_sidebar_order() {
        let s = edited();
        assert_eq!(s.edited_slots(), [(Mode::XInput, 2), (Mode::XInput, 3), (Mode::DInput, 1)]);
        assert!(s.can_write_all());
        let clean = connected(Mode::XInput);
        assert!(clean.edited_slots().is_empty() && !clean.can_write_all());
    }

    #[test]
    fn write_all_reads_first_and_locks_both_buttons_in_every_stage() {
        let mut s = edited();
        s.select(0, 1);
        assert!(s.can_write() && s.can_write_all());
        assert_eq!(s.begin_batch().as_deref(), Some(PORT));
        assert_eq!(s.begin_batch(), None, "a second press sends no second read");
        assert!(s.checking_batch() && !s.can_write() && !s.can_write_all());
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        assert!(!s.checking_batch() && !s.can_write() && !s.can_write_all(), "in review");
        let (_, jobs) = s.confirm_batch().unwrap();
        assert!(!s.can_write() && !s.can_write_all() && !s.can_clear(), "while it runs");
        let bad = WriteResult::failure(
            jobs[0].mode,
            jobs[0].slot,
            controller_core::ErrorCategory::WriteFailure,
            "Write failed.",
        );
        s.write_finished(PORT, &[bad], None);
        s.batch_after_write();
        assert!(!s.can_write() && !s.can_write_all(), "in the failure");
        s.end_batch();
        assert!(s.can_write() && s.can_write_all());
    }

    #[test]
    fn a_failed_read_ends_the_batch_and_a_good_one_opens_the_review() {
        let mut s = edited();
        s.begin_batch().unwrap();
        s.read_finished(PORT, Err("timed out".to_owned()));
        s.review_read(PORT, false);
        assert!(s.write.batch.is_none() && s.batch_info().stage == 0);

        let s = reviewing();
        assert_eq!(s.batch_info().stage, 1);
        assert_eq!(s.batch_info().slots.row_count(), 3);
    }

    #[test]
    fn the_review_waits_for_the_changed_slot_question_and_shrinks_with_the_edits() {
        let mut s = edited();
        s.begin_batch().unwrap();
        let mut read = full_read();
        for p in &mut read.profiles {
            if (p.mode, p.source_slot) == (Mode::XInput, 2) {
                p.canonical.name = "Theirs".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        s.review_read(PORT, true);
        assert_eq!(s.batch_info().stage, 0, "the changed-slot question comes first");
        s.choose_changed((Mode::XInput, 2), false);
        s.apply_changed();
        let info = s.batch_info();
        assert_eq!(info.stage, 1);
        assert_eq!(info.slots.row_count(), 2, "the dropped slot left the list");

        // Dropping the rest ends the batch.
        let mut s = edited();
        s.begin_batch().unwrap();
        let mut read = full_read();
        for p in &mut read.profiles {
            if p.source_slot == 2 || (p.mode, p.source_slot) == (Mode::DInput, 1) {
                p.canonical.name = "Theirs".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        s.review_read(PORT, true);
        for key in s.changed_slots() {
            s.choose_changed(key, false);
        }
        // XInput slot 3 was empty and had no stored profile to change under it.
        s.select(0, 2);
        s.discard();
        s.apply_changed();
        assert_eq!(s.edited_slots().len(), 0);
        assert!(s.write.batch.is_none(), "nothing left to write");
    }

    #[test]
    fn edits_dropped_while_the_controller_is_read_leave_no_hidden_batch() {
        let mut s = edited();
        s.begin_batch().unwrap();
        for (m, n) in [(0, 1), (0, 2), (2, 0)] {
            s.select(m, n);
            s.discard();
        }
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        assert!(s.write.batch.is_none() && !s.can_write_all());
    }

    #[test]
    fn another_controller_drops_a_waiting_batch() {
        let mut s = edited();
        s.presence("3-2", Some(Mode::XInput));
        s.read_finished("3-2", Ok(full_read()));
        s.begin_batch().unwrap();
        s.pick_controller(1);
        assert!(s.write.batch.is_none());
    }

    #[test]
    fn rows_start_open_for_the_first_slot_and_for_a_slot_that_removes_a_macro() {
        let mut s = edited();
        // Slot 2 holds two macros on button p1: giving p1 an output drops them.
        s.select(0, 1);
        s.set_output("p1", "disabled");
        s.begin_batch().unwrap();
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        let open = |s: &AppState| s.batch_info().slots.iter().map(|r| r.open).collect::<Vec<_>>();
        let info = s.batch_info();
        assert!(info.any_removed);
        assert_eq!(open(&s), [true, false, false], "slot 2 is first and removes macros");

        // The first row closes on a click, and a second slot opens.
        s.toggle_batch_slot(0);
        s.toggle_batch_slot(2);
        assert_eq!(open(&s), [false, false, true]);
        s.toggle_batch_slot(9);
    }

    #[test]
    fn a_slot_that_removes_a_macro_is_open_even_when_it_is_not_first() {
        let mut s = connected(Mode::XInput);
        s.select(0, 0);
        s.set_name("One");
        s.select(0, 1);
        s.set_output("p1", "disabled");
        s.begin_batch().unwrap();
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        let rows: Vec<_> = s.batch_info().slots.iter().collect();
        assert_eq!(rows.iter().map(|r| r.open).collect::<Vec<_>>(), [true, true]);
        assert_eq!(rows[1].removed.row_count(), 2);
    }

    #[test]
    fn the_review_counts_the_changes_and_new_slots() {
        let s = reviewing();
        let info = s.batch_info();
        assert!(info.any_new, "XInput slot 3 is new");
        assert!(!info.any_removed);
        // The two renames; the new slot has no change list.
        assert_eq!(info.total_changes, 2);
        let rows: Vec<_> = info.slots.iter().collect();
        assert_eq!(rows[0].title, "XInput slot 2");
        assert_eq!(rows[1].title, "XInput slot 3");
        assert!(rows[1].is_new && rows[1].changes.row_count() == 0);
        assert_eq!(rows[2].title, "DInput slot 1");
    }

    #[test]
    fn the_batch_goes_out_as_one_run_and_reads_once_at_the_end() {
        let mut s = reviewing();
        let (port, jobs) = s.confirm_batch().unwrap();
        assert_eq!(port, PORT);
        assert_eq!(targets(&jobs), [(Mode::XInput, 2), (Mode::XInput, 3), (Mode::DInput, 1)]);
        assert!(matches!(jobs[0].op, WriteOp::Upload { .. }));
        let info = s.batch_info();
        assert_eq!((info.stage, info.at), (STAGE_WRITING, 0));
        assert!(s.write.running.is_some());

        s.write_finished(PORT, &oks(&jobs), None);
        assert!(!s.slot(Mode::XInput, 2).unsaved(), "a written slot is saved at once");
        assert!(!s.slot(Mode::XInput, 3).unsaved() && !s.slot(Mode::DInput, 1).unsaved());
        s.batch_after_write();
        assert!(s.write.batch.is_none() && s.write.running.is_none());
        assert_eq!(s.edited_slots().len(), 0);
        let notice = s.notice.clone().unwrap();
        assert_eq!(notice.title, "Wrote 3 slots to the controller.");
        assert!(!notice.error);
    }

    #[test]
    fn a_failure_stops_the_batch_keeps_the_edits_and_retries_the_rest() {
        let mut s = reviewing();
        let (_, jobs) = s.confirm_batch().unwrap();
        // The second slot's bank failed, and the controller wrote the first.
        let mut bad = WriteResult::failure(
            jobs[1].mode,
            jobs[1].slot,
            controller_core::ErrorCategory::WriteFailure,
            "Write failed at chunk 12/53. Rollback failed.",
        );
        bad.rollback_attempted = true;
        bad.backup_file_path = Some("/home/x/b.bin".to_owned());
        let results = [ok(&jobs[0]), bad.clone(), bad];
        s.write_finished(PORT, &results, Some("Steam also has it open."));
        assert!(s.write.failed.is_some(), "the failure waits for the batch to take it");
        s.batch_after_write();
        assert!(s.write.failed.is_none(), "the single-slot failure does not open");
        assert!(s.notice.is_none());
        assert_eq!(s.edited_slots(), [(Mode::XInput, 3), (Mode::DInput, 1)]);

        let info = s.batch_info();
        assert_eq!((info.stage, info.at), (3, 1));
        assert_eq!(
            info.detail,
            "The controller did not accept the profile, and the slot could not be put back as it was. \
             A copy of the old profile is saved. Steam also has it open."
        );
        assert_eq!(info.backup_path, "/home/x/b.bin");
        assert_eq!(info.slots.row_count(), 3);

        let (_, again) = s.retry_batch().unwrap();
        assert_eq!(again, jobs[1..]);
        let info = s.batch_info();
        assert_eq!((info.stage, info.at), (STAGE_WRITING, 1));
        s.write_finished(PORT, &oks(&again), None);
        s.batch_after_write();
        assert!(s.write.batch.is_none());
        assert_eq!(s.notice.unwrap().title, "Wrote 3 slots to the controller.");
    }

    #[test]
    fn closing_the_failure_keeps_every_edit_that_was_not_written() {
        let mut s = reviewing();
        let (_, jobs) = s.confirm_batch().unwrap();
        let bad = WriteResult::failure(
            jobs[0].mode,
            jobs[0].slot,
            controller_core::ErrorCategory::Timeout,
            "usb error",
        );
        s.write_finished(PORT, &[bad], None);
        s.batch_after_write();
        s.end_batch();
        assert!(s.write.batch.is_none());
        assert_eq!(s.edited_slots().len(), 3);
    }

    #[test]
    fn one_slot_reads_as_one_slot() {
        let mut s = connected(Mode::XInput);
        s.set_name("Solo");
        s.begin_batch().unwrap();
        s.read_finished(PORT, Ok(full_read()));
        s.review_read(PORT, true);
        let (_, jobs) = s.confirm_batch().unwrap();
        s.write_finished(PORT, &oks(&jobs), None);
        s.batch_after_write();
        assert_eq!(s.notice.unwrap().title, "Wrote 1 slot to the controller.");
    }

    #[test]
    fn a_single_write_is_not_a_batch_write() {
        let mut s = connected(Mode::XInput);
        s.set_name("Solo");
        s.begin_review().unwrap();
        s.review_read(PORT, true);
        let (_, jobs) = s.confirm_review().unwrap();
        s.write_finished(PORT, &oks(&jobs), None);
        s.batch_after_write();
        assert!(s.notice.unwrap().title.starts_with("Wrote XInput slot 1"));
    }
}
