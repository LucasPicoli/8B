//! Sending edits to the controller: the review before a write, the clear-slot
//! question, the write in flight and its failure. The slot is read again first, so
//! the review compares the edits with what the controller holds now, not with what
//! it held when the window last looked.

use std::rc::Rc;

use controller_core::devices::pro3::macros::parse_macro_file_name;
use controller_core::devices::pro3::profile::canonical_id;
use controller_core::model::{Mode, Slot as ProfileSlot, WriteResult};
use slint::{Model as _, ModelRc, SharedString, VecModel};

use crate::buttons::{current, output_label};
use crate::render::sentence;
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
}

/// Where the write dialogs and the write in flight stand.
#[derive(Debug, Default)]
pub struct WriteState {
    /// Write was pressed and the slot is being read again.
    pub reading_for: Option<Target>,
    /// The slot was read again: the review waits for a yes.
    pub review: Option<Target>,
    /// Clear slot was pressed: the question waits for a yes.
    pub clearing: Option<Target>,
    /// The write the worker is running, and the controller it goes to.
    pub running: Option<(String, WriteJob)>,
    /// The last write failed.
    pub failed: Option<WriteFailure>,
}

impl AppState {
    /// The slot state of `slot` on the shown controller.
    fn slot_at(&self, slot: (Mode, u8)) -> SlotState {
        self.slot(slot.0, slot.1)
    }

    /// The shown controller's port and the selected slot, when it can take a write
    /// now: present, idle, with no write or dialog in the way.
    fn writable(&self) -> Option<Target> {
        let c = self.active()?;
        let idle = !c.reading
            && c.mode.is_some()
            && self.write.running.is_none()
            && self.write.failed.is_none()
            && self.write.review.is_none()
            && self.write.clearing.is_none();
        if !idle {
            return None;
        }
        Some((c.port.clone(), self.selected_slot()?))
    }

    /// Whether Write to controller is on: the selected slot holds edits.
    #[must_use]
    pub fn can_write(&self) -> bool {
        self.writable().is_some_and(|(_, slot)| self.slot_at(slot).unsaved())
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
        Some(port)
    }

