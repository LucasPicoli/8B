//! Import and export of profile JSON. An import loads a file into a slot's edits,
//! converting a file made for another mode after a warning. An export saves the
//! profile on screen in the CLI's `export` shape. Pure: the dialogs live in
//! `chooser`.

use std::path::Path;
use std::rc::Rc;

use controller_core::convert::{convert_profile, ConversionLoss};
use controller_core::description::{ControllerDescription, SettingsFrame, DISABLED_OUTPUT};
use controller_core::device::Model;
use controller_core::model::{CanonicalProfile, MacroDefinition, Mode};
use controller_core::service::validation::{validate_profile, ValidationError};
use controller_core::{Error, Reason};
use serde_json::Value;
use slint::{ModelRc, VecModel};
use tr::tr;

use crate::buttons::{output_label, target};
use crate::embedded::{file_macros, kept_note, refs_note, replace_note, steps_unread};
use crate::language::prose;
use crate::macro_files::strings;
use crate::macros::label;
use crate::reasons::{failure, problems};
use crate::render::{and_list, slot_name};
use crate::state::{AppState, Notice};
use crate::ui::{AppWindow, Change, ImportWarning};

/// An import waiting for the user's yes: a file made for another mode, converted, or one
/// that loses something on the slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingImport {
    /// The slot it goes into: mode and 1-based number.
    pub slot: (Mode, u8),
    /// The file's name, without its folder.
    pub file_name: String,
    /// The mode the file was made for.
    pub from: Mode,
    /// The converted profile, ready for the slot's edits.
    pub profile: CanonicalProfile,
    /// What the conversion changed.
    pub losses: Vec<ConversionLoss>,
    /// The file's macros, in the slot's mode, which replace the slot's. `None` when
    /// the file has no `macros`: the slot keeps its own.
    pub macros: Option<Vec<MacroDefinition>>,
    /// One sentence per thing to say about the macros: what the file's lose in the
    /// slot's mode, or that its macro references are skipped.
    pub macro_notes: Vec<String>,
}

/// The name an export suggests, as the CLI's `export` names its files.
#[must_use]
pub fn file_name(mode: Mode, number: u8) -> String {
    format!("profile-{mode}-slot-{number}-index-{}.json", number.saturating_sub(1))
}

/// `profile` as an export file holds it: pretty JSON and a final newline.
///
/// # Errors
/// Returns why the profile could not be turned into JSON.
pub fn export_text(profile: &CanonicalProfile) -> Result<String, String> {
    serde_json::to_string_pretty(profile).map(|t| t + "\n").map_err(|e| e.to_string())
}

/// The file name of `path`, for messages.
#[must_use]
pub fn display_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The JSON in `text`, and the model that checks it.
///
/// # Errors
/// Returns why `text` is not JSON, or that `model` is missing.
pub fn parse_json<'a>(
    model: Option<&'a dyn Model>,
    text: &str,
) -> Result<(Value, &'a dyn Model), String> {
    let value: Value = serde_json::from_str(text).map_err(|e| {
        log::info!("not JSON: {e}");
        tr!(
            "This file is not valid JSON (line {line}, column {column}).",
            line = e.line(),
            column = e.column()
        )
    })?;
    let model = model.ok_or_else(|| tr!("This controller model cannot check files."))?;
    Ok((value, model))
}

/// The sentence for the first of `errors`, with a count of the rest. Each one goes to
/// the log. `None` when there are none.
#[must_use]
pub fn first_error(d: &ControllerDescription, errors: &[ValidationError]) -> Option<String> {
    let mut shown = problems(d, errors).into_iter();
    let first = shown.next()?;
    Some(match shown.count() {
        0 => first,
        n => format!("{first} {}", tr!("({n} more problem)" | "({n} more problems)" % n)),
    })
}

/// The profile in `text`, checked as a profile of `model`, which `d` describes.
fn parse(
    d: &ControllerDescription,
    model: Option<&dyn Model>,
    text: &str,
) -> Result<CanonicalProfile, String> {
    let (value, model) = parse_json(model, text)?;
    // A macro file fails every profile rule; one reason says what it is instead. Only a
    // macro file has steps.
    if value.get("steps").is_some_and(Value::is_array) {
        // Why a macro file picked as a profile does not import, and where it goes.
        return Err(tr!(
            "This is a macro file, not a profile. Import it with “{import}” on the {tab} tab.",
            import = tr!("Import macro…"),
            tab = tr!("Macros")
        ));
    }
    let check = validate_profile(model, &value).map_err(|e| failure(d, &e))?;
    if let Some(e) = first_error(d, &check.errors) {
        return Err(e);
    }
    serde_json::from_value(value)
        .map_err(|e| failure(d, &Error::Invalid(Reason::NotAProfile(e.to_string()))))
}

