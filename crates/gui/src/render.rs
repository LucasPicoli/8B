//! Pushes [`AppState`] into the window. Every model fact (modes, slot counts) comes
//! from the controller description, so nothing here names a controller model.

use std::rc::Rc;

use controller_core::transport::udev::{
    keepalive_command, manual_command, KEEPALIVE_RULE_PATH, KEEPALIVE_UNIT_PATH, UDEV_RULE_PATH,
};
use slint::{ComponentHandle as _, ModelRc, SharedString, VecModel};

use crate::buttons::{render_buttons, render_views};
use crate::closing::render_close;
use crate::files::render_files;
use crate::review::render_writes;
use crate::settings::render_settings;
use crate::state::{Access, AppState, Install, Rule};
use crate::ui::{AppWindow, ModeGroup, Slot};

/// The sidebar: every slot of every mode, in the description's order.
#[must_use]
pub fn groups(state: &AppState) -> Vec<ModeGroup> {
    let description = state.description();
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
                name: mode.label.as_str().into(),
                current: state.current_mode() == Some(mode.id),
                slots: ModelRc::from(Rc::new(slots.collect::<VecModel<_>>())),
            }
        })
        .collect()
}

/// The line under the controller name in the sidebar.
#[must_use]
pub fn device_status(state: &AppState) -> String {
    let failed = state.shown().is_some_and(|c| c.read_error.is_some());
    match state.current_mode() {
        None => "Not connected".to_owned(),
        Some(mode) if state.reading() => {
            format!("Reading over USB · {}", state.description().mode_label(mode))
        }
        Some(mode) if failed => {
            format!("Could not read · {}", state.description().mode_label(mode))
        }
        Some(mode) => format!("Connected over USB · {}", state.description().mode_label(mode)),
    }
}

/// A list model of strings.
fn strings(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(items.into_iter().map(SharedString::from).collect::<VecModel<_>>()))
}

/// Each unplugged controller whose edits the new one may take, with how many slots
/// it edited.
#[must_use]
pub fn move_from(state: &AppState) -> Vec<String> {
    let labels = state.controller_labels();
    let listed: Vec<_> = state.listed().collect();
    state
        .move_candidates()
        .into_iter()
        .filter_map(|i| {
            let edited = listed.get(i)?.slots.values().filter(|s| s.unsaved()).count();
            let slots = if edited == 1 {
                "edits in 1 slot".to_owned()
            } else {
                format!("edits in {edited} slots")
            };
            Some(format!("{} · {slots}", labels.get(i)?))
        })
        .collect()
}

/// Each slot of the shown controller that changed under its edits, such as
/// `XInput slot 1 · Racing`, with the name of the edits.
#[must_use]
pub fn changed_slots(state: &AppState) -> Vec<String> {
    state
        .changed_slots()
        .into_iter()
        .map(|(mode, n)| {
            let name = state.slot(mode, n).shown().map(|p| p.name.clone()).unwrap_or_default();
            format!("{} slot {n} · {name}", state.description().mode_label(mode))
        })
        .collect()
}

