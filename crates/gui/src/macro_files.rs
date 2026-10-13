//! Import and export of one macro file in the Macros tab. An import is checked, put
//! into the slot's mode and shown in a dialog before it joins the macro edits. An
//! export saves the macro on screen under the CLI's file name. Pure: the dialogs live
//! in `chooser`.

use std::rc::Rc;

use controller_core::convert_macro::{convert_macro, TargetBank};
use controller_core::model::macros::macro_from_json;
use controller_core::model::macros::macro_to_json;
use controller_core::model::{MacroDefinition, Mode};
use controller_core::service::validation::validate_macro;
use slint::{ModelRc, SharedString, VecModel};

use crate::buttons::output_label;
use crate::files::{parse_json, problem, swapped_letters};
use crate::macros::label;
use crate::render::sentence;
use crate::review::macro_edits::steps;
use crate::state::{AppState, Notice};
use crate::step_table::trigger_control;
use crate::ui::{AppWindow, MacroImportInfo};

/// A macro file, put into the slot's mode, waiting for the user's yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadyImport {
    /// The macro as it joins the edits: in the slot's mode and its macro slot.
    pub def: MacroDefinition,
    /// The mode the file was made for.
    pub from: Mode,
    /// Each button the macro cannot start here, with why, such as "PR is imported
    /// unmapped, as Jump (Macro 1) already uses it.".
    pub dropped: Vec<String>,
    /// Each trigger glide the slot's mode cannot play, such as "ZL in step 3".
    pub glides: Vec<String>,
    /// How many steps press a trigger part way that the slot's mode presses fully.
    pub full_presses: usize,
}

/// A macro file picked for a macro slot, and what it becomes there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingMacroImport {
    /// The slot it goes into: mode and 1-based number.
    pub slot: (Mode, u8),
    /// The macro slot it goes into, 0-based.
    pub macro_slot: u8,
    /// The file's name, without its folder.
    pub file_name: String,
    /// The macro ready to import, or each reason the file cannot be imported.
    pub outcome: Result<ReadyImport, Vec<String>>,
}

/// `def` as a macro file holds it, as the CLI's `export` writes it: pretty JSON and a
/// final newline.
///
/// # Errors
/// Returns why the macro could not be turned into JSON.
pub fn macro_text(def: &MacroDefinition, device: &str) -> Result<String, String> {
    serde_json::to_string_pretty(&macro_to_json(def, device))
        .map(|t| t + "\n")
        .map_err(|e| e.to_string())
}

impl AppState {
    /// Reads file `file_name`, read as `text` (or not), for macro slot `n` of the
    /// selected slot. The dialog waits in `pending_macro_import`, also for a bad file.
    pub fn import_macro(&mut self, n: u8, file_name: &str, text: Result<String, String>) {
        let Some(slot) = self.selected_slot() else { return };
        let outcome = text.map_err(|e| vec![e]).and_then(|t| self.ready(slot, n, &t));
        self.pending_macro_import = Some(PendingMacroImport {
            slot,
            macro_slot: n,
            file_name: file_name.to_owned(),
            outcome,
        });
    }