impl AppState {
    /// Saved slot `slot`'s profile to a file, or failed to.
    pub fn exported(&mut self, slot: (Mode, u8), unsaved: bool, result: Result<String, String>) {
        let mode = self.description().mode_label(slot.0);
        self.notice = Some(match result {
            Ok(name) => Notice {
                error: false,
                title: tr!(
                    "Saved {mode} slot {n} to “{file}”.",
                    mode = mode,
                    n = slot.1,
                    file = name
                ),
                body: if unsaved {
                    tr!("The file includes edits not written to the controller yet.")
                } else {
                    String::new()
                },
            },
            Err(e) => Notice {
                error: true,
                title: tr!("Could not save {mode} slot {n}.", mode = mode, n = slot.1),
                body: e,
            },
        });
    }

    /// Loads file `file_name`, read as `text` (or not), into slot `slot`'s edits. A
    /// file for another mode, or one that loses something on the slot, waits in
    /// `pending_import` for the user's yes.
    pub fn import(&mut self, slot: (Mode, u8), file_name: &str, text: Result<String, String>) {
        let parsed = text.and_then(|t| parse(self.description(), self.model(), &t));
        let converted = parsed.and_then(|mut p| {
            let default =
                |m| self.defaults().get(&m).ok_or_else(|| tr!("This controller has no such mode."));
            let file = p.macros.take();
            let (profile, losses) =
                convert_profile(self.description(), &p, default(p.mode)?, default(slot.0)?)
                    .map_err(|e| failure(self.description(), &e))?;
            let target = self.slot(slot.0, slot.1);
            let (macros, macro_notes) = match file {
                None if p.macro_refs.is_empty() => (None, Vec::new()),
                None => (None, vec![refs_note(p.macro_refs.len())]),
                // The old steps tell an unchanged macro from a changed one in the review.
                Some(_) if !target.macros_read() => return Err(steps_unread()),
                Some(file) => {
                    let (macros, losses) =
                        file_macros(self.description(), &file, slot.0, &profile, default(slot.0)?)?;
                    (Some(macros), losses)
                }
            };
            Ok(PendingImport {
                slot,
                file_name: file_name.to_owned(),
                from: p.mode,
                profile,
                losses,
                macros,
                macro_notes,
            })
        });
        match converted {
            Err(body) => {
                self.notice = Some(Notice {
                    error: true,
                    title: tr!("Could not import “{file}”.", file = file_name),
                    body,
                });
            }
            Ok(pending) => {
                let loses = macro_lead(self, &pending).is_some() || replaces_edits(self, &pending);
                if pending.from == slot.0 && !loses {
                    self.apply_import(pending, false, String::new());
                } else {
                    self.pending_import = Some(pending);
                }
            }
        }
    }

    /// The user said yes to the waiting import.
    pub fn confirm_import(&mut self) {
        if let Some(pending) = self.pending_import.take() {
            self.apply_import(pending, true, String::new());
        }
    }

    /// The user chose the waiting import without the file's macros: the profile only,
    /// the slot keeps its macros.
    pub fn import_without_macros(&mut self) {
        if let Some(mut pending) = self.pending_import.take() {
            let kept = pending.macros.take().and_then(|m| kept_note(&m));
            pending.macro_notes.clear();
            self.apply_import(pending, true, kept.map(|k| k + " ").unwrap_or_default());
        }
    }

    /// The user said no to the waiting import.
    pub fn cancel_import(&mut self) {
        self.pending_import = None;
    }

