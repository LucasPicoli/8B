//! The macros a slot's write changes: the macro edits its job carries, and one review
//! line for each macro the write adds, changes or removes.

use controller_core::model::{MacroDefinition, MacroSlot, Mode, REPEAT_CONTINUOUS};
use controller_core::orchestrator::MacroEdit;
use slint::SharedString;
use tr::tr;

use crate::buttons::button_label;
use crate::macros::{named, title};
use crate::render::and_list;
use crate::state::{AppState, SlotState};

/// One macro slot of a slot, before and after the write.
struct MacroChange {
    /// The macro slot, 0-based.
    slot: u8,
    /// The macro the controller holds there.
    old: Option<MacroDefinition>,
    /// The macro the write leaves there.
    new: Option<MacroDefinition>,
    /// The macro edits change it, so the job carries it. A macro that only loses a
    /// button to a remap is not edited: the job's `drop_macros` takes the button off.
    edited: bool,
}

impl MacroChange {
    /// Whether the write replaces or removes a macro the controller holds.
    fn touches_held(&self) -> bool {
        self.old.is_some() && self.new != self.old
    }
}

impl SlotState {
    /// The buttons that leave their macro because the edited profile gives them another
    /// output. A button the edited macros start is not one of them.
    pub(crate) fn dropped_buttons(&self) -> Vec<String> {
        let Some(edited) = &self.edited else { return Vec::new() };
        let kept: Vec<&String> = edited
            .macro_refs
            .iter()
            .flat_map(|m| &m.trigger_buttons)
            .chain(self.edited_macros.iter().flatten().flat_map(|m| &m.trigger_buttons))
            .collect();
        self.pad
            .iter()
            .flat_map(|p| &p.macro_refs)
            .flat_map(|m| &m.trigger_buttons)
            .filter(|b| !kept.contains(b))
            .cloned()
            .collect()
    }

    /// Every macro slot the controller or the macro edits hold a macro in, in order. A
    /// macro with no macro slot is left out: the Macros tab gives each macro one.
    /// `names`: the controller's macros have names.
    fn macro_changes(&self, names: bool) -> Vec<MacroChange> {
        let dropped = self.dropped_buttons();
        let held = self.pad_macros.iter().map(|m| &m.def);
        let mut slots: Vec<u8> = held
            .clone()
            .chain(self.edited_macros.iter().flatten())
            .filter_map(|d| d.macro_slot)
            .collect();
        slots.sort_unstable();
        slots.dedup();
        slots
            .into_iter()
            .map(|slot| {
                let old = held.clone().find(|d| d.macro_slot == Some(slot)).cloned();
                let mut new = self.edited_macros.as_ref().map_or_else(
                    || old.clone(),
                    |edited| {
                        edited
                            .iter()
                            .find(|d| d.macro_slot == Some(slot))
                            .cloned()
                            .map(|d| named(d, names))
                    },
                );
                let edited = new != old;
                if let Some(new) = &mut new {
                    new.trigger_buttons.retain(|b| !dropped.contains(b));
                }
                MacroChange { slot, old, new, edited }
            })
            .collect()
    }

    /// The macro edits the slot's write job carries: a put for each macro the edits add
    /// or change, a delete for each they remove. `None` for a macro slot out of range.
    pub(super) fn macro_edits(&self, names: bool) -> Option<Vec<MacroEdit>> {
        self.macro_changes(names)
            .into_iter()
            .filter(|c| c.edited)
            .map(|c| {
                let macro_slot = MacroSlot::new(c.slot).ok()?;
                Some(c.new.map_or(MacroEdit::Delete { macro_slot }, |definition| MacroEdit::Put {
                    macro_slot,
                    definition,
                }))
            })
            .collect()
    }

    /// The macros the slot's write job puts, each as the job carries it.
    pub(crate) fn macro_puts(&self, names: bool) -> Vec<MacroDefinition> {
        self.macro_changes(names).into_iter().filter(|c| c.edited).filter_map(|c| c.new).collect()
    }

    /// How many macros the controller holds that the write leaves in their macro slot,
    /// unedited. A macro that loses a button to a remap stays.
    pub(super) fn macros_kept(&self, names: bool) -> usize {
        self.macro_changes(names)
            .iter()
            .filter(|c| c.old.is_some() && c.new.is_some() && !c.edited)
            .count()
    }

    /// How many macros the macro edits add, change or remove.
    pub(super) fn macros_edited(&self, names: bool) -> usize {
        self.macro_changes(names).iter().filter(|c| c.edited).count()
    }

    /// Whether the write replaces or removes a macro the controller holds.
    pub(crate) fn touches_held_macro(&self, names: bool) -> bool {
        self.macro_changes(names).iter().any(MacroChange::touches_held)
    }
}

