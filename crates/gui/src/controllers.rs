//! The controllers the window knows, keyed by USB port path: which one the header
//! shows, what each holds, and the two questions a read can raise. No controller
//! carries a readable id, so the port is the key and the slot contents are the guard.

use std::collections::BTreeMap;

use controller_core::description::ControllerDescription;
use controller_core::device::Model;
use controller_core::model::{CanonicalProfile, Mode, ProfileReadResult};

use crate::state::{AppState, Install, SlotState};

/// One controller: present, or unplugged with edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Controller {
    /// The USB port path, such as `8-5`. A mode change keeps it.
    pub port: String,
    /// The mode the controller presents now. `None` once unplugged.
    pub mode: Option<Mode>,
    /// A read answered with a model this app knows. Until then the controller is
    /// not listed, so a pad that only shares a USB id stays hidden.
    pub known: bool,
    /// Every slot read so far, by mode and 1-based number.
    pub slots: BTreeMap<(Mode, u8), SlotState>,
    /// Whether a read is running.
    pub reading: bool,
    /// Why the last read failed. Cleared by a good read, or when the controller goes
    /// or comes back.
    pub read_error: Option<String>,
    /// Slots whose stored profile changed under their edits since the edits began,
    /// each with the answer picked so far: `true` keeps the edits, `false` takes
    /// the controller's version. Nothing applies until [`AppState::apply_changed`].
    pub changed: BTreeMap<(Mode, u8), bool>,
    /// The empty slots that still hold macros, with how many, at the last read.
    pub leftover: BTreeMap<(Mode, u8), usize>,
    /// The model's description, from the last good read. `None` before one.
    pub description: Option<&'static ControllerDescription>,
    /// The profile a new slot of each of the model's modes starts from.
    pub defaults: BTreeMap<Mode, CanonicalProfile>,
}

impl Controller {
    /// Whether any slot holds an edit that differs from the controller.
    #[must_use]
    pub fn has_edits(&self) -> bool {
        self.slots.values().any(SlotState::unsaved)
    }

    /// Unplugged, with edits kept.
    #[must_use]
    pub const fn disconnected(&self) -> bool {
        self.mode.is_none()
    }

    /// Takes a read: replaces what every slot holds and keeps the edits. A slot whose
    /// stored profile changed under real edits goes in [`Self::changed`]; an edit
    /// copy equal to the old stored profile is dropped, so it cannot undo the change.
    fn take_read(&mut self, read: ProfileReadResult) {
        for summary in read.profiles {
            let key = (summary.mode, summary.source_slot);
            // An empty slot reads back as a summary with no id.
            let pad = (!summary.id.is_empty()).then_some(summary.canonical);
            let slot = self.slots.entry(key).or_default();
            if slot.pad != pad {
                if !slot.unsaved() {
                    slot.edited = None;
                } else if slot.edited != pad {
                    self.changed.entry(key).or_insert(true);
                }
            }
            slot.pad = pad;
        }
    }
}

impl AppState {
    /// The controller on `port`, for a change.
    pub fn controller_mut(&mut self, port: &str) -> Option<&mut Controller> {
        self.controllers.iter_mut().find(|c| c.port == port)
    }

    /// The controllers the header lists: every one a read has named.
    pub fn listed(&self) -> impl Iterator<Item = &Controller> {
        self.controllers.iter().filter(|c| c.known)
    }

    /// Whether the header lists a controller. Until then the window shows only the
    /// no-controller screen.
    #[must_use]
    pub fn has_controller(&self) -> bool {
        self.listed().next().is_some()
    }

    /// The controller the window shows: the one picked in the header, else the
    /// first listed.
    #[must_use]
    pub fn active(&self) -> Option<&Controller> {
        let picked = self.active_port.as_deref();
        self.listed().find(|c| Some(c.port.as_str()) == picked).or_else(|| self.listed().next())
    }

    /// The controller the window shows, for a change.
    pub fn active_mut(&mut self) -> Option<&mut Controller> {
        let port = self.active()?.port.clone();
        self.controller_mut(&port)
    }