    /// The macro in `text`, checked, made for macro slot `n` of `slot`: each button that
    /// cannot start it there is left out, and the content follows the slot's mode.
    fn ready(
        &self,
        (mode, number): (Mode, u8),
        n: u8,
        text: &str,
    ) -> Result<ReadyImport, Vec<String>> {
        let one = |e: String| vec![e];
        let (value, model) = parse_json(self.model(), text).map_err(one)?;
        let errors = validate_macro(model, &value).map_err(|e| one(sentence(&e.to_string())))?;
        if !errors.is_empty() {
            return Err(errors.iter().map(problem).collect());
        }
        let mut file = macro_from_json(&value).map_err(|e| one(sentence(&e.to_string())))?;
        let d = self.description();
        let slot = self.slot(mode, number);
        let macros = slot.shown_macros();
        let mut dropped = Vec::new();
        file.trigger_buttons.retain(|b| {
            let why = match (slot.macro_started_by(b).filter(|&o| o != n), self.mapped_to(b)) {
                (Some(other), _) => format!("{} already uses it", label(&macros, other)),
                (None, Some(output)) => format!("this slot gives it the output {output}"),
                (None, None) => return true,
            };
            dropped.push(format!("{} is imported unmapped, as {why}.", output_label(d, mode, b)));
            false
        });
        // The buttons are already checked against the slot, so the bank refuses none.
        let default = self
            .defaults()
            .get(&mode)
            .ok_or_else(|| one("This controller has no such mode.".to_owned()))?;
        let bank = TargetBank { others: &[], profile: default, default };
        let (mut def, _) =
            convert_macro(d, &file, mode, &bank).map_err(|e| one(sentence(&e.to_string())))?;
        def.macro_slot = Some(n);

        let mut glides = Vec::new();
        for (i, (was, is)) in file.steps.iter().zip(&def.steps).enumerate() {
            for b in &d.buttons {
                let Some(control) = b.step_trigger_value.map(trigger_control) else { continue };
                if was.glide.contains(&control) && !is.glide.contains(&control) {
                    glides.push(format!("{} in step {}", output_label(d, mode, &b.id), i + 1));
                }
            }
        }
        let full_presses = file
            .steps
            .iter()
            .zip(&def.steps)
            .filter(|(was, is)| {
                (was.trigger_left, was.trigger_right) != (is.trigger_left, is.trigger_right)
            })
            .count();
        Ok(ReadyImport { def, from: file.mode, dropped, glides, full_presses })
    }

    /// The user said yes to the waiting macro import: it takes its macro slot in the
    /// edits.
    pub fn confirm_macro_import(&mut self) {
        let Some(PendingMacroImport { slot, macro_slot: n, file_name, outcome }) =
            self.pending_macro_import.take()
        else {
            return;
        };
        let Ok(ready) = outcome else { return };
        if self.selected_slot() != Some(slot) {
            return;
        }
        self.edit_macros(|macros| {
            macros.retain(|m| m.macro_slot != Some(n));
            macros.push(ready.def);
            macros.sort_by_key(|m| m.macro_slot);
        });
        self.notice = Some(Notice {
            error: false,
            title: format!("Imported “{file_name}” into Macro {}.", u16::from(n) + 1),
            body: "Nothing reaches the controller until you write.".to_owned(),
        });
    }

    /// The user closed the macro import dialog.
    pub fn cancel_macro_import(&mut self) {
        self.pending_macro_import = None;
    }

    /// Saved macro slot `n` to a file, or failed to.
    pub fn exported_macro(&mut self, n: u8, unsaved: bool, result: Result<String, String>) {
        let what = format!("Macro {}", u16::from(n) + 1);
        self.notice = Some(match result {
            Ok(name) => Notice {
                error: false,
                title: format!("Saved {what} to “{name}”."),
                body: if unsaved {
                    "The file includes edits not yet written to the controller.".to_owned()
                } else {
                    String::new()
                },
            },
            Err(e) => Notice { error: true, title: format!("Could not save {what}."), body: e },
        });
    }
}

/// A list model of strings.
fn strings(items: &[String]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(items.iter().map(SharedString::from).collect::<VecModel<_>>()))
}

/// The macro an import replaces, as the dialog names it: “Dash”, or the macro without
/// a name.
fn replaced(def: &MacroDefinition) -> String {
    if def.name.is_empty() {
        "the macro without a name".to_owned()
    } else {
        format!("“{}”", def.name)
    }
}

