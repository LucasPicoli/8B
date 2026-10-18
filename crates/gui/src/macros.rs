//! The macros of each slot: the descriptors a read gives, the steps read after it in
//! the background, and the edited copies. Pure: no Slint, no I/O.

use std::collections::BTreeMap;

use controller_core::model::glide::expanded_len;
use controller_core::model::{MacroDefinition, MacroStep, Mode};
use tr::tr;

use crate::buttons::target;
use crate::state::{AppState, SlotState};

/// A macro the controller holds: its descriptor, and its steps once read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadMacro {
    /// The macro. Until [`Self::loaded`], its steps are placeholders, one per step the
    /// descriptor counts.
    pub def: MacroDefinition,
    /// Whether the steps have been read.
    pub loaded: bool,
}

/// The start of the output id the picker gives a button that starts a macro. The rest
/// is the macro slot, 0-based.
const MACRO_OUTPUT: &str = "macro:";

/// The output id of the macro in macro slot `slot`.
#[must_use]
pub fn macro_output(slot: u8) -> String {
    format!("{MACRO_OUTPUT}{slot}")
}

/// The macro slot of output id `id`, when it names a macro.
#[must_use]
pub fn macro_number(id: &str) -> Option<u8> {
    id.strip_prefix(MACRO_OUTPUT)?.parse().ok()
}

/// `def` as 8B writes and saves it: a macro with no name is named "Macro n", n its
/// 1-based macro slot, as the controller needs a name.
#[must_use]
pub fn named(mut def: MacroDefinition) -> MacroDefinition {
    if def.name.is_empty() {
        def.name = tr!("Macro {}", def.macro_slot.map_or(1, |s| u16::from(s) + 1));
    }
    def
}

/// "Jump (Macro 2)", or "Macro 2" for a macro with no name or that name.
#[must_use]
pub fn title(def: &MacroDefinition, slot: u8) -> String {
    let n = u16::from(slot) + 1;
    let plain = tr!("Macro {}", n);
    if def.name.is_empty() || def.name == plain {
        plain
    } else {
        tr!("{name} (Macro {n})", name = def.name, n = n)
    }
}

/// The title of the macro in macro slot `slot` among `defs`, or "Macro n" when none
/// holds that slot.
#[must_use]
pub fn label(defs: &[&MacroDefinition], slot: u8) -> String {
    defs.iter()
        .find(|d| d.macro_slot == Some(slot))
        .map_or_else(|| tr!("Macro {}", u16::from(slot) + 1), |d| title(d, slot))
}

/// What a descriptor holds of `def`: `def` with each step a placeholder, one per step
/// the pad holds, so a glide counts as the short steps it expands to over
/// `glide_step_ms`. Two macros with equal descriptors read back the same until a step
/// read tells them apart.
#[must_use]
pub fn descriptor(def: &MacroDefinition, glide_step_ms: Option<u16>) -> MacroDefinition {
    let len = expanded_len(&def.steps, glide_step_ms);
    MacroDefinition { steps: vec![MacroStep::default(); len], ..def.clone() }
}

/// The macro slot of the first of `macros` that `button` starts.
fn started_by(macros: &[&MacroDefinition], button: &str) -> Option<u8> {
    macros.iter().find(|m| m.trigger_buttons.iter().any(|b| b == button)).and_then(|m| m.macro_slot)
}

/// The descriptors of `macros`, in order.
fn descriptors<'a>(
    macros: impl IntoIterator<Item = &'a MacroDefinition>,
    glide_step_ms: Option<u16>,
) -> Vec<MacroDefinition> {
    macros.into_iter().map(|m| descriptor(m, glide_step_ms)).collect()
}

