//! App state on the UI thread: what each controller holds, the edits, and what the
//! window shows. Pure: no Slint, no I/O.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use controller_core::description::{ControllerDescription, UNRECOGNISED_OUTPUT};
use controller_core::model::{ButtonMapping, CanonicalProfile, Mode};

use crate::controllers::Controller;
use crate::files::PendingImport;
use crate::review::WriteState;
use crate::worker::Command;

/// The name a profile started from default gets.
pub const NEW_PROFILE_NAME: &str = "New profile";

/// One slot: what the controller held at the last read, and the working copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotState {
    /// The profile read from the controller. `None` when the slot is empty.
    pub pad: Option<CanonicalProfile>,
    /// The edited working copy. `None` until the slot is edited.
    pub edited: Option<CanonicalProfile>,
}

/// Which tabs of a slot hold unsaved edits.
// One independent flag per tab, not a state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Dirty {
    /// Button mappings.
    pub buttons: bool,
    /// Stick ranges, inversion and swaps.
    pub sticks: bool,
    /// Trigger ranges or thresholds.
    pub triggers: bool,
    /// Motor levels.
    pub vibration: bool,
}

impl SlotState {
    /// Whether the working copy differs from what the controller holds.
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.edited.as_ref().is_some_and(|e| self.pad.as_ref() != Some(e))
    }

    /// The profile on screen: the working copy, else what the controller holds.
    #[must_use]
    pub fn shown(&self) -> Option<&CanonicalProfile> {
        self.edited.as_ref().or(self.pad.as_ref())
    }

    /// Which tabs differ between the working copy and what the controller holds. A
    /// working copy of an empty slot differs everywhere.
    #[must_use]
    pub fn dirty(&self) -> Dirty {
        match (&self.pad, &self.edited) {
            (_, None) => Dirty::default(),
            (None, Some(_)) => {
                Dirty { buttons: true, sticks: true, triggers: true, vibration: true }
            }
            (Some(p), Some(e)) => Dirty {
                buttons: p.button_mappings != e.button_mappings || p.macro_refs != e.macro_refs,
                sticks: p.sticks != e.sticks,
                triggers: p.triggers != e.triggers,
                vibration: p.vibration != e.vibration,
            },
        }
    }
}

/// Why the window asks for access to the controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// Opening the controller was denied, and the udev rule is not installed.
    Denied,
    /// The udev rule is installed, and opening the controller is still denied.
    StillDenied,
}

/// The installed access rule, against the one this build installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// The file matches.
    Current,
    /// No rule file.
    Missing,
    /// The rule file differs from the one this build installs.
    Outdated,
}

/// Where the udev rule install stands.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Install {
    /// Not started, or done.
    #[default]
    Idle,
    /// The password prompt is open.
    Running,
    /// The last install failed, with why, as a sentence.
    Failed(String),
}

/// A message above the slot, such as the result of an import, until it is closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// An error, not information.
    pub error: bool,
    /// The first sentence.
    pub title: String,
    /// More sentences, or empty.
    pub body: String,
}