    /// The controller the status and the read error come from: the active one, else
    /// one still being read, for the no-controller screen.
    #[must_use]
    pub fn shown(&self) -> Option<&Controller> {
        self.active().or_else(|| self.controllers.first())
    }

    /// The mode the shown controller presents now. `None` while unplugged or absent.
    #[must_use]
    pub fn current_mode(&self) -> Option<Mode> {
        self.shown().and_then(|c| c.mode)
    }

    /// Whether a read of the shown controller is running.
    #[must_use]
    pub fn reading(&self) -> bool {
        self.shown().is_some_and(|c| c.reading)
    }

    /// The controller whose failed read the red bar shows: the shown one, else a
    /// newly plugged one no read has named yet. Without it, a controller whose
    /// first read fails stays out of sight behind a listed one.
    #[must_use]
    pub fn failed(&self) -> Option<&Controller> {
        self.shown().filter(|c| c.read_error.is_some()).or_else(|| {
            self.controllers.iter().find(|c| !c.known && c.mode.is_some() && c.read_error.is_some())
        })
    }

    /// Why the last read of [`Self::failed`] failed.
    #[must_use]
    pub fn read_error(&self) -> Option<&str> {
        self.failed().and_then(|c| c.read_error.as_deref())
    }

    /// Whether the red bar is about a controller the window does not show.
    #[must_use]
    pub fn failed_elsewhere(&self) -> bool {
        self.failed().map(|c| &c.port) != self.shown().map(|c| &c.port)
    }

    /// The controller "Try again" reads: the failed one, else the shown one.
    #[must_use]
    pub fn retry_port(&self) -> Option<String> {
        self.failed().or_else(|| self.shown()).filter(|c| c.mode.is_some()).map(|c| c.port.clone())
    }

    /// The controller on `port` appeared in `mode`, or went away (`None`). An
    /// unplugged controller with edits stays listed; one without goes.
    pub fn presence(&mut self, port: &str, mode: Option<Mode>) {
        self.access = None;
        self.rechecked = false;
        self.install = Install::Idle;
        if mode.is_none() {
            self.fix_gone(port);
        }
        let Some(i) = self.controllers.iter().position(|c| c.port == port) else {
            if mode.is_some() {
                self.controllers.push(Controller {
                    port: port.to_owned(),
                    mode,
                    ..Controller::default()
                });
            }
            return;
        };
        let Some(c) = self.controllers.get_mut(i) else { return };
        c.mode = mode;
        c.reading = false;
        c.read_error = None;
        if mode.is_none() && !c.has_edits() {
            self.remove(i);
        }
    }

    /// Drops controller `i`. If the header showed it, the next listed one shows.
    fn remove(&mut self, i: usize) {
        if i >= self.controllers.len() {
            return;
        }
        let gone = self.controllers.remove(i);
        if self.active_port.as_ref() == Some(&gone.port) {
            let next = self.listed().next().map(|c| c.port.clone());
            self.active_port = next;
        }
        if self.pending_move.as_ref() == Some(&gone.port) {
            self.pending_move = None;
        }
    }

    /// A read of the controller on `port` was sent to the worker. Its last failure
    /// stays until this one ends.
    pub fn read_started(&mut self, port: &str) {
        if let Some(c) = self.controller_mut(port) {
            c.reading = true;
        }
    }

    /// A read of the controller on `port` came back. A success names the controller,
    /// so the header lists it.
    pub fn read_finished(&mut self, port: &str, result: Result<ProfileReadResult, String>) {
        let first_listed = !self.has_controller();
        let others_unplugged_with_edits =
            self.controllers.iter().any(|c| c.port != port && c.disconnected() && c.has_edits());
        let Some(c) = self.controller_mut(port) else { return };
        c.reading = false;
        let read = match result {
            Ok(read) => read,
            Err(e) => {
                c.read_error = Some(e);
                return;
            }
        };
        c.take_read(read);
        c.read_error = None;
        let newly_known = !std::mem::replace(&mut c.known, true);
        let mode = c.mode;
        self.access = None;
        self.rechecked = false;
        if first_listed {
            self.active_port = Some(port.to_owned());
            if let Some(i) = self.description().modes.iter().position(|m| Some(m.id) == mode) {
                self.selected = (i, 0);
            }
        }
        if newly_known && others_unplugged_with_edits {
            self.pending_move = Some(port.to_owned());
        }
    }

