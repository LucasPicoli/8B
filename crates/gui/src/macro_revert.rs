//! "Revert" in the macro header, and "Undo" on a deleted macro: put one macro of the
//! selected slot back as the controller holds it, with the buttons that start it, and
//! leave the slot's other edits.

use controller_core::model::CanonicalProfile;
use tr::tr;

use crate::buttons::output_label;
use crate::macros::label;
use crate::state::{AppState, Notice};

/// Gives `button` in `profile` the output and the macro references it has in `pad`.
fn restore_button(profile: &mut CanonicalProfile, pad: &CanonicalProfile, button: &str) {
    match pad.button_mappings.iter().find(|m| m.source == button) {
        Some(held) => match profile.button_mappings.iter_mut().find(|m| m.source == button) {
            Some(m) => m.clone_from(held),
            None => profile.button_mappings.push(held.clone()),
        },
        None => profile.button_mappings.retain(|m| m.source != button),
    }
    // An edit only takes buttons off a reference, so the references keep their order.
    for (held, edited) in pad.macro_refs.iter().zip(&mut profile.macro_refs) {
        if held.trigger_buttons.iter().any(|b| b == button) {
            let kept = edited.trigger_buttons.clone();
            edited.trigger_buttons = held
                .trigger_buttons
                .iter()
                .filter(|b| *b == button || kept.contains(b))
                .cloned()
                .collect();
        }
    }
}

impl AppState {
    /// Why the macro in macro slot `n` of the selected slot cannot go back to the
    /// controller's copy: a button that copy starts now starts another macro or has
    /// another output, and reverting would undo that edit too.
    fn revert_refusal(&self, n: u8) -> Option<String> {
        let (mode, number) = self.selected_slot()?;
        let slot = self.slot(mode, number);
        let held = slot.held_macros().into_iter().find(|m| m.macro_slot == Some(n))?;
        held.trigger_buttons.iter().find_map(|b| {
            let name = output_label(self.description(), mode, b);
            let clash = if let Some(other) = slot.macro_started_by(b).filter(|&o| o != n) {
                let other = label(&slot.shown_macros(), other);
                tr!("{button} now starts {macro}.", button = name, macro = other)
            } else {
                let output = self.mapped_to(b)?;
                tr!("{button} is now mapped to {output}.", button = name, output = output)
            };
            Some(format!("{clash} {}", tr!("Undo that first, or discard the slot’s edits.")))
        })
    }