impl SlotState {
    /// Whether the controller's macros now read as `new`, against the ones the edits
    /// of this slot began on, `old`: the macro edits then face a changed slot.
    fn macros_changed_under_edits(
        &self,
        old: &[PadMacro],
        new: &[MacroDefinition],
        glide_step_ms: Option<u16>,
    ) -> bool {
        let new = descriptors(new, glide_step_ms);
        self.macros_unsaved()
            && descriptors(old.iter().map(|m| &m.def), glide_step_ms) != new
            && self.edited_macros.as_ref().is_none_or(|e| descriptors(e, glide_step_ms) != new)
    }

    /// Takes the macros `fresh` the controller holds now. A macro whose descriptor did
    /// not change keeps its steps. Returns whether the change meets real macro edits;
    /// edits that only match the old macros are dropped, so they cannot undo it.
    fn take_macros(&mut self, fresh: Vec<MacroDefinition>, glide_step_ms: Option<u16>) -> bool {
        let changed = self.macros_changed_under_edits(&self.pad_macros, &fresh, glide_step_ms);
        if !self.macros_unsaved() {
            self.edited_macros = None;
        }
        let old = std::mem::take(&mut self.pad_macros);
        self.pad_macros = fresh
            .into_iter()
            .map(|def| {
                old.iter()
                    .find(|m| descriptor(&m.def, glide_step_ms) == def)
                    .cloned()
                    .unwrap_or(PadMacro { loaded: def.steps.is_empty(), def })
            })
            .collect();
        changed
    }

    /// The macros whose names and macro slots the slot shows: the edited ones, else the
    /// controller's.
    #[must_use]
    pub fn shown_macros(&self) -> Vec<&MacroDefinition> {
        self.edited_macros
            .as_ref()
            .map_or_else(|| self.held_macros(), |edited| edited.iter().collect())
    }

    /// The macros the controller holds in the slot.
    #[must_use]
    pub fn held_macros(&self) -> Vec<&MacroDefinition> {
        self.pad_macros.iter().map(|m| &m.def).collect()
    }

    /// The macro slot of the macro `button` starts in the slot as shown. The edited
    /// macros hold it when there are any; else the controller's, minus the buttons the
    /// edited profile took off.
    #[must_use]
    pub fn macro_started_by(&self, button: &str) -> Option<u8> {
        if self.edited_macros.is_none() && self.dropped_buttons().iter().any(|b| b == button) {
            return None;
        }
        started_by(&self.shown_macros(), button)
    }

    /// The macro slot of the macro `button` starts on the controller.
    #[must_use]
    pub fn held_macro_started_by(&self, button: &str) -> Option<u8> {
        started_by(&self.held_macros(), button)
    }

    /// Whether a button starts another macro than it does on the controller.
    pub(crate) fn starters_changed(&self) -> bool {
        let buttons = self.shown_macros().into_iter().chain(self.held_macros());
        buttons
            .flat_map(|m| &m.trigger_buttons)
            .any(|b| self.macro_started_by(b) != self.held_macro_started_by(b))
    }

    /// Whether the slot's macros can be edited now: it has a profile on screen, and
    /// the steps of its macros are read, as an edit copies them.
    #[must_use]
    pub fn macros_editable(&self) -> bool {
        self.shown().is_some() && self.macros_read()
    }

    /// Whether the steps of the slot's macros as shown are known: the macros are edited,
    /// or the controller's are read.
    #[must_use]
    pub fn macros_read(&self) -> bool {
        self.edited_macros.is_some() || self.pad_macros.iter().all(|m| m.loaded)
    }

    /// The slot's macros as shown, steps included: the edited ones, else the
    /// controller's minus the buttons the edited profile took off. `None` while their
    /// steps are being read.
    #[must_use]
    pub fn macros_as_shown(&self) -> Option<Vec<MacroDefinition>> {
        if let Some(edited) = &self.edited_macros {
            return Some(edited.clone());
        }
        if !self.macros_read() {
            return None;
        }
        let dropped = self.dropped_buttons();
        let mut macros: Vec<_> = self.pad_macros.iter().map(|m| m.def.clone()).collect();
        for m in &mut macros {
            m.trigger_buttons.retain(|b| !dropped.contains(b));
        }
        Some(macros)
    }

