//! Sending edits to the controller: the review before a write, the clear-slot
//! question, the write in flight and its failure. The slot is read again first, so
//! the review compares the edits with what the controller holds now, not with what
//! it held when the window last looked.

use std::rc::Rc;

use controller_core::description::ControllerDescription;
use controller_core::error::ErrorCategory;
use controller_core::model::profile::canonical_id;
use controller_core::model::{CanonicalProfile, Mode, Slot as ProfileSlot, WriteResult};
use serde_json::Value;
use slint::{Model as _, ModelRc, SharedString, VecModel};
use tr::tr;

use crate::batch::{render_batch, Batch};
use crate::buttons::{current, held_current, output_text};
use crate::controllers::Controller;
use crate::language::prose;
use crate::macros::{descriptor, named, PadMacro};
use crate::render::{and_list, shown_name, slot_name};
use crate::settings::pages_of;
use crate::state::{AppState, Notice, SlotState};
use crate::ui::{AppWindow, Change, ClearInfo, FailedInfo, ReviewInfo};
use crate::writes::{WriteJob, WriteOp};

/// A slot of one controller: its USB port path, its mode and its 1-based number.
pub type Target = (String, (Mode, u8));

/// A write that failed, kept for "Write again".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteFailure {
    /// The USB port path of the controller.
    pub port: String,
    /// What failed.
    pub job: WriteJob,
    /// Why, as sentences, with the rollback result and the programs holding the pad.
    pub detail: String,
    /// The file the old profile was saved in, when the rollback failed too.
    pub backup: Option<String>,
    /// The `macro-v1` files the old macros were saved in, when their steps could not be
    /// put back.
    pub macro_backups: Vec<String>,
}

/// Where the write dialogs and the write in flight stand.
#[derive(Debug, Default)]
pub struct WriteState {
    /// Write was pressed and the slot is being read again.
    pub reading_for: Option<Target>,
    /// The slot was read again: the review waits for a yes.
    pub review: Option<Target>,
    /// The review's two sections, profile then macros, that the user opened or closed
    /// from how they started.
    pub review_flipped: [bool; 2],
    /// Clear slot was pressed: the question waits for a yes.
    pub clearing: Option<Target>,
    /// The writes the worker is running, and the controller they go to.
    pub running: Option<(String, Vec<WriteJob>)>,
    /// The last write failed.
    pub failed: Option<WriteFailure>,
    /// Write all was pressed: every edited slot goes in one run.
    pub batch: Option<Batch>,
}

impl AppState {
    /// The slot state of `slot` on the shown controller.
    fn slot_at(&self, slot: (Mode, u8)) -> SlotState {
        self.slot(slot.0, slot.1)
    }

    /// The shown controller, when it can take a write now: present, idle, with no
    /// write or dialog in the way.
    pub fn idle(&self) -> Option<&Controller> {
        let c = self.active()?;
        let idle = !c.reading
            && c.mode.is_some()
            && self.write.running.is_none()
            && self.write.failed.is_none()
            && self.write.review.is_none()
            && self.write.clearing.is_none()
            && self.write.batch.is_none();
        idle.then_some(c)
    }

    /// The shown controller's port and the selected slot, when it can take a write
    /// now.
    fn writable(&self) -> Option<Target> {
        let c = self.idle()?;
        Some((c.port.clone(), self.selected_slot()?))
    }

    /// Whether the footer's write button ("Write `XInput` slot 2…") is on: the selected
    /// slot holds edits, and the controller takes every macro they put.
    #[must_use]
    pub fn can_write(&self) -> bool {
        self.writable().is_some_and(|(_, slot)| {
            self.slot_at(slot).unsaved() && self.invalid_macro(slot).is_none()
        })
    }

    /// Whether Clear slot is on: the selected slot holds a profile on the controller.
    #[must_use]
    pub fn can_clear(&self) -> bool {
        self.writable().is_some_and(|(_, slot)| self.slot_at(slot).pad.is_some())
    }

    /// Write was pressed. Returns the controller to read again before the review
    /// opens.
    pub fn begin_review(&mut self) -> Option<String> {
        let target = self.writable().filter(|(_, slot)| self.slot_at(*slot).unsaved())?;
        let port = target.0.clone();
        self.write.reading_for = Some(target);
        self.write.review_flipped = [false; 2];
        Some(port)
    }