    /// Puts the macro in macro slot `n` of the selected slot back as the controller
    /// holds it, or takes off a staged new macro. The buttons it gave up start it
    /// again; the buttons it took go back to the macro they started, with the output
    /// the controller gives them. The slot's other edits stay. A button that would
    /// clash with another edit refuses the revert with a notice naming it.
    pub fn revert_macro(&mut self, n: u8) {
        let Some((mode, number)) = self.selected_slot() else { return };
        let slot = self.slot(mode, number);
        if !slot.macro_unsaved(n) {
            return;
        }
        if let Some(body) = self.revert_refusal(n) {
            let title = label(&slot.shown_macros(), n);
            self.notice = Some(Notice {
                error: true,
                title: tr!("Could not revert {macro}.", macro = title),
                body,
            });
            return;
        }
        let held = slot.held_macros().into_iter().find(|m| m.macro_slot == Some(n)).cloned();
        let held_buttons = held.as_ref().map(|m| m.trigger_buttons.clone()).unwrap_or_default();
        let taken: Vec<String> = slot
            .shown_macro(n)
            .into_iter()
            .flat_map(|m| &m.trigger_buttons)
            .filter(|b| !held_buttons.contains(b))
            .cloned()
            .collect();
        let homes: Vec<(String, u8)> = taken
            .iter()
            .filter_map(|b| slot.held_macro_started_by(b).map(|home| (b.clone(), home)))
            .collect();
        let order: Vec<&str> = self.description().buttons.iter().map(|b| b.id.as_str()).collect();
        self.edit_macros(|macros| {
            macros.retain(|m| m.macro_slot != Some(n));
            for (b, home) in &homes {
                if let Some(m) = macros.iter_mut().find(|m| m.macro_slot == Some(*home)) {
                    if !m.trigger_buttons.contains(b) {
                        m.trigger_buttons.push(b.clone());
                        m.trigger_buttons.sort_by_key(|b| order.iter().position(|o| o == b));
                    }
                }
            }
            macros.extend(held);
            macros.sort_by_key(|m| m.macro_slot);
        });
        if let (Some(pad), Some(_)) = (&slot.pad, &slot.edited) {
            self.edit(|p| {
                for b in held_buttons.iter().chain(&taken) {
                    restore_button(p, pad, b);
                }
            });
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::XINPUT;
    use controller_core::model::ButtonMapping;

    use crate::macros::macro_output;
    use crate::state::tests::{connected, hold_macros, macro_def, PORT};
    use crate::state::{AppState, Dirty};

    /// `XInput` slot 2 selected, holding "Jump" in macro slot 0 on PR and PL, and "Dash"
    /// in macro slot 2 on R4.
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp", "lp"], 12), macro_def(2, "Dash", &["r4"], 1)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        s.select(0, 1);
        s
    }

    fn clean(s: &AppState) -> bool {
        !s.slot(XINPUT, 2).unsaved() && s.slot_dirty(XINPUT, 2) == Dirty::default()
    }

    #[test]
    fn a_changed_macro_goes_back_and_the_other_edits_stay() {
        let mut s = held();
        s.rename_macro(0, "Run");
        s.edit_macro(0, |m| m.steps.truncate(3));
        s.rename_macro(2, "Sprint");
        s.revert_macro(0);
        let slot = s.slot(XINPUT, 2);
        assert!(!slot.macro_unsaved(0) && slot.macro_unsaved(2));
        assert_eq!(slot.shown_macro(0).unwrap().steps.len(), 12);
        s.revert_macro(2);
        assert!(clean(&s), "nothing else differs: the dots clear");
        assert_eq!(s.notice, None);
    }

    #[test]
    fn a_staged_new_macro_goes() {
        let mut s = held();
        s.new_macro(1);
        s.revert_macro(1);
        assert_eq!(s.slot(XINPUT, 2).shown_macro(1), None);
        assert!(clean(&s));
    }

    #[test]
    fn a_staged_delete_comes_back_with_its_buttons() {
        let mut s = held();
        s.delete_macro(0);
        s.revert_macro(0);
        assert_eq!(s.slot(XINPUT, 2).macro_started_by("rp"), Some(0));
        assert!(clean(&s));
    }

    #[test]
    fn a_button_it_took_goes_back_to_its_macro() {
        let mut s = held();
        s.add_macro_button(0, "r4");
        s.revert_macro(0);
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.macro_started_by("r4"), Some(2));
        assert!(clean(&s));
    }

    #[test]
    fn a_button_it_took_gets_its_output_back() {
        let mut s = held();
        let slot = s.controller_mut(PORT).unwrap().slots.get_mut(&(XINPUT, 2)).unwrap();
        let mappings = &mut slot.pad.as_mut().unwrap().button_mappings;
        mappings.retain(|m| m.source != "r3");
        mappings.push(ButtonMapping { source: "r3".to_owned(), target: "bottom face".to_owned() });
        s.set_output("r3", &macro_output(0));
        assert_eq!(s.slot(XINPUT, 2).macro_started_by("r3"), Some(0));
        s.revert_macro(0);
        assert_eq!(s.mapped_to("r3").as_deref(), Some("A"));
        assert!(clean(&s));
    }

    #[test]
    fn a_button_it_gave_up_starts_it_again() {
        let mut s = held();
        s.remove_macro_button(0, "lp");
        s.revert_macro(0);
        assert_eq!(s.slot(XINPUT, 2).macro_started_by("lp"), Some(0));
        assert!(clean(&s));
    }

    #[test]
    fn a_button_another_macro_took_refuses() {
        let mut s = held();
        s.add_macro_button(2, "rp");
        let before = s.slot(XINPUT, 2);
        s.revert_macro(0);
        assert_eq!(s.slot(XINPUT, 2), before, "nothing changed");
        let notice = s.notice.unwrap();
        assert_eq!(notice.title, "Could not revert Jump (Macro 1).");
        assert_eq!(
            notice.body,
            "PR now starts Dash (Macro 3). Undo that first, or discard the slot’s edits."
        );
    }

    #[test]
    fn a_remapped_button_refuses() {
        let mut s = held();
        s.rename_macro(0, "Run");
        s.set_output("lp", "bottom face");
        let before = s.slot(XINPUT, 2);
        s.revert_macro(0);
        assert_eq!(s.slot(XINPUT, 2), before);
        assert!(s.notice.unwrap().body.starts_with("PL is now mapped to A."));
    }

    #[test]
    fn an_unedited_macro_does_nothing() {
        let mut s = held();
        s.rename_macro(2, "Sprint");
        let before = s.slot(XINPUT, 2);
        s.revert_macro(0);
        assert_eq!((s.slot(XINPUT, 2), s.notice), (before, None));
    }
}