    /// Each button of slot `key` that starts a macro there but that `imported` gives
    /// another output than the mode's default, which a macro button keeps, with the
    /// sentence that says so.
    fn remapped_starters(
        &self,
        key: (Mode, u8),
        imported: &CanonicalProfile,
    ) -> Vec<(String, String)> {
        let Some(default) = self.defaults().get(&key.0) else { return Vec::new() };
        let slot = self.slot(key.0, key.1);
        let d = self.description();
        let macros = slot.shown_macros();
        d.buttons
            .iter()
            .filter_map(|b| {
                let n = slot.macro_started_by(&b.id)?;
                let output = target(imported, &b.id);
                (output != target(default, &b.id)).then(|| {
                    let button = output_label(d, key.0, &b.id);
                    let to = output_label(d, key.0, output);
                    let name = label(&macros, n);
                    let sentence = tr!(
                        "The file maps {button} to {output}, so {button} no longer starts {macro}.",
                        button = button,
                        output = to,
                        macro = name
                    );
                    (b.id.clone(), sentence)
                })
            })
            .collect()
    }

    /// Takes each button of the selected slot that starts a macro off it when the import
    /// gives the button another output than the mode's default. Returns one sentence per
    /// button, each followed by a space, or nothing.
    fn leave_for_import(&mut self) -> String {
        let Some(key) = self.selected_slot() else { return String::new() };
        let Some(imported) = self.slot(key.0, key.1).edited else { return String::new() };
        let (buttons, sentences): (Vec<String>, Vec<String>) =
            self.remapped_starters(key, &imported).into_iter().unzip();
        if buttons.is_empty() {
            return String::new();
        }
        self.leave_macros(&buttons);
        sentences.into_iter().map(|s| s + " ").collect()
    }

    /// Puts an import into its slot's edits and shows that slot. The slot keeps its
    /// own id. The file's macros replace the slot's; a file with none leaves the slot's.
    /// `said` is true when the dialog was shown, which the notice does not repeat;
    /// `lead` opens the notice.
    fn apply_import(&mut self, pending: PendingImport, said: bool, lead: String) {
        let PendingImport {
            slot: (mode, number), file_name, mut profile, macros, macro_notes, ..
        } = pending;
        let active = self.active().map(|c| c.port.clone());
        let default = self.defaults().get(&mode).cloned();
        let Some(c) = self.controllers.iter_mut().find(|c| Some(&c.port) == active.as_ref()) else {
            return;
        };
        let state = c.slots.entry((mode, number)).or_default();
        let replaced = macros.is_some();
        let base = state.pad.as_ref().or(default.as_ref());
        profile.id = base.map(|b| b.id.clone()).unwrap_or_default();
        profile.preferred_slot = base.and_then(|b| b.preferred_slot);
        profile.macro_refs = state.pad.as_ref().map(|p| p.macro_refs.clone()).unwrap_or_default();
        state.edited = Some(profile);
        if replaced {
            state.edited_macros = macros;
        }
        if let Some(i) = self.description().modes.iter().position(|m| m.id == mode) {
            self.select(i, usize::from(number).saturating_sub(1));
        }
        let left = if replaced { String::new() } else { self.leave_for_import() };
        let mut notes: Vec<String> = self.import_lines((mode, number));
        if !said {
            notes.extend(macro_notes);
        }
        let notes: String = notes.into_iter().map(|n| n + " ").collect();
        self.notice = Some(Notice {
            error: false,
            title: tr!(
                "Imported “{file}” into {mode} slot {n}.",
                file = file_name,
                mode = self.description().mode_label(mode),
                n = number
            ),
            body: lead + &left + &notes + &tr!("Nothing reaches the controller until you write."),
        });
    }
}

/// Buttons whose letter in `to` names another button in `from`, as
/// `XInput B is Switch A`, joined for a sentence. A label that only changes form, such
/// as LB to L, is left out.
#[must_use]
pub fn swapped_letters(description: &ControllerDescription, from: Mode, to: Mode) -> String {
    let label = |b: &controller_core::description::Button, m| b.labels.get(&m).cloned();
    let pairs: Vec<String> = description
        .buttons
        .iter()
        .filter_map(|b| Some((label(b, from)?, label(b, to)?)))
        .filter(|(f, t)| {
            f != t && description.buttons.iter().any(|o| label(o, from).as_ref() == Some(t))
        })
        .map(|(f, t)| {
            tr!(
                "{mode} {button} is {other_mode} {other}",
                mode = description.mode_label(from),
                button = f,
                other_mode = description.mode_label(to),
                other = t
            )
        })
        .collect();
    and_list(&pairs)
}