/// Everything the window shows, kept between renders.
// One independent flag per question the window can ask, not a state.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug)]
pub struct AppState {
    /// The controller model.
    pub description: &'static ControllerDescription,
    /// The profile a new slot starts from, per mode, with an empty name.
    pub defaults: BTreeMap<Mode, CanonicalProfile>,
    /// Every controller present, and every unplugged one that holds edits, in the
    /// order they appeared.
    pub controllers: Vec<Controller>,
    /// The USB port path of the controller picked in the header. The first listed
    /// controller shows while it names none present.
    pub active_port: Option<String>,
    /// A newly read controller on an unused port, asking whether to take the edits
    /// of an unplugged one.
    pub pending_move: Option<String>,
    /// The slot beside the sidebar: an index into the description's modes, and a
    /// 0-based slot.
    pub selected: (usize, usize),
    /// Set while opening the controller is denied: the permission screen shows.
    pub access: Option<Access>,
    /// Where the udev rule install stands.
    pub install: Install,
    /// The installed rule. An outdated one shows the permission screen until installed
    /// or skipped. A missing one asks only once a read is denied: another rule, such as
    /// Steam's, may already grant access.
    pub rule: Rule,
    /// The user skipped the install for this run.
    pub rule_skipped: bool,
    /// "Check again" was chosen and no read has succeeded or been replugged since:
    /// a denied read then means the screen shows "Still blocked".
    pub rechecked: bool,
    /// The window runs in the Flatpak sandbox: no `pkexec`, no view of the host's
    /// rule files and no names for the programs that hold the controller. Read once
    /// at start.
    pub sandboxed: bool,
    /// The USB port paths of the attached controllers for which the keepalive fix is
    /// offered, until it is installed, or put off for this connection.
    pub fix_offered: BTreeSet<String>,
    /// The dialog that explains the keepalive fix is open.
    pub fix_open: bool,
    /// Where the fix install stands.
    pub fix_install: Install,
    /// The message above the slot. Cleared when another slot is picked.
    pub notice: Option<Notice>,
    /// A file for another mode, waiting for the user's yes.
    pub pending_import: Option<PendingImport>,
    /// The review, the clear question and the write in flight.
    pub write: WriteState,
    /// The window was asked to close over unsaved edits: the question is open.
    pub closing: bool,
    /// A command could not reach the worker: its thread has ended, so the stopped
    /// message shows. The worker is not restarted.
    pub worker_stopped: bool,
}

impl AppState {
    /// A window with no controller seen yet. `defaults` holds the profile a new slot
    /// of each mode starts from.
    #[must_use]
    pub fn new(
        description: &'static ControllerDescription,
        defaults: BTreeMap<Mode, CanonicalProfile>,
    ) -> Self {
        Self {
            description,
            defaults,
            controllers: Vec::new(),
            active_port: None,
            pending_move: None,
            selected: (0, 0),
            access: None,
            install: Install::Idle,
            rule: Rule::Current,
            rule_skipped: false,
            rechecked: false,
            sandboxed: false,
            fix_offered: BTreeSet::new(),
            fix_open: false,
            fix_install: Install::Idle,
            notice: None,
            pending_import: None,
            write: WriteState::default(),
            closing: false,
            worker_stopped: false,
        }
    }

    /// Sends `command` to the worker. A send that fails means the worker thread is
    /// gone: the stopped message shows, and the answer is `false`.
    pub fn send(&mut self, commands: &Sender<Command>, command: Command) -> bool {
        let sent = commands.send(command).is_ok();
        if !sent {
            log::error!("the worker has stopped: a command could not be sent");
            self.worker_stopped = true;
        }
        sent
    }

    /// A read of the controller on `port` was denied. `rule` is the installed rule
    /// now. In the sandbox the rule files are out of sight, so the screen asks for
    /// access only while a read is denied, never for the rule alone.
    pub fn read_denied(&mut self, port: &str, rule: Rule) {
        if let Some(c) = self.controller_mut(port) {
            c.reading = false;
        }
        self.rule = if self.sandboxed { Rule::Current } else { rule };
        self.access =
            Some(if rule == Rule::Current { Access::StillDenied } else { Access::Denied });
    }

    /// Whether the permission screen fills the window.
    #[must_use]
    pub fn asks_for_rule(&self) -> bool {
        self.access.is_some() || (self.rule == Rule::Outdated && !self.rule_skipped)
    }

    /// The udev rule install was sent to the worker.
    pub fn install_started(&mut self) {
        self.install = Install::Running;
        self.rechecked = false;
    }

    /// "Check again" was chosen: the reads are about to start. It replaces an old
    /// install failure, so the screen shows one bar, the latest.
    pub fn check_started(&mut self) {
        self.rechecked = true;
        if matches!(self.install, Install::Failed(_)) {
            self.install = Install::Idle;
        }
    }

    /// Whether the permission screen shows "Still blocked": a check came back denied.
    #[must_use]
    pub fn still_blocked(&self) -> bool {
        self.rechecked && self.access.is_some() && !self.reading_any()
    }