/// The title of the selected slot, such as `XInput slot 2`.
#[must_use]
pub fn slot_title(state: &AppState) -> String {
    state.selected_slot().map_or_else(String::new, |(mode, n)| {
        format!("{} slot {n}", state.description().mode_label(mode))
    })
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
    let description = state.description();
    if state.views_of.get().is_none_or(|drawn| !std::ptr::eq(drawn, description)) {
        render_views(description, ui);
        state.views_of.set(Some(description));
    }
    ui.set_has_controller(state.has_controller());
    ui.set_device_name(state.description().short_name.as_str().into());
    ui.set_controllers(strings(state.controller_labels()));
    ui.set_current_controller(i32::try_from(state.active_index()).unwrap_or(0));
    ui.set_move_from(strings(move_from(state)));
    ui.set_changed_slots(strings(changed_slots(state)));
    ui.set_changed_keep(ModelRc::from(Rc::new(VecModel::from(state.changed_choices()))));
    ui.set_device_status(device_status(state).into());
    ui.set_connected(state.current_mode().is_some());
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
    ui.set_name_max(state.description().limits.profile_name_length.max);
    ui.set_unsaved(selected.is_some_and(|s| s.unsaved()));
    render_buttons(state, ui);
    render_settings(state, ui);
    render_files(state, ui);
    render_writes(state, ui);
    render_close(state, ui);
    ui.set_worker_stopped(state.worker_stopped);
    ui.set_read_error(state.read_error().map(sentence).unwrap_or_default().into());
    ui.set_read_error_elsewhere(state.failed_elsewhere());
    ui.set_read_error_title(
        if state.failed_elsewhere() {
            format!(
                "Could not read the {} plugged into another USB port.",
                state.description().short_name
            )
        } else {
            "Could not read the controller.".to_owned()
        }
        .into(),
    );
    ui.set_read_failed(state.shown().is_some_and(|c| c.read_error.is_some()));
    let retry = state.retry_port();
    ui.set_can_read_again(
        retry.is_some_and(|p| state.controllers.iter().any(|c| c.port == p && !c.reading)),
    );
    ui.set_asks_for_rule(state.asks_for_rule());
    ui.set_rule_denied(state.access.is_some());
    ui.set_rule_checking(state.access.is_some() && state.reading_any());
    ui.set_rule_still_blocked(state.still_blocked());
    ui.set_rule_installed(state.access == Some(Access::StillDenied));
    // Not denied: the controller works without the rule, so offer an update and a skip.
    ui.set_rule_outdated(state.access.is_none() && state.rule == Rule::Outdated);
    ui.set_rule_skippable(state.access.is_none());
    ui.set_sandboxed(state.sandboxed);
    ui.set_installing(state.install == Install::Running);
    ui.set_install_error(match &state.install {
        Install::Failed(e) => e.as_str().into(),
        Install::Idle | Install::Running => SharedString::new(),
    });
    ui.set_udev_command(manual_command().into());
    ui.set_udev_rule_path(UDEV_RULE_PATH.into());
    render_fix(state, ui);
}

/// Pushes the keepalive fix offer: the bar above the tabs and its dialog.
fn render_fix(state: &AppState, ui: &AppWindow) {
    let shown = state.fix_shown();
    ui.set_fix_offered(shown);
    ui.set_fix_open(shown && state.fix_open);
    ui.set_fix_installing(state.fix_install == Install::Running);
    ui.set_fix_error(match &state.fix_install {
        Install::Failed(e) => e.as_str().into(),
        Install::Idle | Install::Running => SharedString::new(),
    });
    ui.set_fix_command(keepalive_command().into());
    ui.set_fix_rule_path(KEEPALIVE_RULE_PATH.into());
    ui.set_fix_unit_path(KEEPALIVE_UNIT_PATH.into());
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
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use controller_core::model::Mode;
    use slint::Model as _;

    use super::*;
    use crate::state::tests::{full_read, new_state, PORT};

    fn connected(mode: Mode) -> AppState {
        let mut s = new_state();
        s.presence(PORT, Some(mode));
        s.read_finished(PORT, Ok(full_read()));
        s
    }

    #[test]
    fn sidebar_lists_the_description_modes_and_slots() {
        let s = connected(SWITCH);
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
        let mut s = connected(XINPUT);
        let mut edited = s.slot(XINPUT, 2).pad.unwrap();
        edited.name = "Edited".to_owned();
        s.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().edited = Some(edited);
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
        s.presence(PORT, Some(DINPUT));
        s.read_started(PORT);
        assert_eq!(device_status(&s), "Reading over USB · DInput");
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(device_status(&s), "Connected over USB · DInput");
        s.read_finished(PORT, Err("device communication timed out".to_owned()));
        assert_eq!(device_status(&s), "Could not read · DInput");
        s.read_started(PORT);
        assert_eq!(device_status(&s), "Reading over USB · DInput");
        s.read_finished(PORT, Ok(full_read()));
        assert_eq!(slot_title(&s), "DInput slot 1");
        assert!(!selected_empty(&s));
        s.select(2, 2);
        assert!(selected_empty(&s));
        s.presence(PORT, None);
        assert_eq!(device_status(&s), "Not connected");
    }
}
