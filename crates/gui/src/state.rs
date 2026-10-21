//! App state on the UI thread: what each controller holds, the edits, and what the
//! window shows. Pure: no Slint, no I/O.

use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc::Sender;

use controller_core::description::{ControllerDescription, UNRECOGNISED_OUTPUT};
use controller_core::device::Model;
use controller_core::devices;
use controller_core::model::{ButtonMapping, CanonicalProfile, MacroDefinition, Mode};
use tr::tr;

use crate::controllers::Controller;
use crate::files::PendingImport;
use crate::macros::{macro_number, PadMacro};
use crate::review::WriteState;
use crate::worker::Command;

/// `name` cut to `max` UTF-16 units, the unit the controller stores a name in.
#[must_use]
pub fn cut_name(name: &str, max: i32) -> String {
    let max = usize::try_from(max).unwrap_or(0);
    let mut used = 0;
    name.chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= max
        })
        .collect()
}

/// One slot: what the controller held at the last read, and the working copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotState {
    /// The profile read from the controller. `None` when the slot is empty.
    pub pad: Option<CanonicalProfile>,
    /// The edited working copy. `None` until the slot is edited.
    pub edited: Option<CanonicalProfile>,
    /// The macros the controller held at the last read, in macro-slot order. Empty
    /// for an empty slot, whatever descriptors it keeps.
    pub pad_macros: Vec<PadMacro>,
    /// The edited macros. `None` until a macro of the slot is edited.
    pub edited_macros: Option<Vec<MacroDefinition>>,
}

/// Which tabs of a slot hold unsaved edits.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Dirty {
    /// Button mappings.
    pub buttons: bool,
    /// The ids of the settings tabs with an edit.
    pub tabs: BTreeSet<String>,
    /// The macros.
    pub macros: bool,
}

impl Dirty {
    /// Whether the settings tab `id` holds an edit.
    #[must_use]
    pub fn tab(&self, id: &str) -> bool {
        self.tabs.contains(id)
    }
}

