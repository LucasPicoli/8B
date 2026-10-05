//! App state on the UI thread: what the controller holds, the edits, and what the
//! window shows. Pure: no Slint, no I/O.

use std::collections::BTreeMap;

use controller_core::description::{ControllerDescription, UNRECOGNISED_OUTPUT};
use controller_core::model::{ButtonMapping, CanonicalProfile, Mode, ProfileReadResult};

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
                buttons: p.button_mappings != e.button_mappings,
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

/// The installed udev rule and keepalive unit, against the ones this build installs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rule {
    /// Both files match.
    Current,
    /// No rule file.
    Missing,
    /// The rule file is from an older build, or the unit is missing or differs.
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

/// Everything the window shows, kept between renders.
#[derive(Debug)]
pub struct AppState {
    /// The connected controller model.
    pub description: &'static ControllerDescription,
    /// The profile a new slot starts from, per mode, with an empty name.
    pub defaults: BTreeMap<Mode, CanonicalProfile>,
    /// The mode the controller presents now. `None` while no controller is present.
    pub current_mode: Option<Mode>,
    /// Every slot read so far, by mode and 1-based number. Empty until the first read.
    pub slots: BTreeMap<(Mode, u8), SlotState>,
    /// The slot beside the sidebar: an index into the description's modes, and a
    /// 0-based slot.
    pub selected: (usize, usize),
    /// Whether a read is running.
    pub reading: bool,
    /// Why the last read failed. Cleared by the next connection.
    pub read_error: Option<String>,
    /// Set while opening the controller is denied: the permission screen shows.
    pub access: Option<Access>,
    /// Where the udev rule install stands.
    pub install: Install,
    /// The installed rule. Anything but current shows the permission screen until
    /// installed or skipped.
    pub rule: Rule,
    /// The user skipped the install for this run.
    pub rule_skipped: bool,
}

impl AppState {
    /// A window with no controller seen yet. `defaults` holds the profile a new slot
    /// of each mode starts from.
    #[must_use]
    pub const fn new(
        description: &'static ControllerDescription,
        defaults: BTreeMap<Mode, CanonicalProfile>,
    ) -> Self {
        Self {
            description,
            defaults,
            current_mode: None,
            slots: BTreeMap::new(),
            selected: (0, 0),
            reading: false,
            read_error: None,
            access: None,
            install: Install::Idle,
            rule: Rule::Current,
            rule_skipped: false,
        }
    }

    /// Whether a controller has been read since the app started. Until then the
    /// window shows only the no-controller screen.
    #[must_use]
    pub fn has_controller(&self) -> bool {
        !self.slots.is_empty()
    }

    /// A controller appeared in `mode`, or went away (`None`). Edits stay either way.
    pub fn presence(&mut self, mode: Option<Mode>) {
        self.current_mode = mode;
        self.reading = false;
        self.access = None;
        self.install = Install::Idle;
        if mode.is_some() {
            self.read_error = None;
        }
    }

    /// A read was sent to the worker.
    pub const fn read_started(&mut self) {
        self.reading = true;
    }

    /// A read came back. A success replaces every slot's `pad` and keeps the edits.
    pub fn read_finished(&mut self, result: Result<ProfileReadResult, String>) {
        self.reading = false;
        let read = match result {
            Ok(read) => read,
            Err(e) => {
                self.read_error = Some(e);
                return;
            }
        };
        let first_ever = self.slots.is_empty();
        for summary in read.profiles {
            // An empty slot reads back as a summary with no id.
            let pad = (!summary.id.is_empty()).then_some(summary.canonical);
            self.slots.entry((summary.mode, summary.source_slot)).or_default().pad = pad;
        }
        self.read_error = None;
        self.access = None;
        if first_ever {
            let current = self.current_mode;
            if let Some(i) = self.description.modes.iter().position(|m| Some(m.id) == current) {
                self.selected = (i, 0);
            }
        }
    }

    /// A read was denied the controller. `rule` is the installed rule now.
    pub fn read_denied(&mut self, rule: Rule) {
        self.reading = false;
        self.rule = rule;
        self.access =
            Some(if rule == Rule::Current { Access::StillDenied } else { Access::Denied });
    }

    /// Whether the permission screen fills the window.
    #[must_use]
    pub fn asks_for_rule(&self) -> bool {
        self.access.is_some() || (self.rule != Rule::Current && !self.rule_skipped)
    }