/// The macro import dialog, hidden when no import waits.
#[must_use]
pub fn macro_import_info(state: &AppState) -> MacroImportInfo {
    let Some(p) = &state.pending_macro_import else { return MacroImportInfo::default() };
    let d = state.description();
    let (mode, number) = p.slot;
    let base = MacroImportInfo {
        shown: true,
        file_name: p.file_name.as_str().into(),
        number: i32::from(p.macro_slot) + 1,
        ..MacroImportInfo::default()
    };
    let ready = match &p.outcome {
        Err(e) => return MacroImportInfo { errors: strings(e), ..base },
        Ok(ready) => ready,
    };
    let slot = state.slot(mode, number);
    let buttons: Vec<String> =
        ready.def.trigger_buttons.iter().map(|b| output_label(d, mode, b)).collect();
    MacroImportInfo {
        name: ready.def.name.as_str().into(),
        from_mode: d.mode_label(ready.from).into(),
        to_mode: d.mode_label(mode).into(),
        steps: steps(ready.def.steps.len()).into(),
        buttons: buttons.join(", ").into(),
        replaces: slot.shown_macro(p.macro_slot).map(replaced).unwrap_or_default().into(),
        replaces_unsaved: slot.shown_macro(p.macro_slot).is_some()
            && slot.macro_unsaved(p.macro_slot),
        letters: swapped_letters(d, ready.from, mode).into(),
        full_presses: i32::try_from(ready.full_presses).unwrap_or(i32::MAX),
        glides: ready.glides.join(", ").into(),
        dropped: strings(&ready.dropped),
        ..base
    }
}