impl SlotState {
    /// Whether the working copy or the edited macros differ from what the controller
    /// holds.
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.profile_unsaved() || self.macros_unsaved()
    }

    /// Whether the working copy differs from the profile the controller holds.
    #[must_use]
    pub fn profile_unsaved(&self) -> bool {
        self.edited.as_ref().is_some_and(|e| self.pad.as_ref() != Some(e))
    }

    /// Whether the edited macros differ from the macros the controller holds.
    #[must_use]
    pub fn macros_unsaved(&self) -> bool {
        self.edited_macros
            .as_ref()
            .is_some_and(|e| !e.iter().eq(self.pad_macros.iter().map(|m| &m.def)))
    }

    /// The profile on screen: the working copy, else what the controller holds.
    #[must_use]
    pub fn shown(&self) -> Option<&CanonicalProfile> {
        self.edited.as_ref().or(self.pad.as_ref())
    }

    /// Which tabs of `mode` on `description` differ between the working copy and what
    /// the controller holds. A working copy of an empty slot differs everywhere.
    #[must_use]
    pub fn dirty(&self, description: &ControllerDescription, mode: Mode) -> Dirty {
        let json = |p: &CanonicalProfile| serde_json::to_value(p).unwrap_or_default();
        let macros = self.macros_unsaved();
        let starters = self.starters_changed();
        let (pad, edited) = match (&self.pad, &self.edited) {
            (_, None) => return Dirty { macros, ..Dirty::default() },
            (None, Some(_)) => {
                let tabs = description.pages(mode).map(|p| p.id.clone()).collect();
                return Dirty { buttons: true, tabs, macros };
            }
            (Some(p), Some(e)) => (p, e),
        };
        let (old, new) = (json(pad), json(edited));
        let changed = |field: &str| old.pointer(field) != new.pointer(field);
        Dirty {
            buttons: pad.button_mappings != edited.button_mappings
                || pad.macro_refs != edited.macro_refs
                || starters,
            tabs: description
                .pages(mode)
                .filter(|p| p.fields().any(changed))
                .map(|p| p.id.clone())
                .collect(),
            macros,
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
    /// The model shown before any controller is read: the first supported one.
    pub fallback_description: &'static ControllerDescription,
    /// The profile a new slot of the fallback model starts from, per mode, with an
    /// empty name.
    pub fallback_defaults: BTreeMap<Mode, CanonicalProfile>,
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
    /// A macro file picked in the Macros tab, waiting for the user's yes.
    pub pending_macro_import: Option<crate::macro_files::PendingMacroImport>,
    /// The review, the clear question and the write in flight.
    pub write: WriteState,
    /// The window was asked to close over unsaved edits: the question is open.
    pub closing: bool,
    /// The locked controllers attached, each with where its unlock stands.
    pub locked: Vec<crate::locked::LockedPad>,
    /// The port of the locked pad picked in the header, if one is.
    pub locked_shown: Option<String>,
    /// The relock question is open for the shown controller.
    pub relock_asking: bool,
    /// The port whose edited slots are written first, then relocked.
    pub relock_after_write: Option<String>,
    /// The port whose relock the worker has and has not answered yet.
    pub relock_sent: Option<String>,
    /// A command could not reach the worker: its thread has ended, so the stopped
    /// message shows. The worker is not restarted.
    pub worker_stopped: bool,
    /// The step edits of the Macros tab, oldest first, which Ctrl+Z takes back.
    pub step_undo: Vec<crate::step_table::StepUndo>,
    /// The description whose views the window last drew. [`crate::render::render`]
    /// draws them again when the shown controller is another model.
    pub views_of: Cell<Option<&'static ControllerDescription>>,
}

impl AppState {
    /// The description of the controller the window shows, or the fallback model's
    /// while that controller has not been read.
    #[must_use]
    pub fn description(&self) -> &'static ControllerDescription {
        self.active().map_or(self.fallback_description, |c| self.description_of(c))
    }

    /// The description of controller `c`, or the fallback before its model is known.
    #[must_use]
    pub fn description_of(&self, c: &Controller) -> &'static ControllerDescription {
        c.description.unwrap_or(self.fallback_description)
    }

    /// Which tabs of slot `number` of `mode` hold unsaved edits, on the shown controller.
    #[must_use]
    pub fn slot_dirty(&self, mode: Mode, number: u8) -> Dirty {
        self.slot(mode, number).dirty(self.description(), mode)
    }

    /// The model of the controller the window shows, or of the fallback model while that
    /// controller has not been read. `None` for a model not in the registry.
    #[must_use]
    pub fn model(&self) -> Option<&'static dyn Model> {
        let shown = self.description();
        devices::models()
            .iter()
            .copied()
            .find(|m| m.description().is_ok_and(|d| std::ptr::eq(d, shown)))
    }

    /// The profile a new slot of each mode starts from, for the controller the window
    /// shows, or for the fallback model while that controller has not been read.
    #[must_use]
    pub fn defaults(&self) -> &BTreeMap<Mode, CanonicalProfile> {
        self.active()
            .filter(|c| c.description.is_some())
            .map_or(&self.fallback_defaults, |c| &c.defaults)
    }

    /// A window with no controller seen yet. `defaults` holds the profile a new slot
    /// of each mode starts from.
    #[must_use]
    pub fn new(
        description: &'static ControllerDescription,
        defaults: BTreeMap<Mode, CanonicalProfile>,
    ) -> Self {
        Self {
            fallback_description: description,
            fallback_defaults: defaults,
            controllers: Vec::new(),
            locked: Vec::new(),
            locked_shown: None,
            relock_asking: false,
            relock_after_write: None,
            relock_sent: None,
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
            pending_macro_import: None,
            write: WriteState::default(),
            closing: false,
            worker_stopped: false,
            step_undo: Vec::new(),
            views_of: Cell::new(None),
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
        if mode < self.description().modes.len()
            && slot < usize::from(self.description().slot_count)
        {
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
        let mode = self.description().modes.get(self.selected.0)?.id;
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
    pub(crate) fn selected_mut(&mut self) -> Option<&mut SlotState> {
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

    /// Gives `button` the output `target` in the selected slot, and takes it off the
    /// macro it starts, because the controller sends a button's macro in place of its
    /// output. The macro stays, with no button if that was its last. A macro output
    /// ([`crate::macros::macro_output`]) moves the button into that macro instead. An
    /// unrecognised output is read-only: a button can leave it, never go back to it.
    pub fn set_output(&mut self, button: &str, target: &str) {
        if let Some(macro_slot) = macro_number(target) {
            return self.join_macro(button, macro_slot);
        }
        if target == UNRECOGNISED_OUTPUT {
            return;
        }
        self.leave_macros(&[button.to_owned()]);
        self.edit(|p| {
            let mapping = ButtonMapping { source: button.to_owned(), target: target.to_owned() };
            match p.button_mappings.iter_mut().find(|m| m.source == button) {
                Some(m) => *m = mapping,
                None => p.button_mappings.push(mapping),
            }
        });
    }

    /// Renames the selected slot's profile, cut to the controller's name length. A
    /// model that cannot rename a profile keeps the name.
    pub fn set_name(&mut self, name: &str) {
        if !self.description().profile_rename {
            return;
        }
        let name = cut_name(name, self.description().limits.profile_name_length.max);
        self.edit(|p| p.name = name);
    }

    /// Drops the selected slot's working copy and edited macros.
    pub fn discard(&mut self) {
        if let Some(slot) = self.selected_mut() {
            slot.edited = None;
            slot.edited_macros = None;
        }
    }

    /// Fills an empty selected slot's working copy with the mode's default profile,
    /// named "New profile" where the model can rename it.
    pub fn start_from_default(&mut self) {
        let Some((mode, _)) = self.selected_slot() else { return };
        let Some(mut profile) = self.defaults().get(&mode).cloned() else { return };
        // A default name is data once made: it keeps the language it was made in. A
        // model that cannot rename a profile keeps the default's name.
        if self.description().profile_rename {
            profile.name =
                cut_name(&tr!("New profile"), self.description().limits.profile_name_length.max);
        }
        if let Some(slot) = self.selected_mut().filter(|s| s.shown().is_none()) {
            slot.edited = Some(profile);
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
pub mod tests {
    /// The registered Ultimate.
    fn ultimate_model() -> &'static dyn controller_core::device::Model {
        controller_core::devices::model_named("Ultimate (BT)").unwrap()
    }
    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::{Pro3, DINPUT, MODES, SWITCH, XINPUT};
    use controller_core::model::{
        CanonicalProfileSummary, MacroDefinition, MacroStep, ProfileReadResult, RawProfilePayload,
    };

    use super::*;
    use crate::macros::PadMacro;

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
        crate::string_coverage::wrap_rust_text();
        let defaults = MODES.iter().map(|&m| (m, Pro3.default_profile(m))).collect();
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
                trigger_buttons: vec!["p1".to_owned()],
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
        let profiles = MODES
            .iter()
            .flat_map(|&m| {
                [
                    summary(m, 1, &description().mode_label(m), 0),
                    summary(m, 2, "Racing", 2),
                    summary(m, 3, "", 0),
                ]
            })
            .collect();
        ProfileReadResult { profiles, raw_blobs: vec![], ..Default::default() }
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
        s.presence(PORT, Some(SWITCH));
        s.read_started(PORT);
        s.read_finished(PORT, Ok(full_read()));
        assert!(s.has_controller() && !s.reading());
        assert_eq!(s.active().unwrap().slots.len(), 9);
        assert_eq!(s.slot(DINPUT, 3).pad, None);
        assert_eq!(s.slot(DINPUT, 2).pad.unwrap().name, "Racing");
        assert_eq!(s.selected_slot(), Some((SWITCH, 1)));
    }

    #[test]
    fn unplug_keeps_slots_and_edits() {
        let mut s = new_state();
        s.presence(PORT, Some(XINPUT));
        s.read_finished(PORT, Ok(full_read()));
        let mut edited = s.slot(XINPUT, 1).pad.unwrap();
        edited.name = "Edited".to_owned();
        s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().edited = Some(edited);
        s.presence(PORT, None);
        assert!(s.has_controller());
        assert_eq!(s.current_mode(), None);
        assert!(s.slot(XINPUT, 1).unsaved());
        // A reread replaces what the pad holds and keeps the edit.
        s.presence(PORT, Some(DINPUT));
        s.read_finished(PORT, Ok(full_read()));
        assert!(s.slot(XINPUT, 1).unsaved());
        assert_eq!(s.selected_slot(), Some((XINPUT, 1)), "selection stays after the first read");
    }

    #[test]
    fn failed_read_is_kept_until_it_clears_or_the_controller_goes() {
        let mut s = new_state();
        s.presence(PORT, Some(XINPUT));
        s.read_started(PORT);
        s.read_finished(PORT, Err("device disconnected".to_owned()));
        assert_eq!(s.read_error(), Some("device disconnected"));
        assert!(!s.reading() && !s.has_controller());
        s.presence(PORT, Some(XINPUT));
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
        s.presence(PORT, Some(XINPUT));
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
        s.presence(PORT, Some(XINPUT));
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
        s.presence(PORT, Some(XINPUT));
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
        s.presence(PORT, Some(XINPUT));
        s.read_denied(PORT, Rule::Current);
        s.presence(PORT, None);
        assert_eq!(s.access, None);
    }

    #[test]
    fn missing_rule_asks_only_when_denied() {
        let mut s = new_state();
        s.rule = Rule::Missing;
        assert!(!s.asks_for_rule(), "no read has been denied yet");
        s.presence(PORT, Some(XINPUT));
        s.read_finished(PORT, Ok(full_read()));
        assert!(!s.asks_for_rule(), "another rule grants access");
        s.read_denied(PORT, Rule::Missing);
        assert!(s.asks_for_rule());
    }

    #[test]
    fn outdated_rule_asks_until_installed_or_skipped() {
        let mut s = connected(XINPUT);
        assert!(!s.asks_for_rule());
        s.rule = Rule::Outdated;
        assert!(s.asks_for_rule(), "no denied read needed");
        s.presence(PORT, None);
        assert!(s.asks_for_rule(), "an unplug keeps the question");
        s.rule_skipped = true;
        assert!(!s.asks_for_rule());
        s.presence(PORT, Some(XINPUT));
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
        assert_eq!(s.selected_slot(), Some((DINPUT, 3)));
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

    /// An Ultimate on firmware `firmware`, current mode `XInput`, read with slot 1 of
    /// each mode holding the mode's default profile with no name, slots 2 and 3 empty.
    pub fn ultimate(firmware: &str) -> AppState {
        use controller_core::devices::ultimate;
        let summary = |mode: Mode, number: u8| CanonicalProfileSummary {
            id: if number == 1 { format!("{mode}-{number}") } else { String::new() },
            name: String::new(),
            mode,
            source_slot: number,
            source_profile_index: number - 1,
            canonical: ultimate_model().default_profile(mode),
        };
        let mut s = new_state();
        s.presence(PORT, Some(ultimate::XINPUT));
        s.set_model(PORT, ultimate_model());
        let profiles =
            ultimate::MODES.iter().flat_map(|&m| (1..=3).map(move |n| summary(m, n))).collect();
        let read = ProfileReadResult {
            profiles,
            firmware_version: firmware.to_owned(),
            ..Default::default()
        };
        s.read_finished(PORT, Ok(read));
        s
    }

    /// A read of the test pad: slot 1 named `name`, slot 2 empty.
    pub fn test_pad_read(name: &str) -> ProfileReadResult {
        use controller_core::devices::test_pad::{self, TestPad};
        let summary = |number: u8, name: &str| {
            let mut canonical = TestPad.default_profile(test_pad::STANDARD);
            canonical.name = name.to_owned();
            let id =
                if name.is_empty() { String::new() } else { format!("standard-slot-{number}") };
            CanonicalProfileSummary {
                id,
                name: name.to_owned(),
                mode: test_pad::STANDARD,
                source_slot: number,
                source_profile_index: number - 1,
                canonical,
            }
        };
        ProfileReadResult { profiles: vec![summary(1, name), summary(2, "")], ..Default::default() }
    }

    /// A macro of `XInput` in macro slot `slot` (0-based) named `name`, started by
    /// `buttons`, with `steps` steps of 100 ms each.
    pub fn macro_def(slot: u8, name: &str, buttons: &[&str], steps: usize) -> MacroDefinition {
        MacroDefinition {
            name: name.to_owned(),
            mode: XINPUT,
            trigger_buttons: buttons.iter().map(|&b| b.to_owned()).collect(),
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![MacroStep { duration_ms: 100, ..MacroStep::default() }; steps],
            macro_slot: Some(slot),
        }
    }

    /// Slot `key` of the controller on [`PORT`] holds `macros`, their steps read.
    pub fn hold_macros(s: &mut AppState, key: (Mode, u8), macros: &[MacroDefinition]) {
        let slot = s.controller_mut(PORT).unwrap().slots.get_mut(&key).unwrap();
        slot.pad_macros =
            macros.iter().map(|def| PadMacro { def: def.clone(), loaded: true }).collect();
    }

    #[test]
    fn first_change_copies_the_pad_and_marks_its_tab() {
        let mut s = connected(XINPUT);
        assert_eq!(s.slot_dirty(XINPUT, 1), Dirty::default());
        s.set_output("r1", "disabled");
        let slot = s.slot(XINPUT, 1);
        assert!(slot.unsaved());
        assert_eq!(s.slot_dirty(XINPUT, 1), Dirty { buttons: true, ..Dirty::default() });
        // Changing it back leaves a working copy equal to the pad: nothing unsaved.
        s.set_output("r1", "r1");
        assert!(!s.slot(XINPUT, 1).unsaved());
        assert_eq!(s.slot_dirty(XINPUT, 1), Dirty::default());
    }

    #[test]
    fn a_rename_marks_no_tab_and_is_cut_to_the_limit() {
        let mut s = connected(XINPUT);
        s.set_name("A name far longer than sixteen");
        let slot = s.slot(XINPUT, 1);
        assert_eq!(slot.shown().unwrap().name, "A name far longe");
        assert!(slot.unsaved());
        assert_eq!(s.slot_dirty(XINPUT, 1), Dirty::default());
    }

    #[test]
    fn discard_drops_the_working_copy_of_the_selected_slot_only() {
        let mut s = connected(XINPUT);
        s.set_output("l1", "disabled");
        s.select(0, 1);
        s.set_output("l1", "disabled");
        s.discard();
        assert!(!s.slot(XINPUT, 2).unsaved());
        assert!(s.slot(XINPUT, 1).unsaved());
    }

    #[test]
    fn an_empty_slot_starts_from_the_mode_default() {
        let mut s = connected(SWITCH);
        s.select(1, 2);
        s.set_output("l1", "disabled");
        assert_eq!(s.slot(SWITCH, 3).edited, None, "an empty slot has nothing to edit");
        s.start_from_default();
        let slot = s.slot(SWITCH, 3);
        let p = slot.shown().unwrap();
        assert_eq!((p.name.as_str(), p.mode), ("New profile", SWITCH));
        assert!(slot.unsaved());
        let tabs = ["sticks", "triggers", "vibration"].map(str::to_owned).into();
        assert_eq!(s.slot_dirty(SWITCH, 3), Dirty { buttons: true, tabs, macros: false });
        // A second press keeps the edits made since.
        s.set_output("l1", "disabled");
        s.start_from_default();
        let p = s.slot(SWITCH, 3).edited.unwrap();
        assert!(p.button_mappings.iter().any(|m| m.source == "l1" && m.target == "disabled"));
    }

    #[test]
    fn an_unrecognised_output_can_be_left_but_not_chosen() {
        let mut s = connected(DINPUT);
        s.set_output("l1", UNRECOGNISED_OUTPUT);
        assert_eq!(s.slot(DINPUT, 1).edited, None);
        s.active_mut()
            .unwrap()
            .slots
            .get_mut(&(DINPUT, 1))
            .unwrap()
            .pad
            .as_mut()
            .unwrap()
            .button_mappings[0]
            .target = UNRECOGNISED_OUTPUT.to_owned();
        s.set_output("right face", "disabled");
        s.set_output("right face", UNRECOGNISED_OUTPUT);
        let edited = s.slot(DINPUT, 1).edited.unwrap();
        assert_eq!(edited.button_mappings[0].target, "disabled");
    }
}