    /// A read of `port` came back. If a review waits on it, it opens now, or ends if
    /// the read failed.
    pub fn review_read(&mut self, port: &str, ok: bool) {
        self.batch_read(port, ok);
        if self.write.reading_for.as_ref().is_some_and(|t| t.0 == port) {
            let target = self.write.reading_for.take();
            if ok {
                self.write.review = target;
            }
        }
    }

    /// The slot the review shows, once nothing else needs an answer first: no slot
    /// changed under its edits, and the slot still holds edits.
    #[must_use]
    pub fn review_open(&self) -> Option<&Target> {
        let target = self.write.review.as_ref()?;
        let shown = self.active().is_some_and(|c| c.port == target.0);
        (shown && self.changed_slots().is_empty() && self.slot_at(target.1).unsaved())
            .then_some(target)
    }

    /// Whether writing `slot` overwrites nothing: the controller holds no profile
    /// there, no macro is left in it, and the write is not one the encoder refuses.
    fn writes_nothing_over(&self, slot: (Mode, u8)) -> bool {
        let state = self.slot_at(slot);
        let leftover = self.active().and_then(|c| c.leftover.get(&slot)).copied().unwrap_or(0);
        state.pad.is_none()
            && leftover == 0
            && state.edited.as_ref().is_some_and(|e| clash_text(self.description(), e).is_empty())
    }

    /// A review or a batch review is about to open on slots that hold nothing: skip
    /// the confirm and return the writes to send. Any other review stays.
    pub fn skip_empty_review(&mut self) -> Option<(String, Vec<WriteJob>)> {
        if let Some(slot) = self.review_open().map(|t| t.1) {
            return if self.writes_nothing_over(slot) { self.confirm_review() } else { None };
        }
        if self.batch_review_open()
            && self.edited_slots().into_iter().all(|k| self.writes_nothing_over(k))
        {
            return self.confirm_batch();
        }
        None
    }

    /// The edits of the review's slot are gone, such as after "Take the controller's":
    /// the review ends.
    pub fn settle_review(&mut self) {
        if self.write.review.is_some()
            && self.review_open().is_none()
            && self.changed_slots().is_empty()
        {
            self.write.review = None;
        }
    }

    /// Opens or closes section `section` of the review: 0 the profile, 1 the macros.
    pub fn toggle_review_section(&mut self, section: usize) {
        if let Some(flipped) = self.write.review_flipped.get_mut(section) {
            *flipped = !*flipped;
        }
    }

    /// The user closed the review.
    pub fn cancel_review(&mut self) {
        self.write.review = None;
    }

    /// The job that writes the edits of `slot` of the shown controller: the profile
    /// with its own id and no macro references, or none when only the macros changed;
    /// the buttons that no longer start their macro because they got another output;
    /// and the macro edits.
    pub fn upload_job(&self, slot: (Mode, u8)) -> Option<WriteJob> {
        let state = self.slot_at(slot);
        let profile = match state.edited.clone().filter(|_| state.profile_unsaved()) {
            Some(mut profile) => {
                if profile.id.is_empty() {
                    profile.id = canonical_id(slot.0, slot.1, slot.1.checked_sub(1)?);
                }
                profile.macro_refs.clear();
                Some(serde_json::to_value(&profile).ok()?)
            }
            None => None,
        };
        Some(WriteJob {
            mode: slot.0,
            slot: ProfileSlot::new(slot.1).ok()?,
            op: WriteOp::Upload {
                profile,
                drop_macros: state.dropped_buttons(),
                macro_edits: state.macro_edits(self.macro_names())?,
            },
        })
    }

    /// Hands the writes to the caller and marks them running.
    pub fn start(&mut self, port: String, jobs: Vec<WriteJob>) -> (String, Vec<WriteJob>) {
        self.write.running = Some((port.clone(), jobs.clone()));
        (port, jobs)
    }

