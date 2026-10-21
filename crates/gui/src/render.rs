//! Pushes [`AppState`] into the window. Every model fact (modes, slot counts) comes
//! from the controller description, so nothing here names a controller model.

use std::fmt::Display;
use std::rc::Rc;

use controller_core::description::ControllerDescription;
use controller_core::model::Mode;

use controller_core::transport::udev::{
    keepalive_command, manual_command, KEEPALIVE_RULE_PATH, KEEPALIVE_UNIT_PATH, UDEV_RULE_PATH,
};
use slint::{ComponentHandle as _, Model as _, ModelRc, SharedString, VecModel};
use tr::tr;

use crate::buttons::{keep_same, push_model, render_buttons, render_views};
use crate::closing::render_close;
use crate::files::render_files;
use crate::locked::render_locked;
use crate::macro_check::render_blocks;
use crate::macro_files::render_macro_files;
use crate::macro_tab::render_macros;
use crate::review::render_writes;
use crate::settings::render_settings;
use crate::state::{Access, AppState, Install, Rule};
use crate::step_panel::render_panel;
use crate::step_table::render_steps;
use crate::stick_pad::render_sticks;
use crate::trigger_pad::render_triggers;
use crate::ui::{AccessibleNumber, AppWindow, ModeGroup, Slot};

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
                    name: shown
                        .map_or_else(SharedString::new, |p| shown_name(&p.name, number).into()),
                    empty: shown.is_none(),
                    // Every macro shown, so one that no button starts counts too.
                    macros: i32::try_from(shown.map_or(0, |_| slot.shown_macros().len()))
                        .unwrap_or(i32::MAX),
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

/// The line under the controller name in the sidebar. A connected controller that
/// reports its firmware shows it in place of "over USB", the only link 8B supports, so
/// the line fits the sidebar.
#[must_use]
pub fn device_status(state: &AppState) -> String {
    if let Some(status) = state.lock_status() {
        return status;
    }
    let failed = state.shown().is_some_and(|c| c.read_error.is_some());
    let Some(mode) = state.current_mode() else { return tr!("Not connected") };
    let mode = state.description().mode_label(mode);
    match state.firmware() {
        _ if state.reading() => tr!("Reading over USB · {mode}", mode = mode),
        _ if failed => tr!("Could not read · {mode}", mode = mode),
        "" => tr!("Connected over USB · {mode}", mode = mode),
        version => tr!("Connected · {mode} · {version}", mode = mode, version = version),
    }
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
            let controller = labels.get(i)?;
            Some(tr!(
                "{controller} · edits in {n} slot" | "{controller} · edits in {n} slots" % edited,
                controller = controller
            ))
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
            let name =
                state.slot(mode, n).shown().map(|p| shown_name(&p.name, n)).unwrap_or_default();
            tr!("{slot} · {name}", slot = slot_name(state.description(), mode, n), name = name)
        })
        .collect()
}

/// A profile's name as shown: `name`, or "Profile 2" for slot 2 when it has none.
#[must_use]
pub fn shown_name(name: &str, number: u8) -> String {
    if name.trim().is_empty() {
        tr!("Profile {n}", n = number)
    } else {
        name.to_owned()
    }
}

/// The name in the header of the selected slot: the text of its name field, or the name
/// as shown when the model cannot rename a profile. Empty for an empty slot.
#[must_use]
pub fn profile_name(state: &AppState) -> String {
    let Some((mode, n)) = state.selected_slot() else { return String::new() };
    let Some(profile) = state.slot(mode, n).shown().cloned() else { return String::new() };
    if state.description().profile_rename {
        profile.name
    } else {
        shown_name(&profile.name, n)
    }
}

/// The title of the selected slot, such as `XInput slot 2`.
#[must_use]
pub fn slot_title(state: &AppState) -> String {
    state
        .selected_slot()
        .map_or_else(String::new, |(mode, n)| slot_name(state.description(), mode, n))
}

/// A slot's name in a sentence, such as `XInput slot 2`.
#[must_use]
pub fn slot_name(d: &ControllerDescription, mode: Mode, n: impl Display) -> String {
    tr!("{mode} slot {n}", mode = d.mode_label(mode), n = n)
}

