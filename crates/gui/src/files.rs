//! Import and export of profile JSON. An import loads a file into a slot's edits,
//! converting a file made for another mode after a warning. An export saves the
//! profile on screen in the CLI's `export` shape. Pure: the dialogs live in
//! `chooser`.

use std::path::Path;
use std::rc::Rc;

use controller_core::convert::{convert_profile, ConversionLoss};
use controller_core::description::{ControllerDescription, DISABLED_OUTPUT};
use controller_core::device::Model;
use controller_core::model::{CanonicalProfile, Mode};
use controller_core::service::validation::validate_profile;
use serde_json::Value;
use slint::{ModelRc, VecModel};

use crate::buttons::output_label;
use crate::render::sentence;
use crate::state::{AppState, Notice};
use crate::ui::{AppWindow, Change, ImportWarning};

/// A schema message longer than this, in characters, is cut: some quote the whole
/// offending value.
const REASON_MAX: usize = 160;

/// A file made for another mode, converted and waiting for the user's yes.
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
    /// How many macro references the file carried.
    pub skipped_macros: usize,
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

/// The profile in `text`, checked as a profile of `model`.
fn parse(model: Option<&dyn Model>, text: &str) -> Result<CanonicalProfile, String> {
    let value: Value =
        serde_json::from_str(text).map_err(|e| sentence(&format!("it is not JSON: {e}")))?;
    let model = model.ok_or_else(|| sentence("this controller model cannot check files"))?;
    let check = validate_profile(model, &value).map_err(|e| sentence(&e.to_string()))?;
    if let Some(first) = check.errors.first() {
        let more = match check.errors.len() - 1 {
            0 => String::new(),
            n => format!(" ({n} more problems)"),
        };
        let mut reason: String = first.reason.chars().take(REASON_MAX).collect();
        if reason.len() < first.reason.len() {
            reason.push('…');
        }
        return Err(format!("It is not a valid profile. {}: {reason}{more}", first.path));
    }
    serde_json::from_value(value).map_err(|e| sentence(&e.to_string()))
}

impl AppState {
    /// Saved slot `slot`'s profile to a file, or failed to.
    pub fn exported(&mut self, slot: (Mode, u8), unsaved: bool, result: Result<String, String>) {
        let title = format!("{} slot {}", slot.0.label(), slot.1);
        self.notice = Some(match result {
            Ok(name) => Notice {
                error: false,
                title: format!("Saved {title} to “{name}”."),
                body: if unsaved {
                    "The file includes edits not yet written to the controller.".to_owned()
                } else {
                    String::new()
                },
            },
            Err(e) => Notice { error: true, title: format!("Could not save {title}."), body: e },
        });
    }

    /// Loads file `file_name`, read as `text` (or not), into slot `slot`'s edits. A
    /// file for another mode waits in `pending_import` for the user's yes.
    pub fn import(&mut self, slot: (Mode, u8), file_name: &str, text: Result<String, String>) {
        let parsed = text.and_then(|t| parse(self.model(), &t));
        let converted = parsed.and_then(|p| {
            let default = |m| self.defaults().get(&m).ok_or("This controller has no such mode.");
            let (profile, losses) =
                convert_profile(self.description(), &p, default(p.mode)?, default(slot.0)?)
                    .map_err(|e| sentence(&e.to_string()))?;
            Ok((p.mode, p.macro_refs.len(), profile, losses))
        });
        match converted {
            Err(body) => {
                self.notice = Some(Notice {
                    error: true,
                    title: format!("Could not import “{file_name}”."),
                    body,
                });
            }
            Ok((from, skipped_macros, profile, losses)) => {
                let pending = PendingImport {
                    slot,
                    file_name: file_name.to_owned(),
                    from,
                    profile,
                    losses,
                    skipped_macros,
                };
                if from == slot.0 {
                    self.apply_import(pending);
                } else {
                    self.pending_import = Some(pending);
                }
            }
        }
    }

    /// The user said yes to the waiting import.
    pub fn confirm_import(&mut self) {
        if let Some(pending) = self.pending_import.take() {
            self.apply_import(pending);
        }
    }

    /// The user said no to the waiting import.
    pub fn cancel_import(&mut self) {
        self.pending_import = None;
    }

    /// Puts an import into its slot's edits and shows that slot. The slot keeps its
    /// own id and the macros stored on the controller; the file's are skipped.
    fn apply_import(&mut self, pending: PendingImport) {
        let PendingImport { slot: (mode, number), file_name, mut profile, skipped_macros, .. } =
            pending;
        let active = self.active().map(|c| c.port.clone());
        let default = self.defaults().get(&mode).cloned();
        let Some(c) = self.controllers.iter_mut().find(|c| Some(&c.port) == active.as_ref()) else {
            return;
        };
        let state = c.slots.entry((mode, number)).or_default();
        let base = state.pad.as_ref().or(default.as_ref());
        profile.id = base.map(|b| b.id.clone()).unwrap_or_default();
        profile.preferred_slot = base.and_then(|b| b.preferred_slot);
        profile.macro_refs = state.pad.as_ref().map(|p| p.macro_refs.clone()).unwrap_or_default();
        state.edited = Some(profile);
        if let Some(i) = self.description().modes.iter().position(|m| m.id == mode) {
            self.select(i, usize::from(number).saturating_sub(1));
        }
        let skipped = match skipped_macros {
            0 => String::new(),
            1 => "The file’s macro reference was skipped. The macros stored on the controller stay. ".to_owned(),
            n => format!("The file’s {n} macro references were skipped. The macros stored on the controller stay. "),
        };
        self.notice = Some(Notice {
            error: false,
            title: format!("Imported “{file_name}” into {} slot {number}.", mode.label()),
            body: skipped + "Nothing reaches the controller until you write.",
        });
    }
}