    /// Whether a read of any controller is running.
    #[must_use]
    pub fn reading_any(&self) -> bool {
        self.controllers.iter().any(|c| c.reading)
    }

    /// The udev rule install came back.
    pub fn install_finished(&mut self, result: Result<(), String>) {
        if result.is_ok() {
            self.rule = Rule::Current;
        }
        self.install = result.err().map_or(Install::Idle, Install::Failed);
    }

    /// Shows slot `slot` (0-based) of the mode at `mode` in the description.
    pub fn select(&mut self, mode: usize, slot: usize) {
        if mode < self.description.modes.len() && slot < usize::from(self.description.slot_count) {
            if self.selected != (mode, slot) {
                self.notice = None;
                self.write.reading_for = None;
            }
            self.selected = (mode, slot);
        }
    }

    /// The mode and 1-based slot number of the selected slot.
    #[must_use]
    pub fn selected_slot(&self) -> Option<(Mode, u8)> {
        let mode = self.description.modes.get(self.selected.0)?.id;
        let number = u8::try_from(self.selected.1 + 1).ok()?;
        Some((mode, number))
    }

    /// The state of slot `number` (1-based) of `mode` on the picked controller. An
    /// unread slot is empty.
    #[must_use]
    pub fn slot(&self, mode: Mode, number: u8) -> SlotState {
        self.active().and_then(|c| c.slots.get(&(mode, number))).cloned().unwrap_or_default()
    }

    /// The selected slot of the picked controller, for a change.
    fn selected_mut(&mut self) -> Option<&mut SlotState> {
        let key = self.selected_slot()?;
        Some(self.active_mut()?.slots.entry(key).or_default())
    }

    /// Applies `change` to the selected slot's working copy. The first change copies
    /// what the controller holds. An empty slot has nothing to change.
    pub fn edit(&mut self, change: impl FnOnce(&mut CanonicalProfile)) {
        let Some(slot) = self.selected_mut() else { return };
        let Some(mut edited) = slot.shown().cloned() else { return };
        change(&mut edited);
        slot.edited = Some(edited);
    }

    /// Gives `button` the output `target` in the selected slot, and drops the macro
    /// it starts, because the controller sends a button's macro in place of its
    /// output. An unrecognised output is read-only: a button can leave it, never go
    /// back to it.
    pub fn set_output(&mut self, button: &str, target: &str) {
        if target == UNRECOGNISED_OUTPUT {
            return;
        }
        self.edit(|p| {
            p.macro_refs.retain(|m| m.trigger != button);
            let mapping = ButtonMapping { source: button.to_owned(), target: target.to_owned() };
            match p.button_mappings.iter_mut().find(|m| m.source == button) {
                Some(m) => *m = mapping,
                None => p.button_mappings.push(mapping),
            }
        });
    }

    /// Renames the selected slot's profile, cut to the controller's name length.
    pub fn set_name(&mut self, name: &str) {
        let max = usize::try_from(self.description.limits.profile_name_length.max).unwrap_or(0);
        let name: String = name.chars().take(max).collect();
        self.edit(|p| p.name = name);
    }

    /// Drops the selected slot's working copy.
    pub fn discard(&mut self) {
        if let Some(slot) = self.selected_mut() {
            slot.edited = None;
        }
    }