/// Whether the selected slot holds nothing, on the controller or in the edits.
#[must_use]
pub fn selected_empty(state: &AppState) -> bool {
    state.selected_slot().is_none_or(|(mode, n)| {
        let slot = state.slot(mode, n);
        slot.pad.is_none() && slot.edited.is_none()
    })
}

/// `items` joined for a sentence: "A", "A and B", "A, B and C".
#[must_use]
pub fn and_list(items: &[String]) -> String {
    let Some((last, rest)) = items.split_last() else { return String::new() };
    let Some((first, middle)) = rest.split_first() else { return last.clone() };
    let head = middle.iter().fold(first.clone(), |a, b| tr!("{a}, {b}", a = a, b = b));
    tr!("{a} and {b}", a = head, b = last)
}

/// Lets the window's sliders read the number a screen reader sets.
pub fn read_accessible_numbers(ui: &AppWindow) {
    ui.global::<AccessibleNumber<'_>>()
        .on_parse(|text, fallback| accessible_number(&text, fallback));
}

/// `text` as a number with a point or a comma, or `fallback` when it is none.
fn accessible_number(text: &str, fallback: f32) -> f32 {
    text.trim().replace(',', ".").parse().unwrap_or(fallback)
}

/// Makes an error message a sentence: a capital first letter and a full stop. Only for
/// the OS and portal errors that come untranslated; translated text keeps its own case.
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
    ui.set_has_controller(state.shows_pad());
    ui.set_device_name(state.description().short_name.as_str().into());
    let text = |items: Vec<String>| items.into_iter().map(SharedString::from).collect();
    push_model(&ui.get_controllers(), text(state.picker_labels()), |m| {
        ui.set_controllers(m);
    });
    ui.set_current_controller(i32::try_from(state.picker_index()).unwrap_or(0));
    push_model(&ui.get_move_from(), text(move_from(state)), |m| ui.set_move_from(m));
    push_model(&ui.get_changed_slots(), text(changed_slots(state)), |m| ui.set_changed_slots(m));
    push_model(&ui.get_changed_keep(), state.changed_choices(), |m| ui.set_changed_keep(m));
    ui.set_device_status(device_status(state).into());
    let locked = state.shown_lock();
    ui.set_connected(locked.is_none() && state.current_mode().is_some());
    // A group keeps its slots model while its slots hold, so only a changed one is redone.
    let current = ui.get_groups();
    // A locked controller has no slots to list.
    let mut rows = if locked.is_some() { Vec::new() } else { groups(state) };
    for (old, row) in current.iter().zip(&mut rows) {
        keep_same(&old.slots, &mut row.slots);
    }
    push_model(&current, rows, |m| ui.set_groups(m));
    ui.set_selected_mode(i32::try_from(state.selected.0).unwrap_or(0));
    ui.set_selected_slot(i32::try_from(state.selected.1).unwrap_or(0));
    ui.set_slot_title(slot_title(state).into());
    ui.set_empty_slot(selected_empty(state));
    let selected = state.selected_slot().map(|(m, n)| state.slot(m, n));
    let name = profile_name(state);
    if ui.get_profile_name() != name {
        ui.set_profile_name(name.into());
    }
    ui.set_can_rename(state.description().profile_rename);
    ui.set_name_max(state.description().limits.profile_name_length.max);
    ui.set_unsaved(selected.is_some_and(|s| s.unsaved()));
    render_buttons(state, ui);
    render_macros(state, ui);
    render_blocks(state, ui);
    render_steps(state, ui);
    render_panel(state, ui);
    render_sticks(state, ui);
    render_triggers(state, ui);
    render_settings(state, ui);
    render_files(state, ui);
    render_macro_files(state, ui);
    render_writes(state, ui);
    render_close(state, ui);
    render_locked(state, ui);
    ui.set_worker_stopped(state.worker_stopped);
    ui.set_read_error(state.read_error().unwrap_or_default().into());
    ui.set_read_error_elsewhere(state.failed_elsewhere());
    ui.set_read_error_title(
        if state.failed_elsewhere() {
            tr!(
                "Could not read the {name} controller plugged into another USB port.",
                name = state.description().short_name
            )
        } else {
            tr!("Could not read the controller.")
        }
        .into(),
    );
    ui.set_read_failed(locked.map_or_else(
        || state.shown().is_some_and(|c| c.read_error.is_some()),
        crate::locked::LockedPad::failed,
    ));
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