/// Buttons whose letter in `to` names another button in `from`, as
/// `XInput B is Switch A`. A label that only changes form, such as LB to L, is
/// left out.
#[must_use]
pub fn swapped_letters(description: &ControllerDescription, from: Mode, to: Mode) -> String {
    let label = |b: &controller_core::description::Button, m| b.labels.get(&m).cloned();
    description
        .buttons
        .iter()
        .filter_map(|b| Some((label(b, from)?, label(b, to)?)))
        .filter(|(f, t)| {
            f != t && description.buttons.iter().any(|o| label(o, from).as_ref() == Some(t))
        })
        .map(|(f, t)| format!("{} {f} is {} {t}", from.label(), to.label()))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The warning before a cross-mode import, empty when none waits.
#[must_use]
pub fn import_warning(state: &AppState) -> ImportWarning {
    let Some(p) = &state.pending_import else { return ImportWarning::default() };
    let d = state.description();
    let (from, to) = (p.from, p.slot.0);
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
        slot_title: format!("{} slot {}", to.label(), p.slot.1).into(),
        file_name: p.file_name.as_str().into(),
        from_mode: from.label().into(),
        to_mode: to.label().into(),
        lost: ModelRc::from(Rc::new(VecModel::from(lost))),
        reset: reset_groups(&p.losses).into(),
        letters: swapped_letters(d, from, to).into(),
        skipped_macros: i32::try_from(p.skipped_macros).unwrap_or(i32::MAX),
    }
}