/// How often a macro plays, as the Macros tab says it.
fn plays(repeat_count: u32) -> String {
    match repeat_count {
        1 => tr!("once"),
        REPEAT_CONTINUOUS => tr!("until pressed again"),
        n => tr!("{n} time" | "{n} times" % n),
    }
}

impl AppState {
    /// The buttons that start `def`, as `mode` labels them, or "No button".
    fn starts_with(&self, mode: Mode, def: &MacroDefinition) -> String {
        if def.trigger_buttons.is_empty() {
            return tr!("No button");
        }
        let labels: Vec<_> =
            def.trigger_buttons.iter().map(|b| button_label(self.description(), mode, b)).collect();
        and_list(&labels)
    }

    /// The review line of `c`: the macro, then a label and value pair for each thing
    /// that changed, such as "Jump (Macro 2) changed. Steps: 12 → 15.". `None` for a
    /// macro the write leaves as it is.
    fn macro_line(&self, mode: Mode, c: &MacroChange) -> Option<String> {
        let mut parts = Vec::new();
        match (&c.old, &c.new) {
            (None, Some(new)) => {
                parts.push(tr!("{macro} added.", macro = title(new, c.slot)));
                parts.push(tr!("Steps: {n}.", n = new.steps.len()));
                parts.push(tr!("Starts with: {buttons}.", buttons = self.starts_with(mode, new)));
            }
            (Some(old), None) => parts.push(tr!("{macro} removed.", macro = title(old, c.slot))),
            (Some(old), Some(new)) if old != new => {
                parts.push(tr!("{macro} changed.", macro = title(old, c.slot)));
                if old.name != new.name {
                    parts.push(tr!("Name: “{name}”.", name = new.name));
                }
                if old.steps.len() != new.steps.len() {
                    parts.push(tr!(
                        "Steps: {old} → {new}.",
                        old = old.steps.len(),
                        new = new.steps.len()
                    ));
                } else if old.steps != new.steps {
                    parts.push(tr!("Steps: edited."));
                }
                if old.repeat_count != new.repeat_count {
                    parts.push(tr!(
                        "Plays: {old} → {new}.",
                        old = plays(old.repeat_count),
                        new = plays(new.repeat_count)
                    ));
                }
                if old.interval_ms != new.interval_ms {
                    parts.push(tr!(
                        "Interval: {old} → {new} ms.",
                        old = old.interval_ms,
                        new = new.interval_ms
                    ));
                }
                if old.trigger_buttons != new.trigger_buttons {
                    parts.push(tr!(
                        "Starts with: {old} → {new}.",
                        old = self.starts_with(mode, old),
                        new = self.starts_with(mode, new)
                    ));
                }
            }
            _ => return None,
        }
        Some(parts.join(" "))
    }

    /// One sentence per macro of `slot` that an import adds, replaces or removes, in
    /// macro slot order, such as "Ramp replaces Dash in Macro 1.".
    pub(crate) fn import_lines(&self, slot: (Mode, u8)) -> Vec<String> {
        // A macro with no name, "", gets whole sentences of its own, for the article.
        let name = |d: &Option<MacroDefinition>| d.as_ref().map(|d| d.name.clone());
        self.slot(slot.0, slot.1)
            .macro_changes(self.macro_names())
            .iter()
            .filter(|c| c.edited)
            .filter_map(|c| {
                let n = u16::from(c.slot) + 1;
                Some(match (name(&c.old).as_deref(), name(&c.new).as_deref()) {
                    (None, None) => return None,
                    (None, Some("")) => tr!("A macro without a name is added in Macro {n}.", n = n),
                    (None, Some(new)) => tr!("{name} is added in Macro {n}.", name = new, n = n),
                    (Some(""), None) => {
                        tr!("A macro without a name is removed from Macro {n}.", n = n)
                    }
                    (Some(old), None) => {
                        tr!("{name} is removed from Macro {n}.", name = old, n = n)
                    }
                    (Some(""), Some("")) => {
                        tr!("The file’s macro without a name replaces the one in Macro {n}.", n = n)
                    }
                    (Some(old), Some(new)) if old == new => {
                        tr!("The file’s {name} replaces the one in Macro {n}.", name = new, n = n)
                    }
                    (Some(old), Some("")) => {
                        tr!("A macro without a name replaces {old} in Macro {n}.", old = old, n = n)
                    }
                    (Some(""), Some(new)) => {
                        tr!("{new} replaces a macro without a name in Macro {n}.", new = new, n = n)
                    }
                    (Some(old), Some(new)) => {
                        tr!("{new} replaces {old} in Macro {n}.", new = new, old = old, n = n)
                    }
                })
            })
            .collect()
    }