    /// A read of `port` came back. If a review waits on it, it opens now, or ends if
    /// the read failed.
    pub fn review_read(&mut self, port: &str, ok: bool) {
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

    /// The user closed the review.
    pub fn cancel_review(&mut self) {
        self.write.review = None;
    }

    /// The job that writes the edits of `slot` of the shown controller: the profile
    /// with its own id and no macro references, and the macros whose button got
    /// another output.
    fn upload_job(&self, slot: (Mode, u8)) -> Option<WriteJob> {
        let state = self.slot_at(slot);
        let mut profile = state.edited.clone()?;
        let removed = state
            .pad
            .iter()
            .flat_map(|p| &p.macro_refs)
            .filter(|m| !profile.macro_refs.iter().any(|e| e.trigger == m.trigger))
            .map(|m| m.trigger.clone())
            .collect();
        if profile.id.is_empty() {
            profile.id = canonical_id(slot.0, slot.1, slot.1.checked_sub(1)?);
        }
        profile.macro_refs.clear();
        Some(WriteJob {
            mode: slot.0,
            slot: ProfileSlot::new(slot.1).ok()?,
            op: WriteOp::Upload {
                profile: serde_json::to_value(&profile).ok()?,
                drop_macros: removed,
            },
        })
    }

    /// Hands the write to the caller and marks it running.
    fn start(&mut self, port: String, job: WriteJob) -> (String, WriteJob) {
        self.write.running = Some((port.clone(), job.clone()));
        (port, job)
    }

    /// The user said yes to the review. Returns the write to send.
    pub fn confirm_review(&mut self) -> Option<(String, WriteJob)> {
        let (port, slot) = self.write.review.take()?;
        let job = self.upload_job(slot)?;
        Some(self.start(port, job))
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
    pub fn confirm_clear(&mut self) -> Option<(String, WriteJob)> {
        let (port, (mode, number)) = self.write.clearing.take()?;
        let job = WriteJob { mode, slot: ProfileSlot::new(number).ok()?, op: WriteOp::Clear };
        Some(self.start(port, job))
    }

    /// "Write again" after a failure. Returns the write to send.
    pub fn retry_write(&mut self) -> Option<(String, WriteJob)> {
        let failed = self.write.failed.take()?;
        Some(self.start(failed.port, failed.job))
    }

    /// The user closed the failure.
    pub fn dismiss_failure(&mut self) {
        self.write.failed = None;
    }

    /// The write came back. A success moves the edits into what the controller holds,
    /// until the next read says what it holds; a failure keeps them. `holders` names
    /// the other programs holding the controller.
    pub fn write_finished(&mut self, port: &str, result: &WriteResult, holders: Option<String>) {
        let Some((_, job)) = self.write.running.take() else { return };
        let key = (job.mode, result.slot);
        let title = format!("{} slot {}", job.mode.label(), result.slot);
        if !result.success {
            let detail = holders.map_or_else(
                || sentence(&result.message),
                |names| format!("{} {names}", sentence(&result.message)),
            );
            self.write.failed = Some(WriteFailure {
                port: port.to_owned(),
                job,
                detail,
                backup: result.backup_file_path.clone(),
            });
            return;
        }
        if let Some(slot) = self.controller_mut(port).and_then(|c| c.slots.get_mut(&key)) {
            match job.op {
                WriteOp::Upload { .. } => slot.pad = slot.edited.take(),
                WriteOp::Clear => *slot = SlotState::default(),
            }
        }
        let (title, body) = match job.op {
            WriteOp::Upload { .. } => (
                format!("Wrote {title} to the controller."),
                "The controller uses it while that slot’s light is on.",
            ),
            WriteOp::Clear => (format!("Cleared {title}."), "Its macros stay stored."),
        };
        self.notice = Some(Notice { error: false, title, body: body.to_owned() });
    }

    /// Every difference between the edits of `slot` and what the controller holds,
    /// one line each, in the order of the tabs: name, buttons, sticks, triggers,
    /// vibration. Empty for a slot the controller holds nothing in.
    #[must_use]
    pub fn change_list(&self, slot: (Mode, u8)) -> Vec<Change> {
        let state = self.slot_at(slot);
        let (Some(pad), Some(edited)) = (&state.pad, &state.edited) else { return Vec::new() };
        let d = self.description;
        let mode = slot.0;
        let mut out = Vec::new();
        if pad.name != edited.name {
            out.push(change("Name", &quote(&pad.name), &quote(&edited.name)));
        }
        for b in &d.buttons {
            let (from, to) = (current(pad, &b.id), current(edited, &b.id));
            if from != to {
                let label = b.labels.get(&mode).map_or(b.id.as_str(), String::as_str);
                out.push(change(label, &output_label(d, mode, &from), &output_label(d, mode, &to)));
            }
        }
        for page in pages_of(self, slot) {
            for frame in page.frames.iter() {
                // A frame of sliders names what a check box belongs to; the swaps
                // say it themselves, and their long names would be cut.
                let named = frame.settings.row_count() > 0;
                let mine = |what: &str| {
                    if named {
                        format!("{} · {what}", frame.title)
                    } else {
                        what.to_owned()
                    }
                };
                for s in frame.settings.iter().filter(|s| !s.was.is_empty()) {
                    let was = s.was.strip_prefix("was ").unwrap_or(&s.was);
                    out.push(change(&mine(&s.label), was, &s.text));
                }
                for t in frame.toggles.iter().filter(|t| t.changed) {
                    let (from, to) = if t.on { ("Off", "On") } else { ("On", "Off") };
                    out.push(change(&mine(&t.label), from, to));
                }
            }
        }
        out
    }

    /// What the review of `slot` tells the user.
    fn review_info(&self, slot: (Mode, u8)) -> ReviewInfo {
        let state = self.slot_at(slot);
        let removed: Vec<SharedString> = state
            .pad
            .iter()
            .flat_map(|p| &p.macro_refs)
            .filter(|m| {
                !state.edited.iter().any(|e| e.macro_refs.iter().any(|x| x.trigger == m.trigger))
            })
            .map(|m| {
                parse_macro_file_name(&m.path)
                    .map_or_else(
                        || "Removes a macro".to_owned(),
                        |(n, name)| format!("Removes macro {} ({name})", n + 1),
                    )
                    .into()
            })
            .collect();
        let kept = state.edited.as_ref().map_or(0, |e| e.macro_refs.len());
        let leftover = self.active().and_then(|c| c.leftover.get(&slot)).copied().unwrap_or(0);
        let changes = self.change_list(slot);
        ReviewInfo {
            shown: true,
            slot_title: format!("{} slot {}", slot.0.label(), slot.1).into(),
            name: state.edited.as_ref().map(|e| e.name.as_str()).unwrap_or_default().into(),
            replaces: state.pad.as_ref().map(|p| p.name.as_str()).unwrap_or_default().into(),
            is_new: state.pad.is_none(),
            changes: ModelRc::from(Rc::new(VecModel::from(changes))),
            macros_stay: count(kept),
            removed: ModelRc::from(Rc::new(VecModel::from(removed))),
            leftover: count(leftover),
        }
    }

    /// What the clear question tells the user.
    fn clear_info(&self, slot: (Mode, u8)) -> ClearInfo {
        let state = self.slot_at(slot);
        ClearInfo {
            shown: true,
            slot_title: format!("{} slot {}", slot.0.label(), slot.1).into(),
            name: state.pad.as_ref().map(|p| p.name.as_str()).unwrap_or_default().into(),
            macros: count(state.pad.as_ref().map_or(0, |p| p.macro_refs.len())),
            has_edits: state.unsaved(),
        }
    }
}

/// A profile name in quotes, for the change list.
fn quote(name: &str) -> String {
    format!("“{name}”")
}

fn change(what: &str, from: &str, to: &str) -> Change {
    Change { what: what.into(), from: from.into(), to: to.into() }
}

/// A count for the window.
fn count(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// Pushes the review, the clear question, the failure and the busy sheet.
pub fn render_writes(state: &AppState, ui: &AppWindow) {
    ui.set_can_write(state.can_write());
    ui.set_can_clear(state.can_clear());
    ui.set_review(
        state.review_open().map(|(_, slot)| state.review_info(*slot)).unwrap_or_default(),
    );
    ui.set_clear(
        state.write.clearing.as_ref().map(|(_, slot)| state.clear_info(*slot)).unwrap_or_default(),
    );
    let failed = state.write.failed.as_ref().map(|f| FailedInfo {
        shown: true,
        slot_title: format!("{} slot {}", f.job.mode.label(), f.job.slot.get()).into(),
        detail: f.detail.as_str().into(),
        backup_path: f.backup.as_deref().unwrap_or_default().into(),
        is_clear: f.job.op == WriteOp::Clear,
    });
    ui.set_write_failed(failed.unwrap_or_default());
    ui.set_writing_title(
        state
            .write
            .running
            .as_ref()
            .map(|(_, job)| format!("Writing {} slot {}", job.mode.label(), job.slot.get()))
            .unwrap_or_default()
            .into(),
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use controller_core::model::MacroRef;
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{connected, full_read, PORT};

    /// `XInput` slot 1 with a macro on `rp`, a rename, a remap, a stick range and a flag
    /// edited.
    fn edited() -> AppState {
        let mut s = connected(Mode::XInput);
        let pad = s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 1)).unwrap();
        pad.pad.as_mut().unwrap().macro_refs.push(MacroRef {
            trigger: "rp".to_owned(),
            path: "xinput-slot1-macro0-Buttons.json".to_owned(),
        });
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
            trigger: "rp".to_owned(),
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
        let list = lines(&s.change_list((Mode::XInput, 1)));
        assert_eq!(list[0], "Name | “XInput” -> “Mine”");
        assert!(list.contains(&"RB | RB -> Disabled".to_owned()), "{list:?}");
        assert!(list.contains(&"PR | Macro 1 (Buttons) -> B".to_owned()), "{list:?}");
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
        assert!(s.change_list((Mode::XInput, 2)).is_empty(), "an untouched slot has none");
    }

    #[test]
    fn write_reads_the_slot_again_then_opens_the_review() {
        let mut s = edited();
        assert!(s.can_write() && s.can_clear());
        assert_eq!(s.begin_review().as_deref(), Some(PORT));
        s.read_started(PORT);
        assert!(!s.can_write(), "no second press while the read runs");
        assert!(s.review_open().is_none());
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        assert_eq!(s.review_open().map(|t| t.1), Some((Mode::XInput, 1)));
        let info = s.review_info((Mode::XInput, 1));
        assert!(!info.is_new && info.macros_stay == 0);
        assert_eq!(info.replaces, "XInput");
        let removed: Vec<_> = info.removed.iter().collect();
        assert_eq!(removed, ["Removes macro 1 (Buttons)"]);
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
            if (p.mode, p.source_slot) == (Mode::XInput, 1) {
                p.canonical.name = "Theirs".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        s.review_read(PORT, true);
        assert!(s.review_open().is_none(), "the changed-slot question comes first");
        s.choose_changed((Mode::XInput, 1), false);
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
        assert_eq!(s.change_list((Mode::XInput, 1))[0].from, "“Theirs”");
    }

    #[test]
    fn confirming_builds_the_upload_and_a_success_moves_the_edits_into_the_slot() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        let (port, job) = s.confirm_review().unwrap();
        assert_eq!(port, PORT);
        let WriteOp::Upload { profile, drop_macros } = &job.op else { panic!("an upload") };
        assert_eq!(drop_macros, &["rp"], "the macro whose button got another output goes");
        assert_eq!(profile["macro_refs"], serde_json::json!([]));
        assert_eq!(profile["name"], "Mine");
        assert!(!s.can_write() && !s.can_clear(), "locked while the write runs");

        let ok = WriteResult::success(Mode::XInput, job.slot, "done");
        s.write_finished(PORT, &ok, None);
        let slot = s.slot(Mode::XInput, 1);
        assert!(!slot.unsaved() && slot.pad.unwrap().name == "Mine");
        assert!(s.notice.as_ref().is_some_and(|n| !n.error && n.title.contains("XInput slot 1")));
        assert!(s.write.running.is_none() && s.write.failed.is_none());
    }

    #[test]
    fn a_new_profile_gets_the_slots_own_id() {
        let mut s = connected(Mode::DInput);
        s.select(2, 2);
        s.start_from_default();
        let job = s.upload_job((Mode::DInput, 3)).unwrap();
        let WriteOp::Upload { profile, .. } = job.op else { panic!("an upload") };
        assert_eq!(profile["id"], "dinput-slot-3-index-2");
        let info = s.review_info((Mode::DInput, 3));
        assert!(info.is_new && info.changes.row_count() == 0);
    }

    #[test]
    fn a_failed_write_keeps_the_edits_names_the_backup_and_can_go_again() {
        let mut s = edited();
        s.begin_review().unwrap();
        s.read_finished(PORT, Ok(read_with_macro()));
        s.review_read(PORT, true);
        let (_, job) = s.confirm_review().unwrap();
        let mut bad = WriteResult::failure(
            Mode::XInput,
            job.slot,
            controller_core::ErrorCategory::WriteFailure,
            "Write failed at chunk 12/53. Rollback failed.",
        );
        bad.backup_file_path = Some("/home/x/.local/state/8b/backups/b.bin".to_owned());
        s.write_finished(PORT, &bad, Some("Steam also has it open.".to_owned()));
        let failed = s.write.failed.clone().unwrap();
        assert_eq!(
            failed.detail,
            "Write failed at chunk 12/53. Rollback failed. Steam also has it open."
        );
        assert!(failed.backup.is_some());
        assert!(s.slot(Mode::XInput, 1).unsaved(), "the edits stay");
        assert!(!s.can_write(), "the dialog comes first");
        let (_, again) = s.retry_write().unwrap();
        assert_eq!(again, job);
        assert!(s.write.failed.is_none() && s.write.running.is_some());
    }

    #[test]
    fn clearing_asks_first_then_empties_the_slot() {
        let mut s = edited();
        s.begin_clear();
        let info = s.clear_info((Mode::XInput, 1));
        assert_eq!((info.name.as_str(), info.macros, info.has_edits), ("XInput", 1, true));
        let (_, job) = s.confirm_clear().unwrap();
        assert_eq!((job.mode, job.slot.get(), &job.op), (Mode::XInput, 1, &WriteOp::Clear));
        let ok = WriteResult::success(Mode::XInput, job.slot, "Slot deactivated.");
        s.write_finished(PORT, &ok, None);
        let slot = s.slot(Mode::XInput, 1);
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
    fn leftover_macros_of_an_empty_slot_reach_the_review() {
        let mut s = connected(Mode::DInput);
        s.select(2, 2);
        s.start_from_default();
        s.set_leftover(PORT, [((Mode::DInput, 3), 2)].into());
        assert_eq!(s.review_info((Mode::DInput, 3)).leftover, 2);
    }
}