/// Which of the slot's macros the file's replace, each named. `None` for a file with no
/// `macros`, or when neither holds a macro.
fn macro_lead(state: &AppState, p: &PendingImport) -> Option<String> {
    let slot = state.slot(p.slot.0, p.slot.1);
    p.macros.as_ref().and_then(|m| replace_note(m, &slot.shown_macros()))
}

/// Whether `p` replaces edits to its slot not written yet.
fn replaces_edits(state: &AppState, p: &PendingImport) -> bool {
    let slot = state.slot(p.slot.0, p.slot.1);
    slot.profile_unsaved() || (p.macros.is_some() && slot.macros_unsaved())
}

/// What Import without macros does, which the dialog offers when `lead` names the
/// macros Import replaces: the slot's macros stay, less each button the file maps
/// elsewhere. Empty with no `lead`.
fn keep_note(state: &AppState, p: &PendingImport, lead: Option<&String>) -> String {
    let (Some(_), Some(file)) = (lead, &p.macros) else { return String::new() };
    let held = state.slot(p.slot.0, p.slot.1).shown_macros().len();
    let keep = tr!("Import without macros");
    let kept = match held {
        0 => tr!(
            "“{button}” leaves it out." | "“{button}” leaves them out." % file.len(),
            button = keep
        ),
        h => tr!(
            "“{button}” keeps the slot’s macro." | "“{button}” keeps the slot’s {n} macros." % h,
            button = keep
        ),
    };
    state.remapped_starters(p.slot, &p.profile).into_iter().fold(kept, |all, (_, s)| all + " " + &s)
}

/// One sentence per kind of unwritten edit `p` discards, saying which choice keeps it.
fn edit_notes(state: &AppState, p: &PendingImport, lead: Option<&String>) -> Vec<String> {
    let slot = state.slot(p.slot.0, p.slot.1);
    let mut notes = Vec::new();
    if slot.profile_unsaved() {
        notes.push(tr!(
            "Your profile edits to {mode} slot {n}, not written yet, are discarded.",
            mode = state.description().mode_label(p.slot.0),
            n = p.slot.1
        ));
    }
    if p.macros.is_some() && slot.macros_unsaved() {
        notes.push(if lead.is_some() {
            tr!(
                "“{import}” discards your macro edits, not written yet. “{keep}” keeps them.",
                import = tr!("Import"),
                keep = tr!("Import without macros")
            )
        } else {
            tr!("Your macro edits, not written yet, are discarded.")
        });
    }
    notes
}

/// The dialog before an import that waits, empty when none waits.
#[must_use]
pub fn import_warning(state: &AppState) -> ImportWarning {
    let Some(p) = &state.pending_import else { return ImportWarning::default() };
    let d = state.description();
    let (from, to) = (p.from, p.slot.0);
    let lead = macro_lead(state, p);
    let lost: Vec<Change> = p
        .losses
        .iter()
        .filter_map(|l| match l {
            ConversionLoss::OutputDisabled { source, target } => Some(Change {
                what: output_label(d, from, source).into(),
                from: output_label(d, from, target).into(),
                to: output_label(d, to, DISABLED_OUTPUT).into(),
            }),
            ConversionLoss::SettingsReset { .. } => None,
        })
        .collect();
    ImportWarning {
        shown: true,
        slot_title: slot_name(d, to, p.slot.1).into(),
        file_name: p.file_name.as_str().into(),
        from_mode: state.description().mode_label(from).into(),
        to_mode: state.description().mode_label(to).into(),
        lost: ModelRc::from(Rc::new(VecModel::from(lost))),
        reset: reset_frames(d, to, &p.losses).into(),
        letters: swapped_letters(d, from, to).into(),
        keep_note: keep_note(state, p, lead.as_ref()).into(),
        edit_notes: strings(&edit_notes(state, p, lead.as_ref())),
        macro_lead: lead.unwrap_or_default().into(),
        macro_notes: strings(&p.macro_notes),
    }
}