    /// The user said yes to the review. Returns the write to send.
    pub fn confirm_review(&mut self) -> Option<(String, Vec<WriteJob>)> {
        let (port, slot) = self.write.review.take()?;
        let job = self.upload_job(slot)?;
        Some(self.start(port, vec![job]))
    }

    /// Clear slot was pressed.
    pub fn begin_clear(&mut self) {
        if let Some(target) = self.writable().filter(|(_, s)| self.slot_at(*s).pad.is_some()) {
            self.write.clearing = Some(target);
        }
    }

    /// The user closed the clear question.
    pub fn cancel_clear(&mut self) {
        self.write.clearing = None;
    }

    /// The user said yes to clearing. Returns the write to send.
    pub fn confirm_clear(&mut self) -> Option<(String, Vec<WriteJob>)> {
        let (port, (mode, number)) = self.write.clearing.take()?;
        let job = WriteJob { mode, slot: ProfileSlot::new(number).ok()?, op: WriteOp::Clear };
        Some(self.start(port, vec![job]))
    }

    /// "Write again" after a failure. Returns the write to send.
    pub fn retry_write(&mut self) -> Option<(String, Vec<WriteJob>)> {
        let failed = self.write.failed.take()?;
        Some(self.start(failed.port, vec![failed.job]))
    }

    /// The user closed the failure.
    pub fn dismiss_failure(&mut self) {
        self.write.failed = None;
    }

    /// The writes came back, one result per job. A success moves the edits into what the
    /// controller holds, until the next read says what it holds; a failure keeps them,
    /// and the jobs after the first failure are left alone. `holders` names the other
    /// programs holding the controller.
    pub fn write_finished(&mut self, port: &str, results: &[WriteResult], holders: Option<&str>) {
        let Some((_, jobs)) = self.write.running.take() else { return };
        for (job, result) in jobs.into_iter().zip(results) {
            self.job_finished(port, job, result, holders);
            if self.write.failed.is_some() {
                return;
            }
        }
    }

    /// One write of a run came back.
    fn job_finished(
        &mut self,
        port: &str,
        job: WriteJob,
        result: &WriteResult,
        holders: Option<&str>,
    ) {
        let key = (job.mode, result.slot);
        let mode = self.description().mode_label(job.mode);
        if !result.success {
            // A lost connection before any chunk went out leaves the slot as it was.
            let lost = matches!(
                result.error_category,
                ErrorCategory::Timeout | ErrorCategory::ConnectionFailure
            ) && !result.rollback_attempted;
            let mut detail = crate::plain::write_failure(result);
            if lost {
                detail = format!(
                    "{} {detail}",
                    tr!("Could not reach the controller, so nothing was written.")
                );
            }
            if let Some(names) = holders {
                detail = format!("{detail} {names}");
            }
            self.write.failed = Some(WriteFailure {
                port: port.to_owned(),
                job,
                detail,
                backup: result.backup_file_path.clone(),
                macro_backups: result.macro_backup_paths.clone(),
            });
            return;
        }
        let glide_step_ms = self.description().limits.glide_step_ms;
        let names = self.macro_names();
        if let Some(slot) = self.controller_mut(port).and_then(|c| c.slots.get_mut(&key)) {
            match job.op {
                // A job with only macro edits leaves the profile. The written macros
                // show as descriptors until the read that follows, which reads their
                // steps back from the controller.
                WriteOp::Upload { .. } => {
                    if let Some(edited) = slot.edited.take() {
                        slot.pad = Some(edited);
                    }
                    if let Some(written) = slot.edited_macros.take() {
                        slot.pad_macros = written
                            .into_iter()
                            .filter(|m| m.macro_slot.is_some())
                            .map(|m| PadMacro {
                                def: descriptor(&named(m, names), glide_step_ms),
                                loaded: false,
                            })
                            .collect();
                    }
                }
                WriteOp::Clear => *slot = SlotState::default(),
            }
        }
        let (title, body) = match job.op {
            WriteOp::Upload { .. } => (
                tr!("Wrote {mode} slot {n} to the controller.", mode = mode, n = result.slot),
                tr!("The controller uses it while that slot’s light is on."),
            ),
            WriteOp::Clear => {
                (tr!("Cleared {mode} slot {n}.", mode = mode, n = result.slot), String::new())
            }
        };
        self.notice = Some(Notice { error: false, title, body });
    }