    /// Takes the edited macros of `from`, a slot of an unplugged controller, when it
    /// holds any. Returns whether this slot's macros changed under them.
    pub(crate) fn take_macro_edits(&mut self, from: &Self, glide_step_ms: Option<u16>) -> bool {
        if !from.macros_unsaved() {
            return false;
        }
        let fresh: Vec<_> = self.pad_macros.iter().map(|m| m.def.clone()).collect();
        let changed = from.macros_changed_under_edits(&from.pad_macros, &fresh, glide_step_ms);
        self.edited_macros.clone_from(&from.edited_macros);
        changed
    }
}

impl AppState {
    /// Records the macro descriptors a read of the controller on `port` found, by
    /// slot. An empty slot gets none. A slot whose macros changed under macro edits
    /// goes in the changed list, as a changed profile does.
    pub fn set_macros(
        &mut self,
        port: &str,
        mut found: BTreeMap<(Mode, u8), Vec<MacroDefinition>>,
    ) {
        let fallback = self.fallback_description;
        let Some(c) = self.controller_mut(port) else { return };
        let glide_step_ms = c.description.unwrap_or(fallback).limits.glide_step_ms;
        for (&key, slot) in &mut c.slots {
            let fresh = found.remove(&key).filter(|_| slot.pad.is_some()).unwrap_or_default();
            if slot.take_macros(fresh, glide_step_ms) {
                c.changed.entry(key).or_insert(true);
            }
        }
    }

    /// The macros of the controller on `port` whose steps are still to read, with
    /// their slot.
    #[must_use]
    pub fn unread_macros(&self, port: &str) -> Vec<((Mode, u8), MacroDefinition)> {
        let Some(c) = self.controllers.iter().find(|c| c.port == port) else { return Vec::new() };
        c.slots
            .iter()
            .flat_map(|(&key, slot)| {
                slot.pad_macros.iter().filter(|m| !m.loaded).map(move |m| (key, m.def.clone()))
            })
            .collect()
    }

    /// The steps of `read`, read from the controller on `port`, fill the macros they
    /// belong to. A macro whose descriptor changed since the read was asked for is
    /// left to the next one.
    pub fn steps_read(&mut self, port: &str, read: Vec<((Mode, u8), MacroDefinition)>) {
        let fallback = self.fallback_description;
        let Some(c) = self.controller_mut(port) else { return };
        let glide_step_ms = c.description.unwrap_or(fallback).limits.glide_step_ms;
        for (key, def) in read {
            let wanted = descriptor(&def, glide_step_ms);
            let Some(m) = c.slots.get_mut(&key).and_then(|s| {
                s.pad_macros
                    .iter_mut()
                    .find(|m| !m.loaded && descriptor(&m.def, glide_step_ms) == wanted)
            }) else {
                continue;
            };
            *m = PadMacro { def, loaded: true };
        }
    }

    /// Moves `button` into the macro in macro slot `macro_slot` of the selected slot, out
    /// of the one it started, and gives it the output it has in the mode's default
    /// profile, because a button that starts a macro keeps that output. Does nothing when
    /// the button cannot start a macro, no such macro is there, or the steps of the
    /// slot's macros are still being read.
    pub fn join_macro(&mut self, button: &str, macro_slot: u8) {
        let Some((mode, number)) = self.selected_slot() else { return };
        let slot = self.slot(mode, number);
        let startable = self.description().button(button).is_some_and(|b| b.can_start_macro);
        let held = slot.shown_macros().iter().any(|m| m.macro_slot == Some(macro_slot));
        if !(startable && held && slot.macros_editable()) {
            return;
        }
        let itself = self
            .defaults()
            .get(&mode)
            .map_or_else(|| button.to_owned(), |p| target(p, button).to_owned());
        let order: Vec<String> = self.description().buttons.iter().map(|b| b.id.clone()).collect();
        self.set_output(button, &itself);
        self.edit_macros(|macros| {
            for m in macros.iter_mut() {
                m.trigger_buttons.retain(|b| b != button);
                if m.macro_slot == Some(macro_slot) {
                    m.trigger_buttons.push(button.to_owned());
                    m.trigger_buttons.sort_by_key(|b| order.iter().position(|o| o == b));
                }
            }
        });
    }