/// The settings groups a conversion reset, such as `triggers`, joined for a sentence.
fn reset_groups(losses: &[ConversionLoss]) -> String {
    let groups: Vec<&str> = losses
        .iter()
        .filter_map(|l| match l {
            ConversionLoss::SettingsReset { group } => Some(group.as_str()),
            ConversionLoss::OutputDisabled { .. } => None,
        })
        .collect();
    groups.join(" and ")
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
    use controller_core::model::MacroRef;
    use slint::Model as _;

    use super::*;
    use crate::state::tests::connected;

    /// Gives `source` the output `target`.
    fn remap(p: &mut CanonicalProfile, source: &str, target: &str) {
        let m = p.button_mappings.iter_mut().find(|m| m.source == source).unwrap();
        target.clone_into(&mut m.target);
    }

    /// A Switch default profile as an export file holds it, after `change`.
    fn switch_file(s: &AppState, change: impl FnOnce(&mut CanonicalProfile)) -> String {
        let mut p = s.defaults()[&Mode::Switch].clone();
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
        let s = connected(Mode::XInput);
        assert_eq!(file_name(Mode::Switch, 3), "profile-switch-slot-3-index-2.json");
        let text = file(&s, Mode::XInput, 1, |_| {});
        assert!(text.ends_with("}\n"));
        let back: Value = serde_json::from_str(&text).unwrap();
        assert!(validate_profile(s.model().unwrap(), &back).unwrap().valid);
    }

    #[test]
    fn a_same_mode_file_round_trips_into_the_edits() {
        let mut s = connected(Mode::XInput);
        let text = file(&s, Mode::XInput, 1, |p| {
            p.name = "From file".to_owned();
            p.id = "xinput-slot-1-index-0".to_owned();
            remap(p, "r1", "disabled");
        });
        s.import((Mode::XInput, 2), "a.json", Ok(text));
        assert_eq!(s.pending_import, None);
        assert_eq!(s.selected_slot(), Some((Mode::XInput, 2)), "the import shows its slot");
        let slot = s.slot(Mode::XInput, 2);
        let edited = slot.edited.as_ref().unwrap();
        assert_eq!(edited.name, "From file");
        assert_eq!(edited.id, slot.pad.as_ref().unwrap().id, "the slot keeps its own id");
        assert_eq!(edited.macro_refs, slot.pad.as_ref().unwrap().macro_refs, "macros stay");
        assert!(slot.unsaved());
        assert!(s.notice.as_ref().is_some_and(|n| !n.error));

        // Export of the edits, imported back unchanged, leaves the edits as they were.
        let again = export_text(edited).unwrap();
        s.import((Mode::XInput, 2), "b.json", Ok(again));
        assert_eq!(s.slot(Mode::XInput, 2), slot);
    }

    #[test]
    fn a_file_with_the_dpad_swap_and_an_inverted_left_stick_is_refused() {
        let mut s = connected(Mode::XInput);
        let text = file(&s, Mode::XInput, 1, |p| {
            p.set_setting("/sticks/invert_left_x", true.into());
            p.set_setting("/sticks/swap_dpad_with_left_stick", true.into());
        });
        s.import((Mode::XInput, 2), "clash.json", Ok(text));
        let notice = s.notice.clone().unwrap();
        assert!(notice.error);
        assert!(notice.body.contains("swap_dpad_with_left_stick cannot be on"), "{}", notice.body);
        assert!(!s.slot(Mode::XInput, 2).unsaved());
    }

    #[test]
    fn macro_refs_in_the_file_are_skipped_with_a_note() {
        let mut s = connected(Mode::DInput);
        let text = file(&s, Mode::DInput, 1, |p| {
            p.macro_refs =
                vec![MacroRef { trigger: "right face".to_owned(), path: "m.json".to_owned() }];
        });
        s.import((Mode::DInput, 3), "m.json", Ok(text));
        let edited = s.slot(Mode::DInput, 3).edited.unwrap();
        assert!(edited.macro_refs.is_empty(), "an empty slot holds no macros");
        assert!(s.notice.unwrap().body.starts_with("The file’s macro reference was skipped."));
    }

    #[test]
    fn a_file_for_another_mode_waits_for_a_yes() {
        let mut s = connected(Mode::XInput);
        let text = switch_file(&s, |p| {
            remap(p, "l1", UNRECOGNISED_OUTPUT);
        });
        s.import((Mode::XInput, 2), "switch.json", Ok(text));
        assert_eq!(s.slot(Mode::XInput, 2).edited, None, "nothing loads before the yes");
        let p = s.pending_import.clone().unwrap();
        assert_eq!((p.from, p.slot), (Mode::Switch, (Mode::XInput, 2)));
        let reset = ConversionLoss::SettingsReset { group: "triggers".to_owned() };
        assert!(p.losses.contains(&reset));

        s.cancel_import();
        assert_eq!((s.pending_import.as_ref(), s.slot(Mode::XInput, 2).edited), (None, None));

        s.pending_import = Some(p);
        s.confirm_import();
        let edited = s.slot(Mode::XInput, 2).edited.unwrap();
        assert_eq!(edited.mode, Mode::XInput);
        assert!(edited.button_mappings.iter().any(|m| m.source == "l1" && m.target == "disabled"));
    }

    #[test]
    fn a_bad_file_leaves_the_slot_and_says_why() {
        let mut s = connected(Mode::XInput);
        let before = s.slot(Mode::XInput, 1);
        for text in [
            Ok("not json".to_owned()),
            Ok("{\"id\": 3}".to_owned()),
            Err("No such file.".to_owned()),
        ] {
            s.import((Mode::XInput, 1), "bad.json", text);
            let n = s.notice.take().unwrap();
            assert!(n.error && n.title == "Could not import “bad.json”.", "{n:?}");
            assert_ne!(n.body, "");
        }
        assert_eq!(s.slot(Mode::XInput, 1), before);
        assert_eq!(s.pending_import, None);
    }

    #[test]
    fn export_message_says_when_edits_are_unsaved() {
        let mut s = connected(Mode::Switch);
        s.exported((Mode::Switch, 2), true, Ok("x.json".to_owned()));
        let n = s.notice.take().unwrap();
        assert_eq!(n.title, "Saved Switch slot 2 to “x.json”.");
        assert!(n.body.contains("not yet written"));
        s.exported((Mode::Switch, 2), false, Ok("x.json".to_owned()));
        assert_eq!(s.notice.take().unwrap().body, "");
        s.exported((Mode::Switch, 2), false, Err("Permission denied.".to_owned()));
        assert!(s.notice.unwrap().error);
    }

    #[test]
    fn the_warning_lists_losses_and_swapped_letters() {
        let mut s = connected(Mode::XInput);
        let text = switch_file(&s, |p| {
            remap(p, "r1", UNRECOGNISED_OUTPUT);
        });
        s.import((Mode::XInput, 3), "s.json", Ok(text));
        let w = import_warning(&s);
        assert!(w.shown && w.reset == "triggers");
        assert_eq!((w.from_mode.as_str(), w.to_mode.as_str()), ("Switch", "XInput"));
        let lost: Vec<Change> = w.lost.iter().collect();
        assert_eq!(lost.len(), 1);
        assert_eq!(
            (lost[0].what.as_str(), lost[0].from.as_str(), lost[0].to.as_str()),
            ("R", "Unknown", "Disabled")
        );
        assert_eq!(
            w.letters,
            "Switch A is XInput B, Switch B is XInput A, Switch X is XInput Y, Switch Y is XInput X"
        );
        let d = s.description();
        assert_eq!(swapped_letters(d, Mode::XInput, Mode::DInput), "");
        s.cancel_import();
        assert!(!import_warning(&s).shown);
    }
}