    /// Every difference between the edits of `slot` and what the controller holds,
    /// one line each, in the order of the tabs: name, buttons, sticks, triggers,
    /// vibration. Empty for a slot the controller holds nothing in.
    #[must_use]
    pub fn change_list(&self, slot: (Mode, u8)) -> Vec<Change> {
        let state = self.slot_at(slot);
        let (Some(pad), Some(edited)) = (&state.pad, &state.edited) else { return Vec::new() };
        let d = self.description();
        let mode = slot.0;
        let mut out = Vec::new();
        if pad.name != edited.name {
            out.push(change(&tr!("Name"), &quote(&pad.name), &quote(&edited.name)));
        }
        for b in &d.buttons {
            let (Some(from), Some(to)) = (held_current(&state, &b.id), current(&state, &b.id))
            else {
                continue;
            };
            if from != to {
                let label = b.labels.get(&mode).map_or(b.id.as_str(), String::as_str);
                let (held, shown) = (state.held_macros(), state.shown_macros());
                out.push(change(
                    label,
                    &output_text(d, mode, &held, &from),
                    &output_text(d, mode, &shown, &to),
                ));
            }
        }
        for page in pages_of(self, slot) {
            for frame in page.frames.iter() {
                // A frame of sliders names what a check box belongs to; the swaps
                // say it themselves, and their long names would be cut.
                let named = frame.settings.row_count() > 0;
                let mine = |what: &str| {
                    if named {
                        tr!("{frame} · {setting}", frame = frame.title, setting = what)
                    } else {
                        what.to_owned()
                    }
                };
                for s in frame.settings.iter().filter(|s| !s.was.is_empty()) {
                    out.push(change(&mine(&s.label), &s.was, &s.text));
                }
                for t in frame.toggles.iter().filter(|t| t.changed) {
                    let (off, on) = (tr!("Off"), tr!("On"));
                    let (from, to) = if t.on { (off, on) } else { (on, off) };
                    out.push(change(&mine(&t.label), &from, &to));
                }
            }
        }
        out
    }

    /// What the review of `slot` tells the user.
    pub fn review_info(&self, slot: (Mode, u8)) -> ReviewInfo {
        let state = self.slot_at(slot);
        let leftover = self.active().and_then(|c| c.leftover.get(&slot)).copied().unwrap_or(0);
        let changes = self.change_list(slot);
        let lines = self.macro_lines(slot);
        // The first section starts open, and the macros whenever the write destroys one,
        // as the rows of the Write all review do.
        let [profile_flipped, macros_flipped] = self.write.review_flipped;
        let macros_first = changes.is_empty();
        let macros_open =
            macros_first || state.touches_held_macro(self.macro_names()) || leftover > 0;
        ReviewInfo {
            profile_open: !profile_flipped,
            macros_open: macros_open != macros_flipped,
            shown: true,
            slot_title: slot_name(self.description(), slot.0, slot.1).into(),
            name: state.shown().map(|e| shown_name(&e.name, slot.1)).unwrap_or_default().into(),
            replaces: state
                .pad
                .as_ref()
                .map(|p| shown_name(&p.name, slot.1))
                .unwrap_or_default()
                .into(),
            is_new: state.pad.is_none(),
            keeps_profile: state.pad.is_some() && !state.profile_unsaved(),
            total: count(changes.len().saturating_add(state.macros_edited(self.macro_names()))),
            changes: ModelRc::from(Rc::new(VecModel::from(changes))),
            macros_stay: count(state.macros_kept(self.macro_names())),
            macro_changes: ModelRc::from(Rc::new(VecModel::from(lines))),
            leftover: count(leftover),
            problem: state
                .edited
                .as_ref()
                .map(|e| clash_text(self.description(), e))
                .unwrap_or_default()
                .into(),
        }
    }

    /// What the clear question tells the user.
    fn clear_info(&self, slot: (Mode, u8)) -> ClearInfo {
        let state = self.slot_at(slot);
        ClearInfo {
            shown: true,
            slot_title: slot_name(self.description(), slot.0, slot.1).into(),
            name: state.pad.as_ref().map(|p| p.name.as_str()).unwrap_or_default().into(),
            macros: count(state.pad_macros.len()),
            has_edits: state.unsaved(),
        }
    }
}