/// Pushes the macro import dialog.
pub fn render_macro_files(state: &AppState, ui: &AppWindow) {
    ui.set_macro_import(macro_import_info(state));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::{SWITCH, XINPUT};
    use controller_core::model::macros::macro_file_name;
    use controller_core::model::{GlideControl, MacroStep};
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// `XInput` slot 2 selected, holding "Jump" in macro slot 0 on PR and PL, and "Dash"
    /// in macro slot 2 on R4.
    fn held() -> AppState {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp", "lp"], 3), macro_def(2, "Dash", &["r4"], 1)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        s.select(0, 1);
        s
    }

    /// `def` as a macro file holds it.
    fn file(def: &MacroDefinition) -> String {
        macro_text(def, "8bitdo-pro3").unwrap()
    }

    fn ready(s: &AppState) -> ReadyImport {
        s.pending_macro_import.clone().unwrap().outcome.unwrap()
    }

    #[test]
    fn an_export_imports_back_into_its_macro_slot_unchanged() {
        let mut s = held();
        let jump = s.slot(XINPUT, 2).shown_macro(0).unwrap().clone();
        assert_eq!(macro_file_name(XINPUT, 2, &jump), "xinput-slot2-macro0-Jump.json");
        let text = file(&jump);
        assert!(text.ends_with("}\n"));
        s.import_macro(0, "xinput-slot2-macro0-Jump.json", Ok(text));
        let r = ready(&s);
        assert_eq!(r.def, jump, "its own buttons stay: the old macro goes");
        assert!(r.dropped.is_empty() && r.glides.is_empty() && r.full_presses == 0);
        let info = macro_import_info(&s);
        assert_eq!(info.replaces, "“Jump”");
        assert!(!info.replaces_unsaved);
        assert_eq!((info.buttons.as_str(), info.steps.as_str()), ("PR, PL", "3 steps"));
        assert_eq!((info.from_mode.as_str(), info.to_mode.as_str()), ("XInput", "XInput"));
        assert_eq!(info.letters, "");
        s.confirm_macro_import();
        assert!(!s.slot(XINPUT, 2).unsaved(), "the same macro: nothing to write");
        assert_eq!(s.pending_macro_import, None);
    }

    #[test]
    fn buttons_that_start_another_macro_or_are_mapped_are_left_out() {
        let mut s = held();
        s.set_output("l4", "bottom face");
        let def = macro_def(0, "Run", &["r4", "l4", "r3", "rp"], 2);
        s.import_macro(1, "run.json", Ok(file(&def)));
        let r = ready(&s);
        assert_eq!(
            r.dropped,
            [
                "R4 is imported unmapped, as Dash (Macro 3) already uses it.",
                "L4 is imported unmapped, as this slot gives it the output A.",
                "PR is imported unmapped, as Jump (Macro 1) already uses it."
            ]
        );
        assert_eq!(r.def.trigger_buttons, ["r3"]);
        assert_eq!(r.def.macro_slot, Some(1));
        assert_eq!(macro_import_info(&s).replaces, "", "an empty macro slot");
        s.confirm_macro_import();
        let slot = s.slot(XINPUT, 2);
        assert_eq!(slot.shown_macro(1).unwrap().name, "Run");
        assert_eq!(slot.macro_started_by("r4"), Some(2), "Dash keeps R4");
        assert!(slot.macro_unsaved(1) && !slot.macro_unsaved(0));
        assert!(s.notice.unwrap().title.starts_with("Imported “run.json” into Macro 2."));
    }

    #[test]
    fn an_analog_macro_in_an_on_off_mode_presses_triggers_fully_and_loses_their_glides() {
        let mut s = connected(SWITCH);
        let at = s.description().modes.iter().position(|m| m.id == SWITCH).unwrap();
        s.select(at, 0);
        let mut def = macro_def(0, "Ramp", &["r3"], 3);
        def.steps[0].trigger_left = 100;
        def.steps[0].glide = [GlideControl::LeftTrigger, GlideControl::LeftStick].into();
        def.steps[0].left_stick_x = 0;
        def.steps[1].trigger_left = 255;
        s.import_macro(0, "ramp.json", Ok(file(&def)));
        let r = ready(&s);
        assert_eq!(r.def.mode, SWITCH);
        assert_eq!((r.def.steps[0].trigger_left, r.full_presses), (255, 1));
        assert_eq!(r.glides, ["ZL in step 1"]);
        assert!(r.def.steps[0].glide.contains(&GlideControl::LeftStick), "a stick glide stays");
        let info = macro_import_info(&s);
        assert_eq!((info.from_mode.as_str(), info.to_mode.as_str()), ("XInput", "Switch"));
        assert!(info.letters.contains("XInput A is Switch B"), "{}", info.letters);
        assert_eq!((info.full_presses, info.glides.as_str()), (1, "ZL in step 1"));
    }

    #[test]
    fn replacing_an_edited_macro_says_its_edits_are_discarded() {
        let mut s = held();
        s.rename_macro(2, "");
        let dash = macro_def(2, "Dash", &["r4"], 1);
        s.import_macro(2, "dash.json", Ok(file(&dash)));
        let info = macro_import_info(&s);
        assert_eq!(
            (info.replaces.as_str(), info.replaces_unsaved),
            ("the macro without a name", true)
        );
    }

    #[test]
    fn a_bad_file_shows_why_and_imports_nothing() {
        let mut s = held();
        let mut long = macro_def(0, "Long", &[], 1);
        long.steps = vec![MacroStep { duration_ms: 10, ..MacroStep::default() }; 500];
        for text in [
            Ok("not json".to_owned()),
            Ok("{}".to_owned()),
            Ok(file(&long)),
            Err("No such file.".to_owned()),
        ] {
            s.import_macro(1, "bad.json", text);
            let info = macro_import_info(&s);
            assert!(info.shown && info.errors.row_count() > 0, "{info:?}");
            s.confirm_macro_import();
            assert!(!s.slot(XINPUT, 2).unsaved());
        }
        s.import_macro(1, "bad.json", Ok("{}".to_owned()));
        let errors: Vec<_> = macro_import_info(&s).errors.iter().collect();
        assert!(errors.len() > 1, "every problem, not just the first");
        assert!(errors[0].starts_with("\"version\" is"), "{errors:?}");
        s.cancel_macro_import();
        assert!(!macro_import_info(&s).shown);
    }

    #[test]
    fn export_message_says_when_edits_are_unsaved() {
        let mut s = held();
        s.exported_macro(1, true, Ok("x.json".to_owned()));
        let n = s.notice.take().unwrap();
        assert_eq!(n.title, "Saved Macro 2 to “x.json”.");
        assert!(n.body.contains("not yet written"));
        s.exported_macro(1, false, Err("Permission denied.".to_owned()));
        assert!(s.notice.unwrap().error);
    }
}