/// Narrower than this, in logical pixels, at the 10 pt desktop font, toolbar buttons show
/// only their icon.
const COMPACT_BELOW: f32 = 1200.0;

/// One `rem` at 10 pt, the desktop font `COMPACT_BELOW` was set for.
const BASE_REM: f32 = 40.0 / 3.0;

/// The window width, in logical pixels, below which the toolbar is compact when one `rem`
/// is `rem_px` wide. The same words need more room in a larger font, so the limit grows
/// with it; a smaller font keeps the 10 pt limit.
fn compact_below(rem_px: f32) -> f32 {
    COMPACT_BELOW * (rem_px / BASE_REM).max(1.0)
}

/// Sets the toolbar compact or not from the window's current width and font.
pub fn fit_toolbar(ui: &AppWindow) {
    let window = ui.window();
    let width = window.size().to_logical(window.scale_factor()).width;
    ui.set_compact(width < compact_below(ui.get_rem_px()));
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use controller_core::model::Mode;

    use super::*;
    use crate::state::tests::{full_read, hold_macros, macro_def, new_state, PORT};

    fn connected(mode: Mode) -> AppState {
        let mut s = new_state();
        s.presence(PORT, Some(mode));
        s.read_finished(PORT, Ok(full_read()));
        s
    }

    #[test]
    fn the_compact_limit_grows_with_the_font_and_never_shrinks() {
        assert_eq!(compact_below(12.0), 1200.0, "a small font keeps the 10 pt limit");
        assert_eq!(compact_below(BASE_REM), 1200.0);
        // 150 %: 15 pt, 20px.
        assert_eq!(compact_below(20.0), 1800.0);
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
        assert_eq!((dinput[1].mode, dinput[1].number), (2, 2));
        assert!(dinput[2].empty && dinput[2].name.is_empty());
        assert!(!dinput.iter().any(|s| s.unsaved));
    }

    #[test]
    fn sidebar_counts_every_macro_shown_with_a_button_or_not() {
        let mut s = connected(XINPUT);
        let macros = [macro_def(0, "Jump", &["rp"], 1), macro_def(2, "Idle", &[], 1)];
        hold_macros(&mut s, (XINPUT, 2), &macros);
        let count = |s: &AppState| groups(s)[0].slots.row_data(1).unwrap().macros;
        assert_eq!(count(&s), 2, "the controller's, not the profile's macro refs");
        s.select(0, 1);
        s.new_macro(1);
        assert_eq!(count(&s), 3);
        s.delete_macro(0);
        assert_eq!(count(&s), 2);
        assert_eq!(groups(&s)[0].slots.row_data(2).unwrap().macros, 0, "an empty slot");
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
    fn a_screen_reader_number_reads_with_a_point_or_a_comma() {
        assert_eq!(accessible_number("3", 0.0), 3.0);
        assert_eq!(accessible_number(" 1.6", 0.0), 1.6);
        assert_eq!(accessible_number("4,2", 0.0), 4.2);
        assert_eq!(accessible_number("loud", 2.0), 2.0);
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

    #[test]
    fn the_status_line_shows_a_reported_firmware() {
        let mut s = crate::state::tests::ultimate("1.09");
        assert_eq!(device_status(&s), "Connected · XInput · 1.09");
        s.read_started(PORT);
        assert_eq!(device_status(&s), "Reading over USB · XInput", "a read keeps its text");
        let s = crate::state::tests::ultimate("");
        assert_eq!(device_status(&s), "Connected over USB · XInput", "none reported");
    }

    #[test]
    fn an_unnamed_profile_shows_its_slot_number() {
        use controller_core::devices::ultimate::XINPUT;
        let mut s = crate::state::tests::ultimate("1.09");
        let first = groups(&s)[0].slots.row_data(0).unwrap();
        assert_eq!((first.name.as_str(), first.empty), ("Profile 1", false));
        assert!(groups(&s)[0].slots.row_data(1).unwrap().name.is_empty(), "an empty slot");
        assert_eq!(profile_name(&s), "Profile 1");
        s.set_name("Racing");
        assert_eq!(s.slot(XINPUT, 1).shown().unwrap().name, "", "no rename on this model");
        s.select(0, 1);
        s.start_from_default();
        assert_eq!(s.slot(XINPUT, 2).shown().unwrap().name, "", "a new profile has no name");
        assert_eq!(profile_name(&s), "Profile 2");
    }
}