/// The sentence that names a flag of `profile` that is on together with a flag it
/// excludes, such as the Pro 3's D-pad swap with "Invert X" of the left stick. Empty
/// when no flag clashes. A slot read from the pad can hold such a pair, and the write
/// refuses it.
fn clash_text(description: &ControllerDescription, profile: &CanonicalProfile) -> String {
    let mode = profile.mode;
    let on = |field: &str| profile.setting(field).and_then(Value::as_bool) == Some(true);
    let flags = || {
        description.pages(mode).flat_map(|page| {
            page.frames
                .iter()
                .flat_map(move |frame| frame.flags.iter().map(move |f| (page, frame, f)))
        })
    };
    for (page, _, flag) in flags().filter(|(_, _, f)| on(&f.field)) {
        let named: Vec<String> = flag
            .excludes
            .iter()
            .filter(|e| on(e))
            .filter_map(|e| flags().find(|(_, _, f)| &f.field == e))
            .map(|(_, frame, f)| {
                tr!("“{flag}” in {frame}", flag = prose(&f.label), frame = prose(&frame.title))
            })
            .collect();
        if named.is_empty() {
            continue;
        }
        return format!(
            "{} {}",
            tr!(
                "“{flag}” cannot be on together with {list}.",
                flag = prose(&flag.label),
                list = and_list(&named)
            ),
            tr!(
                "Choose “{cancel}”, then turn one of them off on the {tab} tab.",
                cancel = tr!("Cancel"),
                tab = prose(&page.label)
            )
        );
    }
    String::new()
}

/// A profile name in quotes, for the change list.
fn quote(name: &str) -> String {
    tr!("“{name}”", name = name)
}

fn change(what: &str, from: &str, to: &str) -> Change {
    Change { what: what.into(), from: from.into(), to: to.into() }
}

/// File paths for the window.
pub fn paths(list: &[String]) -> ModelRc<SharedString> {
    let list: Vec<SharedString> = list.iter().map(SharedString::from).collect();
    ModelRc::from(Rc::new(VecModel::from(list)))
}