/// The titles of the frames of `mode` that show a settings group a conversion reset,
/// such as "Left trigger and Right trigger", joined for a sentence. A group no frame
/// shows reads as its key, such as `triggers`.
fn reset_frames(d: &ControllerDescription, mode: Mode, losses: &[ConversionLoss]) -> String {
    let mut titles: Vec<String> = Vec::new();
    for l in losses {
        let ConversionLoss::SettingsReset { group } = l else { continue };
        let prefix = format!("/{group}/");
        let shows = |f: &&SettingsFrame| {
            let numbers = f.sliders.iter().flat_map(|s| std::iter::once(&s.low).chain(&s.high));
            numbers
                .map(|n| &n.field)
                .chain(f.flags.iter().map(|f| &f.field))
                .any(|p| p.starts_with(&prefix))
        };
        let frames: Vec<String> =
            d.pages(mode).flat_map(|p| &p.frames).filter(shows).map(|f| prose(&f.title)).collect();
        if frames.is_empty() {
            titles.push(group.clone());
        }
        for title in frames {
            if !titles.contains(&title) {
                titles.push(title);
            }
        }
    }
    and_list(&titles)
}

/// Pushes the import warning and the message bar.
pub fn render_files(state: &AppState, ui: &AppWindow) {
    ui.set_import_warning(import_warning(state));
    let notice = state.notice.as_ref();
    ui.set_notice_title(notice.map(|n| n.title.as_str()).unwrap_or_default().into());
    ui.set_notice_body(notice.map(|n| n.body.as_str()).unwrap_or_default().into());
    ui.set_notice_error(notice.is_some_and(|n| n.error));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::description::UNRECOGNISED_OUTPUT;
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use controller_core::model::MacroRef;
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{connected, hold_macros, macro_def};

    /// Gives `source` the output `target`.
    fn remap(p: &mut CanonicalProfile, source: &str, target: &str) {
        let m = p.button_mappings.iter_mut().find(|m| m.source == source).unwrap();
        target.clone_into(&mut m.target);
    }

    /// A Switch default profile as an export file holds it, after `change`.
    fn switch_file(s: &AppState, change: impl FnOnce(&mut CanonicalProfile)) -> String {
        let mut p = s.defaults()[&SWITCH].clone();
        "switch-slot-1-index-0".clone_into(&mut p.id);
        "Switch".clone_into(&mut p.name);
        change(&mut p);
        export_text(&p).unwrap()
    }

    /// Slot `number` of `mode` as an export file holds it, after `change`.
    fn file(
        s: &AppState,
        mode: Mode,
        number: u8,
        change: impl FnOnce(&mut CanonicalProfile),
    ) -> String {
        let mut p = s.slot(mode, number).pad.unwrap();
        change(&mut p);
        export_text(&p).unwrap()
    }

    #[test]
    fn export_names_and_shape_match_the_cli() {
        let s = connected(XINPUT);
        assert_eq!(file_name(SWITCH, 3), "profile-switch-slot-3-index-2.json");
        let text = file(&s, XINPUT, 1, |_| {});
        assert!(text.ends_with("}\n"));
        let back: Value = serde_json::from_str(&text).unwrap();
        assert!(validate_profile(s.model().unwrap(), &back).unwrap().valid);
    }

    #[test]
    fn a_same_mode_file_round_trips_into_the_edits() {
        let mut s = connected(XINPUT);
        let text = file(&s, XINPUT, 1, |p| {
            p.name = "From file".to_owned();
            p.id = "xinput-slot-1-index-0".to_owned();
            remap(p, "r1", "disabled");
        });
        s.import((XINPUT, 2), "a.json", Ok(text));
        assert_eq!(s.pending_import, None);
        assert_eq!(s.selected_slot(), Some((XINPUT, 2)), "the import shows its slot");
        let slot = s.slot(XINPUT, 2);
        let edited = slot.edited.as_ref().unwrap();
        assert_eq!(edited.name, "From file");
        assert_eq!(edited.id, slot.pad.as_ref().unwrap().id, "the slot keeps its own id");
        assert_eq!(edited.macro_refs, slot.pad.as_ref().unwrap().macro_refs, "macros stay");
        assert!(slot.unsaved());
        assert!(s.notice.as_ref().is_some_and(|n| !n.error));

        // Export of the edits, imported back unchanged, leaves the edits as they were.
        let again = export_text(edited).unwrap();
        s.import((XINPUT, 2), "b.json", Ok(again));
        assert_eq!(s.slot(XINPUT, 2), slot);
    }

    #[test]
    fn a_same_mode_file_waits_when_it_replaces_unwritten_edits() {
        let mut s = connected(XINPUT);
        s.select(0, 1);
        s.set_output("r1", "disabled");
        let text = file(&s, XINPUT, 1, |p| p.name = "From file".to_owned());
        s.import((XINPUT, 2), "a.json", Ok(text));
        let w = import_warning(&s);
        let notes: Vec<String> = w.edit_notes.iter().map(Into::into).collect();
        assert!(w.shown);
        assert_eq!(notes, ["Your profile edits to XInput slot 2, not written yet, are discarded."]);
        assert_eq!((w.macro_lead.as_str(), w.keep_note.as_str()), ("", ""), "no keep choice");
        s.confirm_import();
        assert_eq!(s.slot(XINPUT, 2).edited.unwrap().name, "From file");
        assert!(!s.notice.unwrap().body.contains("not written"), "the dialog said it");
    }

    #[test]
    fn a_file_with_the_dpad_swap_and_an_inverted_left_stick_is_refused() {
        let mut s = connected(XINPUT);
        let text = file(&s, XINPUT, 1, |p| {
            p.set_setting("/sticks/invert_left_x", true.into());
            p.set_setting("/sticks/swap_dpad_with_left_stick", true.into());
        });
        s.import((XINPUT, 2), "clash.json", Ok(text));
        let notice = s.notice.clone().unwrap();
        assert!(notice.error);
        assert_eq!(
            notice.body,
            "Swaps, “Swap the D-pad and the left stick” cannot be on together with “Invert X”."
        );
        assert!(!s.slot(XINPUT, 2).unsaved());
    }

    #[test]
    fn an_import_takes_a_macro_button_off_its_macro_when_the_file_gives_it_another_output() {
        let mut s = connected(XINPUT);
        let pad =
            s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
        pad.macro_refs = vec![MacroRef {
            trigger_buttons: vec!["rp".to_owned(), "lp".to_owned()],
            path: "xinput-slot1-macro0-Jump.json".to_owned(),
        }];
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(0, "Jump", &["rp", "lp"], 2)]);
        // Both paddles are at rest in the file; it remaps one of them.
        let text = file(&s, XINPUT, 1, |p| remap(p, "rp", "right face"));
        s.import((XINPUT, 1), "a.json", Ok(text));
        let slot = s.slot(XINPUT, 1);
        assert_eq!(slot.macro_started_by("rp"), None);
        assert_eq!(slot.macro_started_by("lp"), Some(0), "the other paddle keeps the macro");
        let refs = &slot.edited.as_ref().unwrap().macro_refs;
        assert_eq!(refs.len(), 1, "the macro stays");
        assert_eq!(refs[0].trigger_buttons, ["lp"]);
        let body = s.notice.unwrap().body;
        assert!(body.starts_with("The file maps PR to B, so PR no longer starts Jump"), "{body}");
    }

    #[test]
    fn an_import_leaves_a_macro_button_that_keeps_its_own_output_alone() {
        let mut s = connected(XINPUT);
        let pad =
            s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
        pad.macro_refs = vec![MacroRef {
            trigger_buttons: vec!["rp".to_owned()],
            path: "xinput-slot1-macro0-Jump.json".to_owned(),
        }];
        hold_macros(&mut s, (XINPUT, 1), &[macro_def(0, "Jump", &["rp"], 2)]);
        let text = file(&s, XINPUT, 1, |p| p.name = "Other".to_owned());
        s.import((XINPUT, 1), "a.json", Ok(text));
        assert!(s.notice.as_ref().is_some_and(|n| !n.error), "{:?}", s.notice);
        assert_eq!(s.slot(XINPUT, 1).macro_started_by("rp"), Some(0));
        assert!(!s.notice.unwrap().body.contains("no longer starts"));
    }

    #[test]
    fn macro_refs_in_the_file_are_skipped_and_named_as_files_8b_cannot_read() {
        let mut s = connected(DINPUT);
        let text = file(&s, DINPUT, 1, |p| {
            p.macro_refs = vec![MacroRef {
                trigger_buttons: vec!["right face".to_owned()],
                path: "m.json".to_owned(),
            }];
        });
        s.import((DINPUT, 3), "m.json", Ok(text));
        let edited = s.slot(DINPUT, 3).edited.unwrap();
        assert!(edited.macro_refs.is_empty(), "an empty slot holds no macros");
        let body = s.notice.unwrap().body;
        assert!(body.starts_with("The file’s macro sits in a separate file"), "{body}");
        assert!(body.contains("upload --with-macros"), "{body}");
    }

    #[test]
    fn a_file_for_another_mode_waits_for_a_yes() {
        let mut s = connected(XINPUT);
        let text = switch_file(&s, |p| {
            remap(p, "l1", UNRECOGNISED_OUTPUT);
        });
        s.import((XINPUT, 2), "switch.json", Ok(text));
        assert_eq!(s.slot(XINPUT, 2).edited, None, "nothing loads before the yes");
        let p = s.pending_import.clone().unwrap();
        assert_eq!((p.from, p.slot), (SWITCH, (XINPUT, 2)));
        let reset = ConversionLoss::SettingsReset { group: "triggers".to_owned() };
        assert!(p.losses.contains(&reset));

        s.cancel_import();
        assert_eq!((s.pending_import.as_ref(), s.slot(XINPUT, 2).edited), (None, None));

        s.pending_import = Some(p);
        s.confirm_import();
        let edited = s.slot(XINPUT, 2).edited.unwrap();
        assert_eq!(edited.mode, XINPUT);
        assert!(edited.button_mappings.iter().any(|m| m.source == "l1" && m.target == "disabled"));
    }

    #[test]
    fn a_macro_file_is_named_as_one_and_points_to_the_right_import() {
        let mut s = connected(XINPUT);
        let def = crate::state::tests::macro_def(0, "Jump", &["rp"], 2);
        let text = crate::macro_files::macro_text(&def, "8bitdo-pro3").unwrap();
        s.import((XINPUT, 1), "xinput-slot1-macro0-Jump.json", Ok(text));
        let n = s.notice.take().unwrap();
        assert_eq!(
            n.body,
            "This is a macro file, not a profile. Import it with “Import macro…” on the Macros tab."
        );
        assert_eq!(s.pending_import, None);
    }

    #[test]
    fn a_bad_file_leaves_the_slot_and_says_why() {
        let mut s = connected(XINPUT);
        let before = s.slot(XINPUT, 1);
        for text in [
            Ok("not json".to_owned()),
            Ok("{\"id\": 3}".to_owned()),
            Err("No such file.".to_owned()),
        ] {
            s.import((XINPUT, 1), "bad.json", text);
            let n = s.notice.take().unwrap();
            assert!(n.error && n.title == "Could not import “bad.json”.", "{n:?}");
            assert_ne!(n.body, "");
        }
        assert_eq!(s.slot(XINPUT, 1), before);
        assert_eq!(s.pending_import, None);
    }

    #[test]
    fn export_message_says_when_edits_are_unsaved() {
        let mut s = connected(SWITCH);
        s.exported((SWITCH, 2), true, Ok("x.json".to_owned()));
        let n = s.notice.take().unwrap();
        assert_eq!(n.title, "Saved Switch slot 2 to “x.json”.");
        assert!(n.body.contains("not written to the controller yet"));
        s.exported((SWITCH, 2), false, Ok("x.json".to_owned()));
        assert_eq!(s.notice.take().unwrap().body, "");
        s.exported((SWITCH, 2), false, Err("Permission denied.".to_owned()));
        assert!(s.notice.unwrap().error);
    }

    #[test]
    fn the_warning_lists_losses_and_swapped_letters() {
        let mut s = connected(XINPUT);
        let text = switch_file(&s, |p| {
            remap(p, "r1", UNRECOGNISED_OUTPUT);
        });
        s.import((XINPUT, 3), "s.json", Ok(text));
        let w = import_warning(&s);
        assert!(w.shown);
        assert_eq!(w.reset, "Left trigger, Right trigger and Swaps");
        assert_eq!((w.from_mode.as_str(), w.to_mode.as_str()), ("Switch", "XInput"));
        let lost: Vec<Change> = w.lost.iter().collect();
        assert_eq!(lost.len(), 1);
        assert_eq!(
            (lost[0].what.as_str(), lost[0].from.as_str(), lost[0].to.as_str()),
            ("R", "Unknown", "Disabled")
        );
        assert_eq!(
            w.letters,
            "Switch A is XInput B, Switch B is XInput A, Switch X is XInput Y and Switch Y is XInput X"
        );
        let d = s.description();
        assert_eq!(swapped_letters(d, XINPUT, DINPUT), "");
        s.cancel_import();
        assert!(!import_warning(&s).shown);
    }
}