    /// Fills an empty selected slot's working copy with the mode's default profile.
    pub fn start_from_default(&mut self) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some(mut profile) = self.defaults.get(&mode).cloned() else { return };
        NEW_PROFILE_NAME.clone_into(&mut profile.name);
        if let Some(slot) = self.selected_mut().filter(|s| s.shown().is_none()) {
            slot.edited = Some(profile);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
pub mod tests {
    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::Pro3;
    use controller_core::model::{CanonicalProfileSummary, ProfileReadResult, RawProfilePayload};

    use super::*;

    #[test]
    fn a_send_to_a_stopped_worker_shows_the_stopped_message() {
        let mut s = new_state();
        let (tx, rx) = std::sync::mpsc::channel();
        assert!(s.send(&tx, Command::InstallUdevRule));
        assert!(!s.worker_stopped);
        drop(rx);
        assert!(!s.send(&tx, Command::InstallUdevRule));
        assert!(s.worker_stopped);
    }

    /// The USB port path of the controller [`connected`] reads.
    pub const PORT: &str = "8-5";

    pub fn description() -> &'static ControllerDescription {
        Pro3.description().unwrap()
    }

    /// A window for the Pro 3 with no controller seen yet.
    pub fn new_state() -> AppState {
        let defaults = Mode::ALL.iter().map(|&m| (m, Pro3.default_profile(m))).collect();
        AppState::new(description(), defaults)
    }

    /// A summary for slot `number` of `mode`, named `name`; an empty name makes it
    /// an empty slot, as the read returns one.
    pub fn summary(mode: Mode, number: u8, name: &str, macros: usize) -> CanonicalProfileSummary {
        let raw = RawProfilePayload {
            payload: std::fs::read("../../fixtures/pro3/xinput.blob").expect("fixture"),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: mode,
        };
        let mut canonical = Pro3.map_profile(&raw).expect("profile").canonical;
        canonical.name = name.to_owned();
        canonical.macro_refs.clear();
        for i in 0..macros {
            canonical.macro_refs.push(controller_core::model::MacroRef {
                trigger: "p1".to_owned(),
                path: format!("macro-{i}.json"),
            });
        }
        let id = if name.is_empty() { String::new() } else { format!("{mode}-{number}") };
        CanonicalProfileSummary {
            id,
            name: name.to_owned(),
            mode,
            source_slot: number,
            source_profile_index: number - 1,
            canonical,
        }
    }

    /// A full read: slot 1 of each mode named after it, slot 2 with two macros,
    /// slot 3 empty.
    pub fn full_read() -> ProfileReadResult {
        let profiles = Mode::ALL
            .iter()
            .flat_map(|&m| {
                [summary(m, 1, m.label(), 0), summary(m, 2, "Racing", 2), summary(m, 3, "", 0)]
            })
            .collect();
        ProfileReadResult { profiles, raw_blobs: vec![] }
    }

    /// [`full_read`] with the `XInput` fixture bank as each mode's raw bank: slot 1
    /// active, which a write's readback needs.
    pub fn readable() -> ProfileReadResult {
        let blob = std::fs::read("../../fixtures/pro3/xinput.blob").expect("fixture");
        ProfileReadResult { raw_blobs: vec![blob; 3], ..full_read() }
    }

    #[test]
    fn starts_with_no_controller() {
        let s = new_state();
        assert!(!s.has_controller());
        assert_eq!(s.current_mode(), None);
    }

    #[test]
    fn first_read_fills_every_slot_and_selects_the_current_mode() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::Switch));
        s.read_started(PORT);
        s.read_finished(PORT, Ok(full_read()));
        assert!(s.has_controller() && !s.reading());
        assert_eq!(s.active().unwrap().slots.len(), 9);
        assert_eq!(s.slot(Mode::DInput, 3).pad, None);
        assert_eq!(s.slot(Mode::DInput, 2).pad.unwrap().name, "Racing");
        assert_eq!(s.selected_slot(), Some((Mode::Switch, 1)));
    }

    #[test]
    fn unplug_keeps_slots_and_edits() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_finished(PORT, Ok(full_read()));
        let mut edited = s.slot(Mode::XInput, 1).pad.unwrap();
        edited.name = "Edited".to_owned();
        s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 1)).unwrap().edited = Some(edited);
        s.presence(PORT, None);
        assert!(s.has_controller());
        assert_eq!(s.current_mode(), None);
        assert!(s.slot(Mode::XInput, 1).unsaved());
        // A reread replaces what the pad holds and keeps the edit.
        s.presence(PORT, Some(Mode::DInput));
        s.read_finished(PORT, Ok(full_read()));
        assert!(s.slot(Mode::XInput, 1).unsaved());
        assert_eq!(
            s.selected_slot(),
            Some((Mode::XInput, 1)),
            "selection stays after the first read"
        );
    }

    #[test]
    fn failed_read_is_kept_until_it_clears_or_the_controller_goes() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_started(PORT);
        s.read_finished(PORT, Err("device disconnected".to_owned()));
        assert_eq!(s.read_error(), Some("device disconnected"));
        assert!(!s.reading() && !s.has_controller());
        s.presence(PORT, Some(Mode::XInput));
        assert_eq!(s.read_error(), None);
        s.read_finished(PORT, Err("device disconnected".to_owned()));
        s.read_started(PORT);
        assert!(s.read_error().is_some(), "the failure stays while the read runs again");
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(s.read_error(), None, "a good read clears it");
        s.read_finished(PORT, Err("device disconnected".to_owned()));
        s.presence(PORT, None);
        assert_eq!(s.read_error(), None, "an unplug clears it");
    }

    #[test]
    fn denied_read_asks_for_access_until_a_good_read() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_started(PORT);
        s.read_denied(PORT, Rule::Missing);
        assert_eq!(s.access, Some(Access::Denied));
        assert!(!s.reading());
        s.install_started();
        assert_eq!(s.install, Install::Running);
        s.install_finished(Err("The password prompt was closed.".to_owned()));
        assert!(matches!(s.install, Install::Failed(_)));
        s.install_started();
        assert_eq!(s.install, Install::Running, "a retry clears the old failure");
        s.install_finished(Ok(()));
        assert_eq!(s.rule, Rule::Current);
        s.read_denied(PORT, Rule::Current);
        assert_eq!(s.access, Some(Access::StillDenied));
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(s.access, None);
    }

    #[test]
    fn the_sandbox_asks_only_while_a_read_is_denied() {
        let mut s = new_state();
        s.sandboxed = true;
        s.presence(PORT, Some(Mode::XInput));
        // The sandbox cannot see the host's rule, so it reads as missing.
        s.read_denied(PORT, Rule::Missing);
        assert_eq!(s.access, Some(Access::Denied));
        assert!(s.asks_for_rule());
        s.read_finished(PORT, Ok(full_read()));
        assert!(!s.asks_for_rule(), "a good read ends the question");
    }

    #[test]
    fn still_blocked_shows_once_a_check_comes_back_denied() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_denied(PORT, Rule::Current);
        assert!(!s.still_blocked(), "no check yet");
        s.check_started();
        s.read_started(PORT);
        assert!(!s.still_blocked(), "the read is running");
        s.read_denied(PORT, Rule::Current);
        assert!(s.still_blocked());
        s.install_started();
        assert!(!s.still_blocked(), "an install replaces the warning");
        s.install_finished(Err("closed".to_owned()));
        s.check_started();
        assert_eq!(s.install, Install::Idle, "a check replaces the install error");
        s.read_finished(PORT, Ok(full_read()));
        assert!(!s.still_blocked() && !s.rechecked);
    }

    #[test]
    fn unplug_leaves_the_permission_screen() {
        let mut s = new_state();
        s.presence(PORT, Some(Mode::XInput));
        s.read_denied(PORT, Rule::Current);
        s.presence(PORT, None);
        assert_eq!(s.access, None);
    }

    #[test]
    fn missing_rule_asks_only_when_denied() {
        let mut s = new_state();
        s.rule = Rule::Missing;
        assert!(!s.asks_for_rule(), "no read has been denied yet");
        s.presence(PORT, Some(Mode::XInput));
        s.read_finished(PORT, Ok(full_read()));
        assert!(!s.asks_for_rule(), "another rule grants access");
        s.read_denied(PORT, Rule::Missing);
        assert!(s.asks_for_rule());
    }

    #[test]
    fn outdated_rule_asks_until_installed_or_skipped() {
        let mut s = connected(Mode::XInput);
        assert!(!s.asks_for_rule());
        s.rule = Rule::Outdated;
        assert!(s.asks_for_rule(), "no denied read needed");
        s.presence(PORT, None);
        assert!(s.asks_for_rule(), "an unplug keeps the question");
        s.rule_skipped = true;
        assert!(!s.asks_for_rule());
        s.presence(PORT, Some(Mode::XInput));
        s.read_denied(PORT, Rule::Outdated);
        assert!(s.asks_for_rule(), "a denied read asks again after a skip");
        s.install_finished(Ok(()));
        s.read_finished(PORT, Ok(full_read()));
        assert!(!s.asks_for_rule());
    }

    #[test]
    fn select_ignores_out_of_range() {
        let mut s = new_state();
        s.select(2, 2);
        assert_eq!(s.selected_slot(), Some((Mode::DInput, 3)));
        s.select(3, 0);
        s.select(0, 3);
        assert_eq!(s.selected, (2, 2));
    }

    /// A Pro 3 read in `mode`.
    pub fn connected(mode: Mode) -> AppState {
        let mut s = new_state();
        s.presence(PORT, Some(mode));
        s.read_finished(PORT, Ok(full_read()));
        s
    }

    #[test]
    fn first_change_copies_the_pad_and_marks_its_tab() {
        let mut s = connected(Mode::XInput);
        assert_eq!(s.slot(Mode::XInput, 1).dirty(), Dirty::default());
        s.set_output("r1", "disabled");
        let slot = s.slot(Mode::XInput, 1);
        assert!(slot.unsaved());
        assert_eq!(slot.dirty(), Dirty { buttons: true, ..Dirty::default() });
        // Changing it back leaves a working copy equal to the pad: nothing unsaved.
        s.set_output("r1", "r1");
        assert!(!s.slot(Mode::XInput, 1).unsaved());
        assert_eq!(s.slot(Mode::XInput, 1).dirty(), Dirty::default());
    }

    #[test]
    fn a_rename_marks_no_tab_and_is_cut_to_the_limit() {
        let mut s = connected(Mode::XInput);
        s.set_name("A name far longer than sixteen");
        let slot = s.slot(Mode::XInput, 1);
        assert_eq!(slot.shown().unwrap().name, "A name far longe");
        assert!(slot.unsaved());
        assert_eq!(slot.dirty(), Dirty::default());
    }

    #[test]
    fn discard_drops_the_working_copy_of_the_selected_slot_only() {
        let mut s = connected(Mode::XInput);
        s.set_output("l1", "disabled");
        s.select(0, 1);
        s.set_output("l1", "disabled");
        s.discard();
        assert!(!s.slot(Mode::XInput, 2).unsaved());
        assert!(s.slot(Mode::XInput, 1).unsaved());
    }

    #[test]
    fn an_empty_slot_starts_from_the_mode_default() {
        let mut s = connected(Mode::Switch);
        s.select(1, 2);
        s.set_output("l1", "disabled");
        assert_eq!(s.slot(Mode::Switch, 3).edited, None, "an empty slot has nothing to edit");
        s.start_from_default();
        let slot = s.slot(Mode::Switch, 3);
        let p = slot.shown().unwrap();
        assert_eq!((p.name.as_str(), p.mode), (NEW_PROFILE_NAME, Mode::Switch));
        assert!(slot.unsaved());
        assert_eq!(
            slot.dirty(),
            Dirty { buttons: true, sticks: true, triggers: true, vibration: true }
        );
        // A second press keeps the edits made since.
        s.set_output("l1", "disabled");
        s.start_from_default();
        let p = s.slot(Mode::Switch, 3).edited.unwrap();
        assert!(p.button_mappings.iter().any(|m| m.source == "l1" && m.target == "disabled"));
    }

    #[test]
    fn an_unrecognised_output_can_be_left_but_not_chosen() {
        let mut s = connected(Mode::DInput);
        s.set_output("l1", UNRECOGNISED_OUTPUT);
        assert_eq!(s.slot(Mode::DInput, 1).edited, None);
        s.active_mut()
            .unwrap()
            .slots
            .get_mut(&(Mode::DInput, 1))
            .unwrap()
            .pad
            .as_mut()
            .unwrap()
            .button_mappings[0]
            .target = UNRECOGNISED_OUTPUT.to_owned();
        s.set_output("right face", "disabled");
        s.set_output("right face", UNRECOGNISED_OUTPUT);
        let edited = s.slot(Mode::DInput, 1).edited.unwrap();
        assert_eq!(edited.button_mappings[0].target, "disabled");
    }
}