    /// Takes `buttons` off every macro of the selected slot's edits: the profile's
    /// references, and the edited macros when there are any. The controller's macros
    /// stay as they are until the write.
    pub(crate) fn leave_macros(&mut self, buttons: &[String]) {
        self.edit(|p| {
            for m in &mut p.macro_refs {
                m.trigger_buttons.retain(|b| !buttons.contains(b));
            }
        });
        if let Some(edited) = self.selected_mut().and_then(|s| s.edited_macros.as_mut()) {
            for m in edited {
                m.trigger_buttons.retain(|b| !buttons.contains(b));
            }
        }
    }

    /// Applies `change` to the selected slot's edited macros. The first change copies
    /// the macros the controller holds, so it waits for their steps. An empty slot
    /// with no profile staged has no macros to change.
    pub fn edit_macros(&mut self, change: impl FnOnce(&mut Vec<MacroDefinition>)) {
        let Some(slot) = self.selected_mut() else { return };
        if slot.shown().is_none() {
            return;
        }
        let Some(mut macros) = slot.macros_as_shown() else { return };
        change(&mut macros);
        slot.edited_macros = Some(macros);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::XINPUT;
    use controller_core::model::{GlideControl, MacroStep, REPEAT_CONTINUOUS};

    use super::*;
    use crate::state::tests::{connected, full_read, PORT};

    /// The Pro 3's `glide_step_ms`.
    const GLIDE_MS: Option<u16> = Some(20);
    use crate::state::Dirty;

    /// A macro in macro slot `slot` named `name`, with `steps` steps of 100 ms each.
    fn macro_def(slot: u8, name: &str, steps: usize) -> MacroDefinition {
        MacroDefinition {
            name: name.to_owned(),
            mode: XINPUT,
            trigger_buttons: vec!["p1".to_owned()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps: vec![MacroStep { duration_ms: 100, ..MacroStep::default() }; steps],
            macro_slot: Some(slot),
        }
    }

    /// What a read finds: slot 2 of `XInput` holds `macros`, and slot 3, empty, keeps
    /// a leftover.
    fn found(macros: &[MacroDefinition]) -> BTreeMap<(Mode, u8), Vec<MacroDefinition>> {
        BTreeMap::from([
            ((XINPUT, 2), descriptors(macros, GLIDE_MS)),
            ((XINPUT, 3), vec![descriptor(&macro_def(0, "Old", 2), GLIDE_MS)]),
        ])
    }

    /// Connected in `XInput`, with [`found`] read and every step read, slot 2 selected.
    fn loaded(macros: &[MacroDefinition]) -> AppState {
        let mut s = connected(XINPUT);
        s.set_macros(PORT, found(macros));
        s.steps_read(PORT, macros.iter().map(|m| ((XINPUT, 2), m.clone())).collect());
        s.select(0, 1);
        s
    }

    #[test]
    fn a_read_gives_each_slot_its_summaries_and_an_empty_slot_none() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = connected(XINPUT);
        s.set_macros(PORT, found(std::slice::from_ref(&jump)));
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.pad_macros.len(), 1);
        assert_eq!(
            (slot.pad_macros[0].def.name.as_str(), slot.pad_macros[0].loaded),
            ("Jump", false)
        );
        assert_eq!(slot.pad_macros[0].def.steps.len(), 3);
        assert_eq!(s.slot(XINPUT, 3).pad_macros, [], "an empty slot shows no macros");
        assert_eq!(s.unread_macros(PORT), [((XINPUT, 2), descriptor(&jump, GLIDE_MS))]);
    }