    /// The controller on `port` named a model this app does not know. It is dropped,
    /// unless it holds edits from an earlier controller on that port: then it shows
    /// as unplugged.
    pub fn read_foreign(&mut self, port: &str) {
        let Some(i) = self.controllers.iter().position(|c| c.port == port) else { return };
        let Some(c) = self.controllers.get_mut(i) else { return };
        c.reading = false;
        c.mode = None;
        if !c.has_edits() {
            self.remove(i);
        }
    }

    /// Shows listed controller `index` in the window.
    pub fn pick_controller(&mut self, index: usize) {
        let Some(port) = self.listed().nth(index).map(|c| c.port.clone()) else { return };
        if self.active().map(|c| &c.port) != Some(&port) {
            self.notice = None;
            self.pending_import = None;
            self.write.reading_for = None;
            self.drop_waiting_batch();
        }
        self.active_port = Some(port);
    }

    /// The header line of each listed controller, such as `Pro 3 · XInput`,
    /// with a number when two read the same.
    #[must_use]
    pub fn controller_labels(&self) -> Vec<String> {
        let base: Vec<String> = self
            .listed()
            .map(|c| {
                let state = c.mode.map_or("Disconnected", Mode::label);
                format!("{} · {state}", self.description().short_name)
            })
            .collect();
        base.iter()
            .enumerate()
            .map(|(i, label)| {
                let same = base.iter().filter(|b| *b == label).count();
                let nth = base.iter().take(i + 1).filter(|b| *b == label).count();
                if same > 1 {
                    format!("{label} ({nth})")
                } else {
                    label.clone()
                }
            })
            .collect()
    }

    /// The listed index of the controller the window shows.
    #[must_use]
    pub fn active_index(&self) -> usize {
        let port = self.active().map(|c| &c.port);
        self.listed().position(|c| Some(&c.port) == port).unwrap_or(0)
    }

    /// The unplugged controllers whose edits the waiting new one may take, as listed
    /// indices. Empty when no question waits.
    #[must_use]
    pub fn move_candidates(&self) -> Vec<usize> {
        if self.pending_move.is_none() {
            return Vec::new();
        }
        self.listed()
            .enumerate()
            .filter(|(_, c)| c.disconnected() && c.has_edits())
            .map(|(i, _)| i)
            .collect()
    }

    /// The answer to "Move the edits?": listed controller `from` hands its edits to
    /// the waiting new controller and goes, or `None` keeps both apart. A slot whose
    /// stored profile differs from the one the edits began on goes in the new
    /// controller's changed list.
    pub fn answer_move(&mut self, from: Option<usize>) {
        let Some(to) = self.pending_move.take() else { return };
        let Some(from) = from.and_then(|i| self.listed().nth(i)).map(|c| c.port.clone()) else {
            return;
        };
        let Some(i) = self.controllers.iter().position(|c| c.port == from && c.disconnected())
        else {
            return;
        };
        let old = self.controllers.remove(i);
        if self.active_port.as_ref() == Some(&old.port) {
            self.active_port = Some(to.clone());
        }
        let Some(c) = self.controller_mut(&to) else { return };
        for (key, slot) in old.slots {
            if !slot.unsaved() {
                continue;
            }
            let fresh = c.slots.entry(key).or_default();
            if ![&slot.pad, &slot.edited].contains(&&fresh.pad) {
                c.changed.entry(key).or_insert(true);
            }
            fresh.edited = slot.edited;
        }
    }

