//! The macros a profile file carries in `macros`. An export embeds the slot's macros
//! as shown; an import makes the file's macros the slot's, in the slot's mode. Pure:
//! the dialogs live in `chooser`.

use controller_core::convert_macro::{convert_macro, TargetBank};
use controller_core::description::ControllerDescription;
use controller_core::model::{CanonicalProfile, MacroDefinition, Mode};
use tr::tr;

use crate::buttons::{output_label, target};
use crate::files::export_text;
use crate::macro_files::content_losses;
use crate::macros::{named, title};
use crate::reasons::failure;
use crate::render::and_list;
use crate::state::SlotState;

/// Why a slot cannot be exported, or take a file's macros, yet.
#[must_use]
pub fn steps_unread() -> String {
    tr!("The slot’s macro steps are still being read from the controller. Try again in a moment.")
}

/// The export file of `slot`: the profile on screen, with the macros on screen in
/// `macros` and no macro references. `None` for an empty slot.
///
/// # Errors
/// Returns [`steps_unread`] while the slot's macro steps are being read, or why the
/// profile could not be turned into JSON.
pub fn export_file(slot: &SlotState) -> Option<Result<String, String>> {
    let mut profile = slot.shown()?.clone();
    let Some(macros) = slot.macros_as_shown() else {
        return Some(Err(steps_unread()));
    };
    profile.macro_refs.clear();
    profile.macros = Some(macros.into_iter().map(named).collect());
    Some(export_text(&profile))
}

/// What an import says about a file with `refs` macro references and no `macros`.
#[must_use]
pub fn refs_note(refs: usize) -> String {
    let held =
        tr!("The file’s macro sits in a separate file that 8B cannot read, so the slot keeps its \
         macros."
            | "The file’s {n} macros sit in separate files that 8B cannot read, so the slot \
               keeps its macros."
                % refs);
    format!("{held} {}", tr!("The command line’s upload --with-macros reads them."))
}

/// The titles of `defs`, such as "Jump (Macro 1) and Dash (Macro 3)".
fn titles<'a>(defs: impl IntoIterator<Item = &'a MacroDefinition>) -> String {
    let titles: Vec<String> =
        defs.into_iter().map(|m| title(m, m.macro_slot.unwrap_or_default())).collect();
    and_list(&titles)
}

/// What an import says first about a file whose macros are `file`, into a slot that
/// shows the macros `held`: the file's macros replace the slot's, each named.
#[must_use]
pub fn replace_note(file: &[MacroDefinition], held: &[&MacroDefinition]) -> Option<String> {
    let (old, new) = (titles(held.iter().copied()), titles(file));
    match (file.len(), held.len()) {
        (0, 0) => None,
        (0, h) => Some(tr!(
            "The file holds no macros, so the slot’s macro {old} is removed."
                | "The file holds no macros, so the slot’s {n} macros, {old}, are removed." % h,
            old = old
        )),
        (n, 0) => Some(tr!(
            "The file brings {n} macro: {new}." | "The file brings {n} macros: {new}." % n,
            new = new
        )),
        // One sentence per case, as the verb agrees with the file's count.
        (1, h) => Some(tr!(
            "The file’s macro {new} replaces the slot’s macro {old}."
                | "The file’s macro {new} replaces the slot’s {n} macros, {old}." % h,
            new = new,
            old = old
        )),
        (f, h) => Some(tr!(
            "The file’s {f} macros, {new}, replace the slot’s macro {old}."
                | "The file’s {f} macros, {new}, replace the slot’s {n} macros, {old}." % h,
            f = f,
            new = new,
            old = old
        )),
    }
}

/// What an import without macros says about the file's macros `file`, which stay out
/// of the slot. `None` when the file holds none.
#[must_use]
pub fn kept_note(file: &[MacroDefinition]) -> Option<String> {
    match file.len() {
        0 => None,
        n => Some(tr!(
            "The file’s macro {list} was not imported."
                | "The file’s {n} macros, {list}, were not imported." % n,
            list = titles(file)
        )),
    }
}

