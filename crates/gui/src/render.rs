//! Pushes [`AppState`] into the window. Every model fact (modes, slot counts) comes
//! from the controller description, so nothing here names a controller model.

use std::rc::Rc;

use controller_core::transport::udev::{manual_command, KEEPALIVE_UNIT_PATH, UDEV_RULE_PATH};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};

use crate::buttons::render_buttons;
use crate::files::render_files;
use crate::settings::render_settings;
use crate::state::{Access, AppState, Install, Rule};
use crate::ui::{AppWindow, ModeGroup, Slot};

/// The sidebar: every slot of every mode, in the description's order.
#[must_use]
pub fn groups(state: &AppState) -> Vec<ModeGroup> {
    let description = state.description;
    (0_i32..)
        .zip(&description.modes)
        .map(|(gi, mode)| {
            let slots = (1..=description.slot_count).map(|number| {
                let slot = state.slot(mode.id, number);
                let shown = slot.edited.as_ref().or(slot.pad.as_ref());
                Slot {
                    mode: gi,
                    number: i32::from(number),
                    name: shown.map_or_else(SharedString::new, |p| p.name.as_str().into()),
                    empty: shown.is_none(),
                    macros: shown
                        .map_or(0, |p| i32::try_from(p.macro_refs.len()).unwrap_or(i32::MAX)),
                    unsaved: slot.unsaved(),
                }
            });
            ModeGroup {
                name: mode.id.label().into(),
                current: state.current_mode == Some(mode.id),
                slots: ModelRc::from(Rc::new(slots.collect::<VecModel<_>>())),
            }
        })
        .collect()
}

/// The line under the controller name in the sidebar.
#[must_use]
pub fn device_status(state: &AppState) -> String {
    match state.current_mode {
        None => "Not connected".to_owned(),
        Some(mode) if state.reading => format!("Reading over USB · {}", mode.label()),
        Some(mode) => format!("Connected over USB · {}", mode.label()),
    }
}

/// The title of the selected slot, such as `XInput slot 2`.
#[must_use]
pub fn slot_title(state: &AppState) -> String {
    state.selected_slot().map_or_else(String::new, |(mode, n)| format!("{} slot {n}", mode.label()))
}

/// Whether the selected slot holds nothing, on the controller or in the edits.
#[must_use]
pub fn selected_empty(state: &AppState) -> bool {
    state.selected_slot().is_none_or(|(mode, n)| {
        let slot = state.slot(mode, n);
        slot.pad.is_none() && slot.edited.is_none()
    })
}

/// Makes an error message a sentence: a capital first letter and a full stop.
#[must_use]
pub fn sentence(message: &str) -> String {
    let mut chars = message.chars();
    let mut out: String =
        chars.next().map(|c| c.to_uppercase().chain(chars).collect()).unwrap_or_default();
    if !out.is_empty() && !out.ends_with('.') {
        out.push('.');
    }
    out
}