    /// The slots of the shown controller whose stored profile changed under their
    /// edits, waiting for an answer.
    #[must_use]
    pub fn changed_slots(&self) -> Vec<(Mode, u8)> {
        self.active().map(|c| c.changed.keys().copied().collect()).unwrap_or_default()
    }

    /// The answer picked so far for each of [`Self::changed_slots`]: `true` keeps
    /// the edits.
    #[must_use]
    pub fn changed_choices(&self) -> Vec<bool> {
        self.active().map(|c| c.changed.values().copied().collect()).unwrap_or_default()
    }

    /// Picks the answer for one changed slot: keep the edits over the new stored
    /// profile, or take the controller's version. Applies nothing yet.
    pub fn choose_changed(&mut self, key: (Mode, u8), keep: bool) {
        if let Some(choice) = self.active_mut().and_then(|c| c.changed.get_mut(&key)) {
            *choice = keep;
        }
    }

    /// Applies every picked answer and closes the question.
    pub fn apply_changed(&mut self) {
        let Some(c) = self.active_mut() else { return };
        for (key, keep) in std::mem::take(&mut c.changed) {
            if let (false, Some(slot)) = (keep, c.slots.get_mut(&key)) {
                slot.edited = None;
            }
        }
        self.settle_review();
        self.settle_batch();
    }

    /// Records which empty slots of the controller on `port` still hold macros.
    pub fn set_leftover(&mut self, port: &str, leftover: BTreeMap<(Mode, u8), usize>) {
        if let Some(c) = self.controller_mut(port) {
            c.leftover = leftover;
        }
    }