    #[test]
    fn read_steps_fill_their_macro_and_survive_a_read_with_the_same_descriptor() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = loaded(std::slice::from_ref(&jump));
        assert_eq!(s.slot(XINPUT, 2).pad_macros, [PadMacro { def: jump.clone(), loaded: true }]);
        assert_eq!(s.unread_macros(PORT), []);
        // The next read finds the same descriptor: the steps stay, nothing to read.
        s.set_macros(PORT, found(std::slice::from_ref(&jump)));
        assert!(s.slot(XINPUT, 2).pad_macros[0].loaded);
        assert_eq!(s.unread_macros(PORT), []);
        // A changed descriptor reads again.
        let longer = macro_def(1, "Jump", 4);
        s.set_macros(PORT, found(std::slice::from_ref(&longer)));
        assert_eq!(s.unread_macros(PORT), [((XINPUT, 2), descriptor(&longer, GLIDE_MS))]);
        // Steps for the old descriptor, late, fill nothing.
        s.steps_read(PORT, vec![((XINPUT, 2), jump)]);
        assert!(!s.slot(XINPUT, 2).pad_macros[0].loaded);
    }

    #[test]
    fn a_glide_macro_loads_from_its_expanded_descriptor_and_stays_loaded() {
        // The pad holds the glide as 5 short steps; the steps read folds them into one.
        let mut glide = macro_def(1, "Glide", 2);
        glide.steps[0].left_stick_x = 255;
        glide.steps[0].glide = [GlideControl::LeftStick].into();
        let on_pad = controller_core::model::glide::expand(&glide, GLIDE_MS);
        assert_eq!(on_pad.steps.len(), 6);
        let mut s = connected(XINPUT);
        let found = BTreeMap::from([((XINPUT, 2), vec![descriptor(&on_pad, GLIDE_MS)])]);
        s.set_macros(PORT, found.clone());
        s.steps_read(PORT, vec![((XINPUT, 2), glide.clone())]);
        assert_eq!(s.slot(XINPUT, 2).pad_macros, [PadMacro { def: glide.clone(), loaded: true }]);
        s.set_macros(PORT, found);
        assert!(s.slot(XINPUT, 2).pad_macros[0].loaded, "a read of the same macro keeps its steps");
        assert_eq!(s.unread_macros(PORT), []);
    }

    #[test]
    fn a_macro_with_no_steps_needs_no_read() {
        let mut s = connected(XINPUT);
        s.set_macros(PORT, found(&[macro_def(0, "Empty", 0)]));
        assert!(s.slot(XINPUT, 2).pad_macros[0].loaded);
        assert_eq!(s.unread_macros(PORT), []);
    }

    #[test]
    fn a_macro_edit_waits_for_the_steps_then_marks_the_slot() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = connected(XINPUT);
        s.set_macros(PORT, found(std::slice::from_ref(&jump)));
        s.select(0, 1);
        s.edit_macros(|m| m[0].repeat_count = REPEAT_CONTINUOUS);
        assert_eq!(s.slot(XINPUT, 2).edited_macros, None, "the steps are not read yet");
        s.steps_read(PORT, vec![((XINPUT, 2), jump)]);
        s.edit_macros(|m| m[0].repeat_count = REPEAT_CONTINUOUS);
        let slot = s.slot(XINPUT, 2);
        assert!(slot.unsaved() && slot.macros_unsaved() && !slot.profile_unsaved());
        assert_eq!(s.slot_dirty(XINPUT, 2), Dirty { macros: true, ..Dirty::default() });
        assert!(s.active().unwrap().has_edits());
        // Changed back: nothing unsaved.
        s.edit_macros(|m| m[0].repeat_count = 1);
        assert!(!s.slot(XINPUT, 2).unsaved());
    }

    #[test]
    fn an_empty_slot_takes_macros_only_with_a_staged_profile() {
        let mut s = connected(XINPUT);
        s.set_macros(PORT, found(&[]));
        s.select(0, 2);
        s.edit_macros(|m| m.push(macro_def(0, "New", 1)));
        assert_eq!(s.slot(XINPUT, 3).edited_macros, None);
        s.start_from_default();
        s.edit_macros(|m| m.push(macro_def(0, "New", 1)));
        assert_eq!(s.slot(XINPUT, 3).edited_macros.unwrap().len(), 1);
    }

    #[test]
    fn discard_drops_the_macro_edits() {
        let mut s = loaded(&[macro_def(1, "Jump", 3)]);
        s.edit_macros(|m| m[0].name = "Dash".to_owned());
        s.discard();
        assert_eq!(s.slot(XINPUT, 2).edited_macros, None);
    }

    #[test]
    fn a_read_that_changes_macros_under_edits_asks_and_one_that_matches_them_does_not() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = loaded(std::slice::from_ref(&jump));
        s.edit_macros(|m| m[0].name = "Dash".to_owned());
        // Another program wrote the edit itself: no question, the edits are done.
        let dash = MacroDefinition { name: "Dash".to_owned(), ..jump };
        s.set_macros(PORT, found(std::slice::from_ref(&dash)));
        assert_eq!(s.changed_slots(), []);
        s.steps_read(PORT, vec![((XINPUT, 2), dash)]);
        assert!(!s.slot(XINPUT, 2).unsaved(), "equal once the steps are read");
        // Edits again, then another change on the pad: the question shows.
        s.edit_macros(|m| m[0].name = "Run".to_owned());
        s.set_macros(PORT, found(&[macro_def(1, "Walk", 3)]));
        assert_eq!(s.changed_slots(), [(XINPUT, 2)]);
        // Taking the controller's version drops the macro edits.
        s.choose_changed((XINPUT, 2), false);
        s.apply_changed();
        assert_eq!(s.slot(XINPUT, 2).edited_macros, None);
    }

    #[test]
    fn edits_that_equal_the_pad_go_when_the_pad_changes() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = loaded(std::slice::from_ref(&jump));
        s.edit_macros(|m| m[0].name = "Jump".to_owned());
        s.set_macros(PORT, found(&[macro_def(1, "Walk", 3)]));
        assert_eq!(s.changed_slots(), []);
        assert_eq!(s.slot(XINPUT, 2).edited_macros, None);
    }

    #[test]
    fn a_profile_change_does_not_ask_about_a_slot_with_only_macro_edits() {
        let mut s = loaded(&[macro_def(1, "Jump", 3)]);
        s.edit_macros(|m| m[0].name = "Dash".to_owned());
        let mut read = full_read();
        let i = read.profiles.iter().position(|p| p.mode == XINPUT && p.source_slot == 2).unwrap();
        read.profiles[i].canonical.name = "Rally".to_owned();
        s.read_finished(PORT, Ok(read));
        assert_eq!(s.changed_slots(), []);
        assert!(s.slot(XINPUT, 2).macros_unsaved());
    }

    #[test]
    fn a_moved_controller_brings_its_macro_edits() {
        let jump = macro_def(1, "Jump", 3);
        let mut s = loaded(std::slice::from_ref(&jump));
        s.edit_macros(|m| m[0].name = "Dash".to_owned());
        s.presence(PORT, None);
        s.presence("9-1", Some(XINPUT));
        s.read_finished("9-1", Ok(full_read()));
        s.set_macros("9-1", found(&[macro_def(1, "Walk", 3)]));
        let from = s.move_candidates()[0];
        s.answer_move(Some(from));
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.edited_macros.unwrap()[0].name, "Dash");
        assert_eq!(s.changed_slots(), [(XINPUT, 2)], "the new pad holds another macro");
    }
}