/// A count for the window.
pub fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Pushes the review, the clear question, the failure, the batch and the busy sheet.
pub fn render_writes(state: &AppState, ui: &AppWindow) {
    ui.set_can_write(state.can_write());
    ui.set_checking_slot(state.write.reading_for.is_some());
    ui.set_can_clear(state.can_clear());
    render_batch(state, ui);
    ui.set_review(
        state.review_open().map(|(_, slot)| state.review_info(*slot)).unwrap_or_default(),
    );
    ui.set_clear(
        state.write.clearing.as_ref().map(|(_, slot)| state.clear_info(*slot)).unwrap_or_default(),
    );
    let failed = state.write.failed.as_ref().map(|f| FailedInfo {
        shown: true,
        slot_title: slot_name(state.description(), f.job.mode, f.job.slot.get()).into(),
        detail: f.detail.as_str().into(),
        backup_path: f.backup.as_deref().unwrap_or_default().into(),
        macro_backups: paths(&f.macro_backups),
        is_clear: f.job.op == WriteOp::Clear,
    });
    ui.set_write_failed(failed.unwrap_or_default());
    // A batch shows its own progress.
    ui.set_writing_title(
        state
            .write
            .running
            .as_ref()
            .filter(|_| state.write.batch.is_none())
            .and_then(|(_, jobs)| jobs.first())
            .map(|job| {
                tr!(
                    "Writing {mode} slot {n}",
                    mode = state.description().mode_label(job.mode),
                    n = job.slot.get()
                )
            })
            .unwrap_or_default()
            .into(),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use controller_core::devices::pro3::{DINPUT, XINPUT};
    use controller_core::model::MacroRef;

    use super::*;
    use crate::state::tests::{connected, full_read, hold_macros, macro_def, PORT};

    /// `XInput` slot 1 with a macro on `rp`, a rename, a remap, a stick range and a flag
    /// edited.
    fn edited() -> AppState {
        let mut s = connected(XINPUT);
        let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap();
        pad.pad.as_mut().unwrap().macro_refs.push(MacroRef {
            trigger_buttons: vec!["rp".to_owned()],
            path: "xinput-slot1-macro0-Buttons.json".to_owned(),
        });
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(0, "Buttons", &["rp"], 0)]);
        s.set_name("Mine");
        s.set_output("r1", "disabled");
        s.set_output("rp", "right face");
        s.set_number("/sticks/left_min_pct", 12.0);
        s.set_flag("/sticks/invert_left_x", true);
        s
    }

    /// A fresh read of the controller with the macro on `rp`, as `edited` began with.
    fn read_with_macro() -> controller_core::model::ProfileReadResult {
        let mut read = full_read();
        read.profiles[0].canonical.macro_refs.push(MacroRef {
            trigger_buttons: vec!["rp".to_owned()],
            path: "xinput-slot1-macro0-Buttons.json".to_owned(),
        });
        read
    }

    fn lines(list: &[Change]) -> Vec<String> {
        list.iter().map(|c| format!("{} | {} -> {}", c.what, c.from, c.to)).collect()
    }

    #[test]
    fn the_change_list_names_each_difference_in_tab_order() {
        let s = edited();
        let list = lines(&s.change_list((XINPUT, 1)));
        assert_eq!(list[0], "Name | “XInput” -> “Mine”");
        assert!(list.contains(&"RB | RB -> Disabled".to_owned()), "{list:?}");
        assert!(list.contains(&"PR | Buttons (Macro 1) -> B".to_owned()), "{list:?}");
        assert!(
            list.contains(&"Left stick · Dead zone | 20 to 80% -> 12 to 80%".to_owned()),
            "{list:?}"
        );
        assert!(list.contains(&"Left stick · Invert X | Off -> On".to_owned()), "{list:?}");
        let order: Vec<_> = ["Name", "RB", "Left stick"]
            .iter()
            .map(|p| list.iter().position(|l| l.starts_with(p)).unwrap())
            .collect();
        assert!(order.windows(2).all(|w| w[0] < w[1]), "name, buttons, sticks: {order:?}");
        assert!(s.change_list((XINPUT, 2)).is_empty(), "an untouched slot has none");
    }

    #[test]
    fn write_reads_the_slot_again_then_opens_the_review() {
        let mut s = edited();
        assert!(s.can_write() && s.can_clear());
        assert_eq!(s.begin_review().as_deref(), Some(PORT));
        s.read_started(PORT);
        assert!(!s.can_write(), "no second press while the read runs");
        assert_eq!(s.begin_review(), None, "a second press sends no second read");
        assert!(s.review_open().is_none());
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        assert_eq!(s.review_open().map(|t| t.1), Some((XINPUT, 1)));
        let info = s.review_info((XINPUT, 1));
        assert!(!info.is_new && info.macros_stay == 1);
        assert_eq!(info.replaces, "XInput");
        let changed: Vec<_> = info.macro_changes.iter().collect();
        assert_eq!(changed, ["Buttons (Macro 1) changed. Starts with: PR → No button."]);
    }

    #[test]
    fn a_remap_takes_one_button_off_a_two_button_macro() {
        let mut s = connected(XINPUT);
        let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap();
        pad.pad.as_mut().unwrap().macro_refs.push(MacroRef {
            trigger_buttons: vec!["rp".to_owned(), "lp".to_owned()],
            path: "xinput-slot1-macro1-Jump.json".to_owned(),
        });
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(1, "Jump", &["rp", "lp"], 0)]);
        s.set_output("rp", "right face");
        let info = s.review_info((XINPUT, 1));
        assert_eq!(info.macros_stay, 1);
        let changed: Vec<_> = info.macro_changes.iter().collect();
        assert_eq!(changed, ["Jump (Macro 2) changed. Starts with: PR and PL → PL."]);
        let job = s.upload_job((XINPUT, 1)).unwrap();
        let WriteOp::Upload { drop_macros, .. } = job.op else { panic!("an upload") };
        assert_eq!(drop_macros, ["rp"]);
    }

    #[test]
    fn a_failed_fresh_read_ends_the_review() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Err("timed out".to_owned()));
        s.review_read(PORT, false);
        assert!(s.review_open().is_none() && s.write.reading_for.is_none());
    }

    #[test]
    fn a_slot_changed_under_its_edits_asks_before_the_review_and_ends_it_on_take() {
        let mut s = edited();
        s.begin_review().unwrap();
        let mut read = full_read();
        for p in &mut read.profiles {
            if (p.mode, p.source_slot) == (XINPUT, 1) {
                p.canonical.name = "Theirs".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        s.review_read(PORT, true);
        assert!(s.review_open().is_none(), "the changed-slot question comes first");
        s.choose_changed((XINPUT, 1), false);
        s.apply_changed();
        assert!(s.review_open().is_none() && s.write.review.is_none(), "nothing left to write");

        let mut s = edited();
        s.begin_review().unwrap();
        let mut read = full_read();
        read.profiles[0].canonical.name = "Theirs".to_owned();
        s.read_finished(PORT, Ok(read));
        s.review_read(PORT, true);
        s.apply_changed();
        assert!(s.review_open().is_some(), "keep mine goes on to the review");
        assert_eq!(s.change_list((XINPUT, 1))[0].from, "“Theirs”");
    }

    #[test]
    fn confirming_builds_the_upload_and_a_success_moves_the_edits_into_the_slot() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        let (port, mut jobs) = s.confirm_review().unwrap();
        let job = jobs.remove(0);
        assert_eq!(port, PORT);
        let WriteOp::Upload { profile: Some(profile), drop_macros, .. } = &job.op else {
            panic!("an upload")
        };
        assert_eq!(drop_macros, &["rp"], "the macro whose button got another output goes");
        assert_eq!(profile["macro_refs"], serde_json::json!([]));
        assert_eq!(profile["name"], "Mine");
        assert!(!s.can_write() && !s.can_clear(), "locked while the write runs");

        let ok = WriteResult::success(XINPUT, job.slot, "done");
        s.write_finished(PORT, &[ok], None);
        let slot = s.slot(XINPUT, 1);
        assert!(!slot.unsaved() && slot.pad.unwrap().name == "Mine");
        assert!(s.notice.as_ref().is_some_and(|n| !n.error && n.title.contains("XInput slot 1")));
        assert!(s.write.running.is_none() && s.write.failed.is_none());
    }

    #[test]
    fn a_new_profile_gets_the_slots_own_id() {
        let mut s = connected(DINPUT);
        s.select(2, 2);
        s.start_from_default();
        let job = s.upload_job((DINPUT, 3)).unwrap();
        let WriteOp::Upload { profile: Some(profile), .. } = job.op else { panic!("an upload") };
        assert_eq!(profile["id"], "dinput-slot-3-index-2");
        let info = s.review_info((DINPUT, 3));
        assert!(info.is_new && info.changes.row_count() == 0);
    }

    #[test]
    fn a_failed_write_keeps_the_edits_names_the_backup_and_can_go_again() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        let (_, mut jobs) = s.confirm_review().unwrap();
        let job = jobs.remove(0);
        let mut bad = WriteResult::failure(
            XINPUT,
            job.slot,
            controller_core::ErrorCategory::WriteFailure,
            "Write failed at chunk 12/53. Rollback failed.",
        );
        bad.rollback_attempted = true;
        bad.backup_file_path = Some("/home/x/.local/state/8b/backups/b.bin".to_owned());
        s.write_finished(PORT, &[bad], Some("Steam also has it open."));
        let failed = s.write.failed.clone().unwrap();
        assert_eq!(
            failed.detail,
            "The controller did not accept the profile, and the slot could not be put back as it was. A copy of the old profile is saved. Steam also has it open."
        );
        assert!(failed.backup.is_some());
        assert!(s.slot(XINPUT, 1).unsaved(), "the edits stay");
        assert!(!s.can_write(), "the dialog comes first");
        let (_, mut agains) = s.retry_write().unwrap();
        let again = agains.remove(0);
        assert_eq!(again, job);
        assert!(s.write.failed.is_none() && s.write.running.is_some());
    }

    #[test]
    fn a_lost_connection_says_nothing_was_written() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        let (_, mut jobs) = s.confirm_review().unwrap();
        let job = jobs.remove(0);
        let lost = WriteResult::failure(
            XINPUT,
            job.slot,
            ErrorCategory::Timeout,
            "usb error: Connection timed out (os error 110)",
        );
        s.write_finished(PORT, &[lost], None);
        assert_eq!(
            s.write.failed.clone().unwrap().detail,
            "Could not reach the controller, so nothing was written. Check the cable and the mode switch."
        );
        assert!(s.slot(XINPUT, 1).unsaved(), "the edits stay");
    }

    #[test]
    fn clearing_asks_first_then_empties_the_slot() {
        let mut s = edited();
        s.begin_clear();
        let info = s.clear_info((XINPUT, 1));
        assert_eq!((info.name.as_str(), info.macros, info.has_edits), ("XInput", 1, true));
        let (_, mut jobs) = s.confirm_clear().unwrap();
        let job = jobs.remove(0);
        assert_eq!((job.mode, job.slot.get(), &job.op), (XINPUT, 1, &WriteOp::Clear));
        let ok = WriteResult::success(XINPUT, job.slot, "Slot deactivated.");
        s.write_finished(PORT, &[ok], None);
        let slot = s.slot(XINPUT, 1);
        assert_eq!((slot.pad, slot.edited), (None, None));
        s.begin_clear();
        assert!(s.write.clearing.is_none(), "an empty slot has nothing to clear");
    }

    #[test]
    fn nothing_writes_to_an_unplugged_controller() {
        let mut s = edited();
        s.presence(PORT, None);
        assert!(!s.can_write() && !s.can_clear());
        assert_eq!(s.begin_review(), None);
    }

    #[test]
    fn a_clashing_pair_read_from_the_pad_loads_and_the_review_names_it() {
        let mut s = connected(XINPUT);
        let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap();
        let held = pad.pad.as_mut().unwrap();
        held.set_setting("/sticks/invert_left_x", true.into());
        held.set_setting("/sticks/swap_dpad_with_left_stick", true.into());
        s.set_name("Mine");
        let problem = s.review_info((XINPUT, 1)).problem;
        assert!(problem.contains("“Invert X” in Left stick"), "{problem}");
        // Turning the swap off in the window clears it.
        s.set_flag("/sticks/swap_dpad_with_left_stick", false);
        assert_eq!(s.review_info((XINPUT, 1)).problem, "");
    }

    /// `DInput` slot 3, empty on the pad, started from default; Write pressed and the
    /// slot read again.
    fn empty_slot_write(leftover: usize) -> AppState {
        let mut s = connected(DINPUT);
        s.select(2, 2);
        s.start_from_default();
        s.set_leftover(PORT, [((DINPUT, 3), leftover)].into());
        s.begin_review().unwrap();
        s.review_read(PORT, true);
        s
    }

    #[test]
    fn an_empty_slot_with_no_macros_writes_without_the_review() {
        let mut s = empty_slot_write(0);
        let (_, mut jobs) = s.skip_empty_review().unwrap();
        let job = jobs.remove(0);
        assert_eq!((job.mode, job.slot.get()), (DINPUT, 3));
        assert!(s.review_open().is_none() && s.write.running.is_some());
    }

    #[test]
    fn an_empty_slot_with_leftover_macros_keeps_the_review() {
        let mut s = empty_slot_write(1);
        assert!(s.skip_empty_review().is_none());
        assert!(s.review_open().is_some());
    }

    #[test]
    fn an_occupied_slot_keeps_the_review() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        assert!(s.skip_empty_review().is_none());
        assert!(s.review_open().is_some());
    }

    #[test]
    fn leftover_macros_of_an_empty_slot_reach_the_review() {
        let mut s = connected(DINPUT);
        s.select(2, 2);
        s.start_from_default();
        s.set_leftover(PORT, [((DINPUT, 3), 2)].into());
        assert_eq!(s.review_info((DINPUT, 3)).leftover, 2);
    }
}

pub mod macro_edits;

#[cfg(test)]
mod control_map;