/// The file's macros `file` made for a slot of mode `to` that takes `profile`, in
/// macro-slot order, and one sentence per loss. A trigger button that `profile` gives
/// another output than `default` does is left out, as the controller would send the
/// output, and a macro of another mode is converted.
///
/// # Errors
/// Returns why a macro cannot be converted, such as a step button `to` lacks.
pub fn file_macros(
    d: &ControllerDescription,
    file: &[MacroDefinition],
    to: Mode,
    profile: &CanonicalProfile,
    default: &CanonicalProfile,
) -> Result<(Vec<MacroDefinition>, Vec<String>), String> {
    let mode = d.mode_label(to);
    let triggers: Vec<String> = d
        .buttons
        .iter()
        .filter(|b| b.step_trigger_value.is_some())
        .map(|b| output_label(d, to, &b.id))
        .collect();
    let (mut notes, mut out) = (Vec::new(), Vec::new());
    for m in file {
        let name = title(m, m.macro_slot.unwrap_or_default());
        let mut kept = m.clone();
        kept.trigger_buttons.retain(|b| {
            let output = target(profile, b);
            if output == target(default, b) {
                return true;
            }
            let button = output_label(d, to, b);
            notes.push(tr!(
                "The file maps {button} to {output}, so {button} cannot start {macro}.",
                button = button,
                output = output_label(d, to, output),
                macro = name
            ));
            false
        });
        // The buttons are already checked against the profile, so the bank refuses none.
        let bank = TargetBank { others: &[], profile: default, default };
        let (def, _) = convert_macro(d, &kept, to, &bank).map_err(|e| failure(d, &e))?;
        let (glides, full) = content_losses(d, to, &kept, &def);
        let triggers = and_list(&triggers);
        if full > 0 {
            notes.push(tr!(
                "In {mode} mode, {triggers} are only on or off, so {macro} presses them fully \
                 in {n} step."
                    | "In {mode} mode, {triggers} are only on or off, so {macro} presses them \
                       fully in {n} steps."
                    % full,
                mode = mode,
                triggers = triggers,
                macro = name
            ));
        }
        if !glides.is_empty() {
            notes.push(tr!(
                "In {mode} mode, {triggers} are only on or off, so {macro} drops its glide on \
                 {list}."
                    | "In {mode} mode, {triggers} are only on or off, so {macro} drops its glides \
                       on {list}."
                    % glides.len(),
                mode = mode,
                triggers = triggers,
                macro = name,
                list = and_list(&glides)
            ));
        }
        out.push(def);
    }
    out.sort_by_key(|m| m.macro_slot);
    Ok((out, notes))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::{SWITCH, XINPUT};
    use controller_core::model::GlideControl;
    use controller_core::service::validation::validate_profile;
    use serde_json::Value;
    use slint::Model as _;

    use super::*;
    use crate::files::import_warning;
    use crate::macros::PadMacro;
    use crate::state::tests::{connected, hold_macros, macro_def, PORT};
    use crate::state::AppState;

    /// `XInput` slot 2 selected, holding "Jump" in macro slot 0 on PR and PL, and "Dash"
    /// in macro slot 2 on R4.
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp", "lp"], 3), macro_def(2, "Dash", &["r4"], 1)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        s.select(0, 1);
        s
    }

    /// The export of slot `number` of `mode`.
    fn export(s: &AppState, mode: Mode, number: u8) -> String {
        export_file(&s.slot(mode, number)).unwrap().unwrap()
    }

    /// `text` with its `macros` replaced by `macros`.
    fn with_macros(text: &str, macros: &[MacroDefinition]) -> String {
        let mut p: CanonicalProfile = serde_json::from_str(text).unwrap();
        p.macros = Some(macros.to_vec());
        export_text(&p).unwrap()
    }

    #[test]
    fn an_export_embeds_the_macros_on_screen_and_is_a_valid_profile() {
        let mut s = held();
        s.rename_macro(2, "Sprint");
        let text = export(&s, XINPUT, 2);
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(validate_profile(s.model().unwrap(), &value).unwrap().valid, "{text}");
        assert_eq!(value["macro_refs"], serde_json::json!([]));
        let p: CanonicalProfile = serde_json::from_value(value).unwrap();
        let names: Vec<_> = p.macros.unwrap().into_iter().map(|m| m.name).collect();
        assert_eq!(names, ["Jump", "Sprint"]);
    }

    #[test]
    fn an_export_leaves_out_a_button_the_edits_remapped() {
        let mut s = held();
        let pad = s.controller_mut(PORT).unwrap().slots.get_mut(&(XINPUT, 2)).unwrap();
        pad.pad.as_mut().unwrap().macro_refs = vec![controller_core::model::MacroRef {
            trigger_buttons: vec!["rp".to_owned(), "lp".to_owned()],
            path: "j.json".to_owned(),
        }];
        s.set_output("rp", "bottom face");
        let p: CanonicalProfile = serde_json::from_str(&export(&s, XINPUT, 2)).unwrap();
        assert_eq!(p.macros.unwrap()[0].trigger_buttons, ["lp"]);
    }

    #[test]
    fn an_export_or_a_macro_import_waits_for_the_steps() {
        let mut s = held();
        let slot = s.controller_mut(PORT).unwrap().slots.get_mut(&(XINPUT, 2)).unwrap();
        let text = export_file(slot).unwrap().unwrap();
        slot.pad_macros[0] = PadMacro { loaded: false, ..slot.pad_macros[0].clone() };
        assert_eq!(export_file(slot), Some(Err(steps_unread())));
        assert_eq!(export_file(&s.slot(XINPUT, 3)), None, "an empty slot");

        let before = s.slot(XINPUT, 2);
        s.import((XINPUT, 2), "a.json", Ok(text));
        let n = s.notice.take().unwrap();
        assert!(n.error && n.body == steps_unread(), "{n:?}");
        assert_eq!(s.slot(XINPUT, 2), before);
    }

    #[test]
    fn an_export_imports_back_unchanged() {
        let mut s = held();
        let text = export(&s, XINPUT, 2);
        s.import((XINPUT, 2), "a.json", Ok(text));
        s.confirm_import();
        assert!(!s.slot(XINPUT, 2).unsaved(), "{:?}", s.slot(XINPUT, 2).edited_macros);
    }

    /// `held()` with a file waiting that names the profile "Other", maps R4 to A, and
    /// brings Ramp in macro slot 0 and Hop in macro slot 1.
    fn waiting_with_macros() -> AppState {
        let mut s = held();
        // The pad's refs, as a read gives them: a keep takes R4 off them.
        let slot = s.controller_mut(PORT).unwrap().slots.get_mut(&(XINPUT, 2)).unwrap();
        slot.pad.as_mut().unwrap().macro_refs = vec![controller_core::model::MacroRef {
            trigger_buttons: vec!["r4".to_owned()],
            path: "d.json".to_owned(),
        }];
        let mut p: CanonicalProfile = serde_json::from_str(&export(&s, XINPUT, 2)).unwrap();
        "Other".clone_into(&mut p.name);
        p.button_mappings.iter_mut().find(|m| m.source == "r4").unwrap().target =
            "bottom face".to_owned();
        p.macros = Some(vec![macro_def(0, "Ramp", &["r3"], 2), macro_def(1, "Hop", &["lp"], 2)]);
        s.import((XINPUT, 2), "a.json", Ok(export_text(&p).unwrap()));
        assert!(s.pending_import.is_some(), "a same-mode file whose macros touch the slot");
        s
    }

    #[test]
    fn import_without_macros_keeps_the_slots_macros_and_import_replaces_them() {
        let mut s = waiting_with_macros();
        assert_eq!(
            import_warning(&s).macro_lead,
            "The file’s 2 macros, Ramp (Macro 1) and Hop (Macro 2), replace the slot’s 2 \
             macros, Jump (Macro 1) and Dash (Macro 3)."
        );
        assert_eq!(
            import_warning(&s).keep_note,
            "Import without macros keeps the slot’s 2 macros. The file maps R4 to A, so R4 no \
             longer starts Dash (Macro 3)."
        );
        s.import_without_macros();
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.edited.as_ref().unwrap().name, "Other");
        let names: Vec<_> = slot.shown_macros().iter().map(|m| m.name.clone()).collect();
        assert_eq!(names, ["Jump", "Dash"]);
        assert_eq!(slot.macro_started_by("r4"), None, "the file maps R4 elsewhere");
        assert_eq!(
            s.notice.unwrap().body,
            "The file’s 2 macros, Ramp (Macro 1) and Hop (Macro 2), were not imported. The file \
             maps R4 to A, so R4 no longer starts Dash (Macro 3). Nothing reaches the controller \
             until you write."
        );

        let mut s = waiting_with_macros();
        s.confirm_import();
        let names: Vec<_> =
            s.slot(XINPUT, 2).shown_macros().iter().map(|m| m.name.clone()).collect();
        assert_eq!(names, ["Ramp", "Hop"]);
        assert_eq!(
            kept_note(&[macro_def(1, "Hop", &[], 1)]).unwrap(),
            "The file’s macro Hop (Macro 2) was not imported."
        );
        assert_eq!(kept_note(&[]), None);
    }

    #[test]
    fn import_without_macros_keeps_unwritten_macro_edits() {
        let mut s = held();
        s.rename_macro(0, "Leap");
        let text = with_macros(&export(&s, XINPUT, 1), &[macro_def(1, "Hop", &["lp"], 2)]);
        s.import((XINPUT, 2), "a.json", Ok(text));
        let notes: Vec<String> = import_warning(&s).edit_notes.iter().map(Into::into).collect();
        assert_eq!(
            notes,
            ["Import discards your macro edits, not written yet. Import without macros keeps them."]
        );
        s.import_without_macros();
        assert_eq!(s.slot(XINPUT, 2).edited_macros.unwrap()[0].name, "Leap");
    }

    #[test]
    fn a_file_with_macros_replaces_the_slots_macros() {
        let mut s = held();
        let mut hop = macro_def(0, "Hop", &["rp", "lp"], 3);
        hop.repeat_count = 2;
        let run = macro_def(1, "Run", &["r3"], 2);
        let text = with_macros(&export(&s, XINPUT, 2), &[run.clone(), hop.clone()]);
        s.import((XINPUT, 2), "a.json", Ok(text));
        assert_eq!(s.slot(XINPUT, 2).edited_macros, None, "it asks first");
        s.confirm_import();
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.edited_macros, Some(vec![hop, run]), "in macro-slot order");
        assert_eq!(slot.macro_started_by("r4"), None, "Dash is gone");
        assert!(slot.edited.unwrap().macros.is_none(), "the edits hold no copy");
        let body = s.notice.unwrap().body;
        assert!(
            body.starts_with(
                "Hop replaces Jump in Macro 1. Run is added in Macro 2. Dash is removed from \
                 Macro 3. Nothing"
            ),
            "{body}"
        );
    }

    #[test]
    fn an_import_says_it_replaces_edits_not_written_yet() {
        let mut s = held();
        s.rename_macro(0, "Leap");
        let text = export(&s, XINPUT, 1);
        s.import((XINPUT, 2), "a.json", Ok(text.clone()));
        let w = import_warning(&s);
        assert!(w.edit_notes.row_count() > 0);
        s.confirm_import();
        assert!(!s.notice.take().unwrap().body.contains("not written"), "the dialog said it");

        // A file of another mode asks too.
        s.rename_macro(0, "Leap");
        let mut switch: CanonicalProfile = serde_json::from_str(&text).unwrap();
        switch = controller_core::convert::convert_profile(
            s.description(),
            &switch,
            &s.defaults()[&XINPUT],
            &s.defaults()[&SWITCH],
        )
        .unwrap()
        .0;
        switch.macros = Some(vec![]);
        s.import((XINPUT, 2), "s.json", Ok(export_text(&switch).unwrap()));
        assert!(import_warning(&s).edit_notes.row_count() > 0);
    }

    #[test]
    fn an_empty_macros_list_clears_the_slots_macros() {
        let mut s = held();
        let text = with_macros(&export(&s, XINPUT, 2), &[]);
        s.import((XINPUT, 2), "a.json", Ok(text));
        s.confirm_import();
        assert_eq!(s.slot(XINPUT, 2).edited_macros, Some(vec![]));
        assert!(s.notice.unwrap().body.contains("Dash is removed from Macro 3."));
    }

    #[test]
    fn a_macro_button_the_file_remaps_does_not_start_the_macro() {
        let mut s = held();
        let mut p: CanonicalProfile = serde_json::from_str(&export(&s, XINPUT, 2)).unwrap();
        p.button_mappings.iter_mut().find(|m| m.source == "r3").unwrap().target =
            "bottom face".to_owned();
        p.macros = Some(vec![macro_def(1, "Run", &["r3", "r4"], 2)]);
        s.import((XINPUT, 2), "a.json", Ok(export_text(&p).unwrap()));
        let notes: Vec<String> = import_warning(&s).macro_notes.iter().map(Into::into).collect();
        assert_eq!(notes, ["The file maps RS to A, so RS cannot start Run (Macro 2)."]);
        s.confirm_import();
        let edited = s.slot(XINPUT, 2).edited_macros.unwrap();
        assert_eq!(edited[0].trigger_buttons, ["r4"]);
        assert!(!s.notice.unwrap().body.contains("cannot start"), "the dialog said it");
    }

    #[test]
    fn a_file_of_another_mode_converts_its_macros_after_a_yes() {
        let mut s = connected(SWITCH);
        let mut ramp = macro_def(0, "Ramp", &["r3"], 3);
        ramp.steps[0].trigger_left = 100;
        ramp.steps[0].glide = [GlideControl::LeftTrigger].into();
        ramp.steps[1].trigger_left = 255;
        let text = with_macros(&export(&s, XINPUT, 1), &[ramp]);
        s.import((SWITCH, 1), "x.json", Ok(text));
        assert_eq!(s.slot(SWITCH, 1).edited_macros, None, "nothing before the yes");
        let w = import_warning(&s);
        assert_eq!(w.macro_lead, "The file brings 1 macro: Ramp (Macro 1).");
        assert_eq!(w.edit_notes.row_count(), 0);
        let notes: Vec<String> = w.macro_notes.iter().map(|n| n.to_string()).collect();
        assert_eq!(
            notes,
            [
                "In Switch mode, ZL and ZR are only on or off, so Ramp (Macro 1) presses them \
                 fully in 1 step.",
                "In Switch mode, ZL and ZR are only on or off, so Ramp (Macro 1) drops its glide \
                 on ZL in step 1."
            ]
        );
        s.confirm_import();
        let body = s.notice.take().unwrap().body;
        assert_eq!(
            body,
            "Ramp is added in Macro 1. Nothing reaches the controller until you write."
        );
        let edited = s.slot(SWITCH, 1).edited_macros.unwrap();
        assert_eq!((edited[0].mode, edited[0].steps[0].trigger_left), (SWITCH, 255));
    }

    #[test]
    fn notes_count_macros_and_files() {
        let one = macro_def(1, "Hop", &[], 1);
        assert_eq!(
            replace_note(std::slice::from_ref(&one), &[&one, &one]).unwrap(),
            "The file’s macro Hop (Macro 2) replaces the slot’s 2 macros, Hop (Macro 2) and Hop \
             (Macro 2)."
        );
        let (jump, dash) = (macro_def(0, "Jump", &[], 1), macro_def(2, "", &[], 1));
        assert_eq!(replace_note(&[], &[]), None);
        assert_eq!(
            replace_note(&[jump.clone(), dash.clone()], &[&jump]).unwrap(),
            "The file’s 2 macros, Jump (Macro 1) and Macro 3, replace the slot’s macro \
             Jump (Macro 1)."
        );
        assert_eq!(
            replace_note(&[], &[&jump, &dash]).unwrap(),
            "The file holds no macros, so the slot’s 2 macros, Jump (Macro 1) and Macro 3, are \
             removed."
        );
        assert!(refs_note(1).starts_with("The file’s macro sits in a separate file"));
        assert!(refs_note(3).contains("3 macros sit in separate files"));
    }
}
