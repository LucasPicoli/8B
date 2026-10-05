//! App state on the UI thread: what the controller holds, the edits, and what the
//! window shows. Pure: no Slint, no I/O.

use std::collections::BTreeMap;

use controller_core::description::ControllerDescription;
use controller_core::model::{CanonicalProfile, Mode, ProfileReadResult};

/// One slot: what the controller held at the last read, and the working copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SlotState {
    /// The profile read from the controller. `None` when the slot is empty.
    pub pad: Option<CanonicalProfile>,
    /// The edited working copy. `None` until the slot is edited.
    pub edited: Option<CanonicalProfile>,
}

impl SlotState {
    /// Whether the working copy differs from what the controller holds.
    #[must_use]
    pub fn unsaved(&self) -> bool {
        self.edited.as_ref().is_some_and(|e| self.pad.as_ref() != Some(e))
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
    /// Whether the read-slot note shows.
    pub read_note: bool,
    /// Set while opening the controller is denied: the permission screen shows.
    pub access: Option<Access>,
    /// Where the udev rule install stands.
    pub install: Install,
    /// Whether this connection has had its first read.
    read_since_connect: bool,
}

impl AppState {
    /// A window with no controller seen yet.
    #[must_use]
    pub const fn new(description: &'static ControllerDescription) -> Self {
        Self {
            description,
            current_mode: None,
            slots: BTreeMap::new(),
            selected: (0, 0),
            reading: false,
            read_error: None,
            read_note: false,
            access: None,
            install: Install::Idle,
            read_since_connect: false,
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
            self.read_since_connect = false;
            self.read_error = None;
            self.read_note = false;
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
        if !self.read_since_connect {
            self.read_since_connect = true;
            self.read_note = true;
        }
        if first_ever {
            let current = self.current_mode;
            if let Some(i) = self.description.modes.iter().position(|m| Some(m.id) == current) {
                self.selected = (i, 0);
            }
        }
    }

    /// A read was denied the controller. `rule_installed` says whether the udev rule
    /// is already in place.
    pub const fn read_denied(&mut self, rule_installed: bool) {
        self.reading = false;
        self.access = Some(if rule_installed { Access::StillDenied } else { Access::Denied });
    }

    /// The udev rule install was sent to the worker.
    pub fn install_started(&mut self) {
        self.install = Install::Running;
    }

    /// The udev rule install came back.
    pub fn install_finished(&mut self, result: Result<(), String>) {
        self.install = result.err().map_or(Install::Idle, Install::Failed);
    }

    /// The read-slot note was closed. It stays closed for this connection.
    pub const fn close_read_note(&mut self) {
        self.read_note = false;
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
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub mod tests {
    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::Pro3;
    use controller_core::model::{CanonicalProfileSummary, ProfileReadResult, RawProfilePayload};

    use super::*;

    pub fn description() -> &'static ControllerDescription {
        Pro3.description().unwrap()
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
        let s = AppState::new(description());
        assert!(!s.has_controller());
        assert_eq!(s.current_mode, None);
    }

    #[test]
    fn first_read_fills_every_slot_and_selects_the_current_mode() {
        let mut s = AppState::new(description());
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
    fn read_note_shows_once_per_connection() {
        let mut s = AppState::new(description());
        s.presence(Some(Mode::XInput));
        s.read_finished(Ok(full_read()));
        assert!(s.read_note);
        s.close_read_note();
        s.read_finished(Ok(full_read()));
        assert!(!s.read_note, "a second read on the same connection keeps it closed");
        s.presence(None);
        s.presence(Some(Mode::XInput));
        s.read_finished(Ok(full_read()));
        assert!(s.read_note, "a new connection shows it again");
    }

    #[test]
    fn unplug_keeps_slots_and_edits() {
        let mut s = AppState::new(description());
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
        let mut s = AppState::new(description());
        s.presence(Some(Mode::XInput));
        s.read_started();
        s.read_finished(Err("device disconnected".to_owned()));
        assert_eq!(s.read_error.as_deref(), Some("device disconnected"));
        assert!(!s.reading && !s.read_note && !s.has_controller());
        s.presence(Some(Mode::XInput));
        assert_eq!(s.read_error, None);
    }

    #[test]
    fn denied_read_asks_for_access_until_a_good_read() {
        let mut s = AppState::new(description());
        s.presence(Some(Mode::XInput));
        s.read_started();
        s.read_denied(false);
        assert_eq!(s.access, Some(Access::Denied));
        assert!(!s.reading);
        s.install_started();
        assert_eq!(s.install, Install::Running);
        s.install_finished(Err("The password prompt was closed.".to_owned()));
        assert!(matches!(s.install, Install::Failed(_)));
        s.install_started();
        assert_eq!(s.install, Install::Running, "a retry clears the old failure");
        s.install_finished(Ok(()));
        s.read_denied(true);
        assert_eq!(s.access, Some(Access::StillDenied));
        s.read_finished(Ok(full_read()));
        assert_eq!(s.access, None);
    }

    #[test]
    fn unplug_leaves_the_permission_screen() {
        let mut s = AppState::new(description());
        s.presence(Some(Mode::XInput));
        s.read_denied(true);
        s.presence(None);
        assert_eq!(s.access, None);
    }

    #[test]
    fn select_ignores_out_of_range() {
        let mut s = AppState::new(description());
        s.select(2, 2);
        assert_eq!(s.selected_slot(), Some((Mode::DInput, 3)));
        s.select(3, 0);
        s.select(0, 3);
        assert_eq!(s.selected, (2, 2));
    }
}