    /// Records the model a read of `port` identified: its description and the profile
    /// a new slot of each mode starts from.
    pub fn set_model(&mut self, port: &str, model: &'static dyn Model) {
        let Some(c) = self.controller_mut(port) else { return };
        let Ok(description) = model.description() else { return };
        c.description = Some(description);
        c.defaults =
            description.modes.iter().map(|m| (m.id, model.default_profile(m.id))).collect();
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::state::tests::{connected, full_read, new_state, PORT};

    /// A read of the test pad: slot 1 named `name`, slot 2 empty.
    fn test_pad_read(name: &str) -> ProfileReadResult {
        use controller_core::device::ProtocolCodec as _;
        use controller_core::devices::test_pad::TestPad;
        let summary = |number: u8, name: &str| {
            let mut canonical = TestPad.default_profile(Mode::DInput);
            canonical.name = name.to_owned();
            let id = if name.is_empty() { String::new() } else { format!("dinput-slot-{number}") };
            controller_core::model::CanonicalProfileSummary {
                id,
                name: name.to_owned(),
                mode: Mode::DInput,
                source_slot: number,
                source_profile_index: number - 1,
                canonical,
            }
        };
        ProfileReadResult { profiles: vec![summary(1, name), summary(2, "")], raw_blobs: vec![] }
    }

    #[test]
    fn each_controller_shows_its_own_model() {
        use controller_core::devices::test_pad::TestPad;
        let mut s = connected(Mode::XInput);
        s.presence("3-1", Some(Mode::DInput));
        s.set_model("3-1", &TestPad);
        s.read_finished("3-1", Ok(test_pad_read("Pad")));

        assert_eq!(s.description().short_name, "Pro 3", "the first controller stays shown");
        s.active_port = Some("3-1".to_owned());
        assert_eq!(s.description().short_name, "Test Pad");
        assert_eq!(s.defaults().keys().copied().collect::<Vec<_>>(), [Mode::DInput]);
        let groups = crate::render::groups(&s);
        assert_eq!(groups.len(), 1, "the test pad has one mode");
        let slots = groups.first().map(|g| slint::Model::row_count(&g.slots));
        assert_eq!(slots, Some(2), "and two slots");
        assert_eq!(s.slot(Mode::DInput, 1).pad.as_ref().map(|p| p.name.as_str()), Some("Pad"));
        s.select(0, 0);
        let tabs: Vec<_> = crate::buttons::sections(&s).into_iter().map(|t| t.name).collect();
        assert_eq!(tabs, ["Buttons", "Feel"], "the test pad declares one settings tab");
        s.set_number("/rumble/level", 9.0);
        s.set_flag("/lights/on", false);
        let edited = s.slot(Mode::DInput, 1).edited.unwrap();
        assert_eq!(edited.setting("/rumble/level"), Some(&3.into()), "kept in its own range");
        assert_eq!(edited.setting("/lights/on"), Some(&false.into()));

        s.active_port = Some(PORT.to_owned());
        assert_eq!(s.description().modes.len(), 3, "back to the Pro 3");
    }

    /// Renames slot 1 of `mode` on the shown controller.
    fn edit(s: &mut AppState, mode: Mode, name: &str) {
        let i = s.description().modes.iter().position(|m| m.id == mode).unwrap();
        s.select(i, 0);
        s.set_name(name);
    }

    /// A read whose slot 1 of `mode` is named `name`.
    fn read_with(mode: Mode, name: &str) -> ProfileReadResult {
        let mut read = full_read();
        for p in &mut read.profiles {
            if (p.mode, p.source_slot) == (mode, 1) {
                p.canonical.name = name.to_owned();
            }
        }
        read
    }

    #[test]
    fn a_controller_is_listed_only_once_a_read_names_it() {
        let mut s = new_state();
        s.presence("3-1", Some(Mode::Switch));
        assert!(!s.has_controller());
        s.read_started("3-1");
        assert!(s.reading(), "the no-controller screen shows the read");
        s.read_finished("3-1", Err("device communication timed out".to_owned()));
        assert!(!s.has_controller());
        assert_eq!(s.read_error(), Some("device communication timed out"));
        s.read_foreign("3-1");
        assert!(s.controllers.is_empty(), "another model is dropped");
    }

    #[test]
    fn a_failed_first_read_on_a_new_port_shows_behind_a_listed_controller() {
        let mut s = connected(Mode::XInput);
        s.set_name("Kept");
        s.presence(PORT, None);
        s.presence("3-2", Some(Mode::DInput));
        s.read_finished("3-2", Err("device communication timed out".to_owned()));
        assert_eq!(s.active().unwrap().port, PORT, "the unplugged entry still shows");
        assert_eq!(s.read_error(), Some("device communication timed out"));
        assert!(s.failed_elsewhere());
        assert_eq!(s.retry_port().as_deref(), Some("3-2"), "Try again reads the new port");
        s.read_started("3-2");
        assert!(s.read_error().is_some(), "the bar stays while it reads again");
        s.read_finished("3-2", Ok(full_read()));
        assert_eq!(s.read_error(), None);
        assert_eq!(s.pending_move.as_deref(), Some("3-2"), "then the move question comes");
    }

    #[test]
    fn two_controllers_keep_their_edits_apart() {
        let mut s = connected(Mode::XInput);
        s.presence("3-2", Some(Mode::XInput));
        s.read_finished("3-2", Ok(full_read()));
        assert_eq!(s.active().unwrap().port, PORT, "the selection stays when another appears");
        assert_eq!(s.controller_labels(), ["Pro 3 · XInput (1)", "Pro 3 · XInput (2)"]);
        edit(&mut s, Mode::XInput, "First");
        s.pick_controller(1);
        assert_eq!(s.active_index(), 1);
        assert!(!s.slot(Mode::XInput, 1).unsaved());
        edit(&mut s, Mode::XInput, "Second");
        s.pick_controller(0);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().name, "First");
    }

    #[test]
    fn a_mode_slide_keeps_the_entry_and_its_edits() {
        let mut s = connected(Mode::XInput);
        edit(&mut s, Mode::DInput, "Kept");
        s.presence(PORT, None);
        s.presence(PORT, Some(Mode::DInput));
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(s.controllers.len(), 1);
        assert_eq!(s.controller_labels(), ["Pro 3 · DInput"]);
        assert_eq!(s.slot(Mode::DInput, 1).shown().unwrap().name, "Kept");
        assert!(s.changed_slots().is_empty(), "the slot did not change under the edit");
    }