    /// One line per macro of `slot` that the write adds, changes or removes, in macro
    /// slot order. A button that joins or leaves a macro reads under that macro.
    pub(super) fn macro_lines(&self, slot: (Mode, u8)) -> Vec<SharedString> {
        self.slot(slot.0, slot.1)
            .macro_changes(self.macro_names())
            .iter()
            .filter_map(|c| self.macro_line(slot.0, c))
            .map(SharedString::from)
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    use controller_core::devices::pro3::XINPUT;
    use controller_core::model::{MacroRef, MacroStep, WriteResult};
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def, PORT};
    use crate::writes::WriteOp;

    /// `XInput` slot 2 selected, holding "Jump" (macro slot 0, 12 steps, on PR and PL)
    /// and "Dash" (macro slot 1, 3 steps).
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp", "lp"], 12), macro_def(1, "Dash", &["r4"], 3)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        s.select(0, 1);
        s
    }

    fn lines(s: &AppState) -> Vec<String> {
        s.review_info((XINPUT, 2)).macro_changes.iter().map(|l| l.to_string()).collect()
    }

    fn edits(s: &AppState) -> Vec<MacroEdit> {
        let WriteOp::Upload { macro_edits, .. } = s.upload_job((XINPUT, 2)).unwrap().op else {
            panic!("an upload")
        };
        macro_edits
    }

    #[test]
    fn a_slot_with_only_macro_edits_writes_them_and_keeps_its_profile() {
        let mut s = held();
        s.edit_macros(|m| m[1].name = "Run".to_owned());
        assert_eq!(s.edited_slots(), [(XINPUT, 2)]);
        assert!(s.can_write());
        let job = s.upload_job((XINPUT, 2)).unwrap();
        let WriteOp::Upload { profile, drop_macros, macro_edits } = job.op else {
            panic!("an upload")
        };
        assert_eq!(profile, None, "the profile stays as the controller holds it");
        assert_eq!(drop_macros, Vec::<String>::new());
        let definition = macro_def(1, "Run", &["r4"], 3);
        assert_eq!(
            macro_edits,
            [MacroEdit::Put { macro_slot: MacroSlot::new(1).unwrap(), definition }]
        );
        let info = s.review_info((XINPUT, 2));
        assert_eq!((info.total, info.keeps_profile, info.macros_stay), (1, true, 1));
        assert_eq!(info.name, "Racing");
    }

    #[test]
    fn the_review_names_each_added_changed_and_removed_macro() {
        let mut s = held();
        s.edit_macros(|m| {
            m[0].steps.extend(vec![MacroStep::default(); 3]);
            m[0].trigger_buttons = vec!["lp".to_owned()];
            m.remove(1);
            m.push(macro_def(2, "Spin", &["rp"], 1));
            m.push(macro_def(3, "", &[], 2));
        });
        assert_eq!(
            lines(&s),
            [
                "Jump (Macro 1) changed. Steps: 12 → 15. Starts with: PR and PL → PL.",
                "Dash (Macro 2) removed.",
                "Spin (Macro 3) added. Steps: 1. Starts with: PR.",
                "Macro 4 added. Steps: 2. Starts with: No button.",
            ]
        );
        let macro_slot = |n| MacroSlot::new(n).unwrap();
        let kinds: Vec<_> = edits(&s)
            .iter()
            .map(|e| (e.macro_slot(), matches!(e, MacroEdit::Put { .. })))
            .collect();
        assert_eq!(
            kinds,
            [
                (macro_slot(0), true),
                (macro_slot(1), false),
                (macro_slot(2), true),
                (macro_slot(3), true)
            ]
        );
        let info = s.review_info((XINPUT, 2));
        assert_eq!((info.total, info.macros_stay), (4, 0));
    }

    #[test]
    fn a_change_lists_only_what_changed() {
        let mut s = held();
        s.edit_macros(|m| {
            m[0].steps[4].duration_ms = 300;
            m[1].name = String::new();
            m[1].repeat_count = REPEAT_CONTINUOUS;
            m[1].interval_ms = 200;
        });
        assert_eq!(
            lines(&s),
            [
                "Jump (Macro 1) changed. Steps: edited.",
                "Dash (Macro 2) changed. Name: “Macro 2”. Plays: once → until pressed again. \
                 Interval: 0 → 200 ms.",
            ]
        );
    }

    #[test]
    fn a_remap_reaches_an_edited_macro_and_reads_under_it() {
        let mut s = held();
        let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap();
        pad.pad.as_mut().unwrap().macro_refs = vec![MacroRef {
            trigger_buttons: vec!["rp".to_owned(), "lp".to_owned()],
            path: "xinput-slot2-macro0-Jump.json".to_owned(),
        }];
        s.set_output("rp", "right face");
        // Remapped only: the job drops the button, and the macro is not rewritten.
        assert_eq!(lines(&s), ["Jump (Macro 1) changed. Starts with: PR and PL → PL."]);
        assert_eq!(edits(&s), []);
        // Then edited: the put leaves the remapped button out.
        s.edit_macros(|m| m[0].name = "Hop".to_owned());
        let Ok::<[_; 1], _>([MacroEdit::Put { definition, .. }]) = edits(&s).try_into() else {
            panic!("one put")
        };
        assert_eq!(definition.trigger_buttons, ["lp"]);
        assert_eq!(
            lines(&s),
            ["Jump (Macro 1) changed. Name: “Hop”. Starts with: PR and PL → PL."]
        );
    }

    #[test]
    fn an_empty_slot_writes_its_staged_profile_and_macros_in_one_job() {
        let mut s = connected(XINPUT);
        s.select(0, 2);
        s.start_from_default();
        s.edit_macros(|m| m.push(macro_def(0, "New", &["r4"], 2)));
        let WriteOp::Upload { profile, macro_edits, .. } = s.upload_job((XINPUT, 3)).unwrap().op
        else {
            panic!("an upload")
        };
        assert!(profile.is_some());
        assert_eq!(macro_edits.len(), 1);
        let info = s.review_info((XINPUT, 3));
        assert!(info.is_new && !info.keeps_profile);
        assert_eq!(info.macro_changes.row_count(), 1);
    }

    #[test]
    fn a_good_write_drops_the_macro_edits_and_keeps_the_profile() {
        let mut s = held();
        s.edit_macros(|m| m[1].name = "Run".to_owned());
        s.begin_review().unwrap();
        s.review_read(PORT, true);
        let (_, jobs) = s.confirm_review().unwrap();
        s.write_finished(PORT, &[WriteResult::success(XINPUT, jobs[0].slot, "done")], None);
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.edited_macros, None);
        assert_eq!(slot.pad.unwrap().name, "Racing", "a macro-only job leaves the profile");
        // Until the next read, the slot shows what was written as a descriptor, its
        // steps to read again: as many as the controller holds.
        let run = &slot.pad_macros[1];
        assert_eq!((run.def.name.as_str(), run.loaded), ("Run", false));
        assert!(run.def.steps.iter().all(|st| *st == MacroStep::default()));
    }

    #[test]
    fn a_failure_names_each_saved_macro() {
        let mut s = held();
        s.edit_macros(|m| m[1].name = "Run".to_owned());
        s.begin_review().unwrap();
        s.review_read(PORT, true);
        let (_, jobs) = s.confirm_review().unwrap();
        let mut bad = WriteResult::failure(
            XINPUT,
            jobs[0].slot,
            controller_core::ErrorCategory::WriteFailure,
            "Write failed.",
        );
        bad.rollback_attempted = true;
        bad.macro_backup_paths = vec!["/b/dash.json".to_owned()];
        s.write_finished(PORT, &[bad], None);
        assert_eq!(s.write.failed.as_ref().unwrap().macro_backups, ["/b/dash.json"]);
        assert!(s.slot(XINPUT, 2).macros_unsaved(), "the edits stay");
    }

    #[test]
    fn a_batch_row_starts_open_for_a_replaced_macro_not_an_added_one() {
        let mut s = held();
        s.select(0, 0);
        s.set_name("One");
        s.select(0, 1);
        s.edit_macros(|m| m.push(macro_def(2, "Spin", &[], 1)));
        let open = |s: &mut AppState| {
            s.begin_batch().unwrap();
            s.review_read(PORT, true);
            let rows: Vec<_> = s.batch_info().slots.iter().map(|r| r.open).collect();
            s.end_batch();
            rows
        };
        assert_eq!(open(&mut s), [true, false], "an added macro replaces nothing");
        s.edit_macros(|m| m[0].name = "Hop".to_owned());
        assert_eq!(open(&mut s), [true, true]);
    }

    #[test]
    fn the_macros_section_starts_open_when_it_is_first_or_the_write_destroys_a_macro() {
        let mut s = held();
        let open = |s: &AppState| {
            let info = s.review_info((XINPUT, 2));
            (info.profile_open, info.macros_open)
        };
        s.edit_macros(|m| m.push(macro_def(2, "Spin", &[], 1)));
        assert!(open(&s).1, "with no profile changes the macros come first");
        s.set_name("Fast");
        assert_eq!(open(&s), (true, false), "an added macro destroys nothing");
        s.toggle_review_section(1);
        s.toggle_review_section(0);
        assert_eq!(open(&s), (false, true));
        s.toggle_review_section(1);
        s.edit_macros(|m| {
            m.remove(1);
        });
        assert!(open(&s).1, "removing Dash opens it");
        s.begin_review().unwrap();
        assert_eq!(open(&s), (true, true), "a new review starts over");
    }

    #[test]
    fn the_clear_question_counts_every_macro_the_pad_holds() {
        let mut s = held();
        s.begin_clear();
        assert_eq!(s.clear_info((XINPUT, 2)).macros, 2);
    }
}