/// Pushes the whole visible screen.
pub fn render(state: &AppState, ui: &AppWindow) {
    ui.set_has_controller(state.has_controller());
    ui.set_device_name(state.description.display_name.as_str().into());
    ui.set_device_status(device_status(state).into());
    ui.set_connected(state.current_mode.is_some());
    ui.set_groups(ModelRc::from(Rc::new(VecModel::from(groups(state)))));
    ui.set_selected_mode(i32::try_from(state.selected.0).unwrap_or(0));
    ui.set_selected_slot(i32::try_from(state.selected.1).unwrap_or(0));
    ui.set_slot_title(slot_title(state).into());
    ui.set_empty_slot(selected_empty(state));
    let selected = state.selected_slot().map(|(m, n)| state.slot(m, n));
    let name =
        selected.as_ref().and_then(|s| s.shown()).map(|p| p.name.as_str()).unwrap_or_default();
    if ui.get_profile_name() != name {
        ui.set_profile_name(name.into());
    }
    ui.set_name_max(state.description.limits.profile_name_length.max);
    ui.set_unsaved(selected.is_some_and(|s| s.unsaved()));
    render_buttons(state, ui);
    render_settings(state, ui);
    render_files(state, ui);
    ui.set_read_error(state.read_error.as_deref().map(sentence).unwrap_or_default().into());
    ui.set_asks_for_rule(state.asks_for_rule());
    ui.set_rule_installed(state.access == Some(Access::StillDenied));
    // Not denied: the controller works without the rule, so offer an update and a skip.
    ui.set_rule_outdated(state.access.is_none() && state.rule == Rule::Outdated);
    ui.set_rule_skippable(state.access.is_none());
    ui.set_installing(state.install == Install::Running);
    ui.set_install_error(match &state.install {
        Install::Failed(e) => e.as_str().into(),
        Install::Idle | Install::Running => SharedString::new(),
    });
    ui.set_udev_command(manual_command().into());
    ui.set_udev_rule_path(UDEV_RULE_PATH.into());
    ui.set_keepalive_unit_path(KEEPALIVE_UNIT_PATH.into());
}

/// Narrower than this, in logical pixels, toolbar buttons show only their icon.
const COMPACT_BELOW: f32 = 1200.0;

/// Sets the toolbar compact or not from the window's current width.
pub fn fit_toolbar(ui: &AppWindow) {
    let window = ui.window();
    ui.set_compact(window.size().to_logical(window.scale_factor()).width < COMPACT_BELOW);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::model::Mode;
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{full_read, new_state};

    fn connected(mode: Mode) -> AppState {
        let mut s = new_state();
        s.presence(Some(mode));
        s.read_finished(Ok(full_read()));
        s
    }

    #[test]
    fn sidebar_lists_the_description_modes_and_slots() {
        let s = connected(Mode::Switch);
        let g = groups(&s);
        let names: Vec<_> = g.iter().map(|g| g.name.to_string()).collect();
        assert_eq!(names, ["XInput", "Switch", "DInput"]);
        assert_eq!(g.iter().map(|g| g.current).collect::<Vec<_>>(), [false, true, false]);
        let dinput: Vec<Slot> = g[2].slots.iter().collect();
        assert_eq!(dinput.len(), 3);
        assert_eq!((dinput[0].name.as_str(), dinput[0].empty), ("DInput", false));
        assert_eq!((dinput[1].macros, dinput[1].mode, dinput[1].number), (2, 2, 2));
        assert!(dinput[2].empty && dinput[2].name.is_empty());
        assert!(!dinput.iter().any(|s| s.unsaved));
    }

    #[test]
    fn sidebar_shows_the_edited_copy() {
        let mut s = connected(Mode::XInput);
        let mut edited = s.slot(Mode::XInput, 2).pad.unwrap();
        edited.name = "Edited".to_owned();
        s.slots.get_mut(&(Mode::XInput, 2)).unwrap().edited = Some(edited);
        let slot = groups(&s)[0].slots.row_data(1).unwrap();
        assert_eq!(slot.name, "Edited");
        assert!(slot.unsaved);
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(sentence("device communication timed out"), "Device communication timed out.");
        assert_eq!(sentence("Done."), "Done.");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn status_title_and_empty_slot() {
        let mut s = new_state();
        assert_eq!(device_status(&s), "Not connected");
        s.presence(Some(Mode::DInput));
        s.read_started();
        assert_eq!(device_status(&s), "Reading over USB · DInput");
        s.read_finished(Ok(full_read()));
        assert_eq!(device_status(&s), "Connected over USB · DInput");
        assert_eq!(slot_title(&s), "DInput slot 1");
        assert!(!selected_empty(&s));
        s.select(2, 2);
        assert!(selected_empty(&s));
        s.presence(None);
        assert_eq!(device_status(&s), "Not connected");
    }
}