    /// The udev rule install was sent to the worker.
    pub fn install_started(&mut self) {
        self.install = Install::Running;
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

    /// The state of slot `number` (1-based) of `mode`. An unread slot is empty.
    #[must_use]
    pub fn slot(&self, mode: Mode, number: u8) -> SlotState {
        self.slots.get(&(mode, number)).cloned().unwrap_or_default()
    }

    /// The selected slot, for a change.
    fn selected_mut(&mut self) -> Option<&mut SlotState> {
        let key = self.selected_slot()?;
        Some(self.slots.entry(key).or_default())
    }

    /// Applies `change` to the selected slot's working copy. The first change copies
    /// what the controller holds. An empty slot has nothing to change.
    pub fn edit(&mut self, change: impl FnOnce(&mut CanonicalProfile)) {
        let Some(slot) = self.selected_mut() else { return };
        let Some(mut edited) = slot.shown().cloned() else { return };
        change(&mut edited);
        slot.edited = Some(edited);
    }

    /// Gives `button` the output `target` in the selected slot. An unrecognised
    /// output is read-only: a button can leave it, never go back to it.
    pub fn set_output(&mut self, button: &str, target: &str) {
        if target == UNRECOGNISED_OUTPUT {
            return;
        }
        self.edit(|p| {
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
        canonical.macro_refs.truncate(0);
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

    #[test]
    fn starts_with_no_controller() {
        let s = new_state();
        assert!(!s.has_controller());
        assert_eq!(s.current_mode, None);
    }

    #[test]
    fn first_read_fills_every_slot_and_selects_the_current_mode() {
        let mut s = new_state();
        s.presence(Some(Mode::Switch));
        s.read_started();
        s.read_finished(Ok(full_read()));
        assert!(s.has_controller() && !s.reading);
        assert_eq!(s.slots.len(), 9);
        assert_eq!(s.slot(Mode::DInput, 3).pad, None);
        assert_eq!(s.slot(Mode::DInput, 2).pad.unwrap().name, "Racing");
        assert_eq!(s.selected_slot(), Some((Mode::Switch, 1)));
    }

    #[test]
    fn unplug_keeps_slots_and_edits() {
        let mut s = new_state();
        s.presence(Some(Mode::XInput));
        s.read_finished(Ok(full_read()));
        let mut edited = s.slot(Mode::XInput, 1).pad.unwrap();
        edited.name = "Edited".to_owned();
        s.slots.get_mut(&(Mode::XInput, 1)).unwrap().edited = Some(edited);
        s.presence(None);
        assert!(s.has_controller());
        assert_eq!(s.current_mode, None);
        assert!(s.slot(Mode::XInput, 1).unsaved());
        // A reread replaces what the pad holds and keeps the edit.
        s.presence(Some(Mode::DInput));
        s.read_finished(Ok(full_read()));
        assert!(s.slot(Mode::XInput, 1).unsaved());
        assert_eq!(
            s.selected_slot(),
            Some((Mode::XInput, 1)),
            "selection stays after the first read"
        );
    }

    #[test]
    fn failed_read_is_kept_until_the_next_connection() {
        let mut s = new_state();
        s.presence(Some(Mode::XInput));
        s.read_started();
        s.read_finished(Err("device disconnected".to_owned()));
        assert_eq!(s.read_error.as_deref(), Some("device disconnected"));
        assert!(!s.reading && !s.has_controller());
        s.presence(Some(Mode::XInput));
        assert_eq!(s.read_error, None);
    }

    #[test]
    fn denied_read_asks_for_access_until_a_good_read() {
        let mut s = new_state();
        s.presence(Some(Mode::XInput));
        s.read_started();
        s.read_denied(Rule::Missing);
        assert_eq!(s.access, Some(Access::Denied));
        assert!(!s.reading);
        s.install_started();
        assert_eq!(s.install, Install::Running);
        s.install_finished(Err("The password prompt was closed.".to_owned()));
        assert!(matches!(s.install, Install::Failed(_)));
        s.install_started();
        assert_eq!(s.install, Install::Running, "a retry clears the old failure");
        s.install_finished(Ok(()));
        assert_eq!(s.rule, Rule::Current);
        s.read_denied(Rule::Current);
        assert_eq!(s.access, Some(Access::StillDenied));
        s.read_finished(Ok(full_read()));
        assert_eq!(s.access, None);
    }

    #[test]
    fn unplug_leaves_the_permission_screen() {
        let mut s = new_state();
        s.presence(Some(Mode::XInput));
        s.read_denied(Rule::Current);
        s.presence(None);
        assert_eq!(s.access, None);
    }

    #[test]
    fn outdated_rule_asks_until_installed_or_skipped() {
        let mut s = connected(Mode::XInput);
        assert!(!s.asks_for_rule());
        s.rule = Rule::Outdated;
        assert!(s.asks_for_rule(), "no denied read needed");
        s.presence(None);
        assert!(s.asks_for_rule(), "an unplug keeps the question");
        s.rule_skipped = true;
        assert!(!s.asks_for_rule());
        s.read_denied(Rule::Outdated);
        assert!(s.asks_for_rule(), "a denied read asks again after a skip");
        s.install_finished(Ok(()));
        s.read_finished(Ok(full_read()));
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
        s.presence(Some(mode));
        s.read_finished(Ok(full_read()));
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
        s.slots.get_mut(&(Mode::DInput, 1)).unwrap().pad.as_mut().unwrap().button_mappings[0]
            .target = UNRECOGNISED_OUTPUT.to_owned();
        s.set_output("right face", "disabled");
        s.set_output("right face", UNRECOGNISED_OUTPUT);
        let edited = s.slot(Mode::DInput, 1).edited.unwrap();
        assert_eq!(edited.button_mappings[0].target, "disabled");
    }
}