    #[test]
    fn unplug_keeps_an_entry_with_edits_and_drops_one_without() {
        let mut s = connected(Mode::XInput);
        s.presence("3-2", Some(Mode::DInput));
        s.read_finished("3-2", Ok(full_read()));
        edit(&mut s, Mode::XInput, "Kept");
        s.presence(PORT, None);
        s.presence("3-2", None);
        assert_eq!(s.controller_labels(), ["Pro 3 · Disconnected"]);
        assert!(s.has_controller() && s.current_mode().is_none());
        // Back on the same port: the entry takes it.
        s.presence(PORT, Some(Mode::Switch));
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(s.pending_move, None);
        assert_eq!(s.controller_labels(), ["Pro 3 · Switch"]);
        assert!(s.slot(Mode::XInput, 1).unsaved());
    }

    #[test]
    fn a_new_port_is_asked_to_take_the_edits() {
        let mut s = connected(Mode::XInput);
        edit(&mut s, Mode::XInput, "Moved");
        s.presence(PORT, None);
        s.presence("3-2", Some(Mode::XInput));
        s.read_finished("3-2", Ok(read_with(Mode::XInput, "Changed")));
        assert_eq!(s.pending_move.as_deref(), Some("3-2"));
        assert_eq!(s.move_candidates(), [0]);
        s.answer_move(Some(0));
        assert_eq!(s.controllers.len(), 1);
        assert_eq!(s.active().unwrap().port, "3-2");
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().name, "Moved");
        assert_eq!(s.changed_slots(), [(Mode::XInput, 1)], "the new pad's slot 1 differs");
    }

    #[test]
    fn no_keeps_the_unplugged_entry_apart() {
        let mut s = connected(Mode::XInput);
        edit(&mut s, Mode::XInput, "Stays");
        s.presence(PORT, None);
        s.presence("3-2", Some(Mode::DInput));
        s.read_finished("3-2", Ok(full_read()));
        s.answer_move(None);
        assert_eq!(s.pending_move, None);
        assert_eq!(s.controller_labels(), ["Pro 3 · Disconnected", "Pro 3 · DInput"]);
        assert_eq!(s.move_candidates(), Vec::<usize>::new());
    }

    #[test]
    fn a_slot_changed_under_its_edits_asks_keep_or_discard() {
        let mut s = connected(Mode::XInput);
        edit(&mut s, Mode::XInput, "Mine");
        edit(&mut s, Mode::Switch, "Mine too");
        s.read_started(PORT);
        let mut read = read_with(Mode::XInput, "Theirs");
        for p in &mut read.profiles {
            if (p.mode, p.source_slot) == (Mode::Switch, 1) {
                p.canonical.name = "Theirs too".to_owned();
            }
        }
        s.read_finished(PORT, Ok(read));
        assert_eq!(s.changed_slots(), [(Mode::XInput, 1), (Mode::Switch, 1)]);
        assert_eq!(s.changed_choices(), [true, true], "Keep mine is picked first");
        s.choose_changed((Mode::Switch, 1), false);
        assert_eq!(s.changed_choices(), [true, false]);
        assert_eq!(s.slot(Mode::Switch, 1).shown().unwrap().name, "Mine too", "not yet applied");
        s.apply_changed();
        assert_eq!(s.changed_slots(), []);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().name, "Mine");
        assert_eq!(s.slot(Mode::Switch, 1).shown().unwrap().name, "Theirs too");
    }

    #[test]
    fn an_edit_undone_by_hand_does_not_undo_a_change_on_the_pad() {
        let mut s = connected(Mode::XInput);
        edit(&mut s, Mode::XInput, "Temp");
        edit(&mut s, Mode::XInput, "XInput");
        s.read_finished(PORT, Ok(read_with(Mode::XInput, "Theirs")));
        assert_eq!(s.changed_slots(), []);
        assert_eq!(s.slot(Mode::XInput, 1).shown().unwrap().name, "Theirs");
    }
}
