//! Smoke tests: each screen renders into memory with the software renderer, no
//! display needed. Set `GUI_SHOTS=<dir>` to keep the pictures as PPM files.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::rc::Rc;

use controller_core::devices::pro3::{DINPUT, SWITCH, XINPUT};
use controller_core::model::Mode;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle as _, PhysicalSize, PlatformError};

use crate::buttons::render_views;
use crate::render::{fit_toolbar, render};
use crate::state::tests::{connected, full_read, hold_macros, macro_def, new_state, PORT};
use crate::state::{AppState, Rule};
use crate::ui::{AppWindow, Change, Diagnostics};

struct Headless;

thread_local! {
    /// What the window last put on the clipboard.
    static CLIPBOARD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

impl Platform for Headless {
    fn set_clipboard_text(&self, text: &str, _: slint::platform::Clipboard) {
        CLIPBOARD.with_borrow_mut(|c| text.clone_into(c));
    }

    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Shows `state` at 1280×800, with no display.
fn window(state: &AppState) -> AppWindow {
    // Each test runs on its own thread, and the platform is per thread.
    let _ = slint::platform::set_platform(Box::new(Headless));
    let ui = AppWindow::new().unwrap();
    render_views(state.description(), &ui);
    render(state, &ui);
    ui.window().set_size(PhysicalSize::new(1280, 800));
    ui.show().unwrap();
    ui
}

/// Renders `state` at 1280×800 and checks the picture is not blank.
fn shoot(name: &str, state: &AppState) {
    save(name, &window(state));
}

/// Checks the window's picture is not blank, and keeps it when `GUI_SHOTS` is set.
fn save(name: &str, ui: &AppWindow) {
    let shot = ui.window().take_snapshot().unwrap();
    let size = ui.window().size();
    assert_eq!((shot.width(), shot.height()), (size.width, size.height));
    let colours: HashSet<_> = shot.as_slice().iter().map(|p| (p.r, p.g, p.b)).collect();
    assert!(colours.len() > 8, "{name}: only {} colours", colours.len());

    if let Some(dir) = std::env::var_os("GUI_SHOTS") {
        let path = std::path::Path::new(&dir).join(format!("{name}.ppm"));
        let mut out = std::fs::File::create(path).unwrap();
        write!(out, "P6\n{} {}\n255\n", shot.width(), shot.height()).unwrap();
        for p in shot.as_slice() {
            out.write_all(&[p.r, p.g, p.b]).unwrap();
        }
    }
}

#[test]
fn no_controller() {
    shoot("no-controller", &new_state());
}

#[test]
fn first_read_failed() {
    let mut s = new_state();
    s.presence(PORT, Some(XINPUT));
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    shoot("first-read-failed", &s);
}

#[test]
fn sidebar_with_an_empty_slot() {
    let mut s = connected(DINPUT);
    s.select(2, 2);
    shoot("empty-slot", &s);
}

#[test]
fn unplugged() {
    let mut s = connected(SWITCH);
    s.set_name("Edited");
    s.presence(PORT, None);
    shoot("unplugged", &s);
}

/// The Pro 3 on [`PORT`] with an edit in Switch slot 1, plus a second one in
/// `mode` on port `3-2`.
fn two_controllers(mode: Mode) -> AppState {
    let mut s = connected(SWITCH);
    s.set_name("Edited");
    s.presence("3-2", Some(mode));
    s.read_finished("3-2", Ok(full_read()));
    s
}

#[test]
fn controller_dropdown() {
    shoot("controller-dropdown", &two_controllers(SWITCH));
}

#[test]
fn move_edits_from_one() {
    let mut s = connected(SWITCH);
    s.set_name("Edited");
    s.presence(PORT, None);
    s.presence("3-2", Some(XINPUT));
    s.read_finished("3-2", Ok(full_read()));
    shoot("move-edits-one", &s);
}

#[test]
fn move_edits_from_several() {
    let mut s = two_controllers(XINPUT);
    s.pick_controller(1);
    s.set_name("Other");
    s.presence(PORT, None);
    s.presence("3-2", None);
    s.presence("3-3", Some(DINPUT));
    s.read_finished("3-3", Ok(full_read()));
    shoot("move-edits-several", &s);
}

#[test]
fn new_port_read_failed_behind_an_unplugged_entry() {
    let mut s = connected(SWITCH);
    s.set_name("Edited");
    s.presence(PORT, None);
    s.presence("3-2", Some(DINPUT));
    s.read_finished(
        "3-2",
        Err("Device communication timed out. steam also has the controller open. Close it \
             and try again."
            .to_owned()),
    );
    shoot("new-port-read-failed", &s);
}

#[test]
fn slots_changed_under_edits() {
    let mut s = connected(XINPUT);
    s.set_name("Mine");
    s.select(0, 1);
    s.set_name("Mine too");
    let mut read = full_read();
    for p in &mut read.profiles {
        if p.mode == XINPUT && p.source_slot < 3 {
            p.canonical.name = "Theirs".to_owned();
        }
    }
    s.read_finished(PORT, Ok(read));
    shoot("slots-changed", &s);
}

#[test]
fn read_failed() {
    let mut s = connected(XINPUT);
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    shoot("read-failed", &s);
}

#[test]
fn read_failed_with_holders() {
    let mut s = connected(SWITCH);
    s.read_finished(
        PORT,
        Err("Device communication timed out. steam and winedevice.exe also have \
         the controller open. Close them and try again."
            .to_owned()),
    );
    shoot("read-failed-holders", &s);
}

#[test]
fn read_failed_in_the_sandbox() {
    let mut s = connected(SWITCH);
    s.sandboxed = true;
    let hint = crate::holders::hint(&[], s.sandboxed).unwrap();
    s.read_finished(PORT, Err(format!("Device communication timed out. {hint}")));
    shoot("read-failed-sandbox", &s);
}

#[test]
fn read_failed_trying_again() {
    let mut s = connected(SWITCH);
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    s.read_started(PORT);
    shoot("read-failed-trying-again", &s);
}

fn denied(rule: Rule) -> AppState {
    let mut s = new_state();
    s.presence(PORT, Some(XINPUT));
    s.read_started(PORT);
    s.read_denied(PORT, rule);
    s
}

#[test]
fn permission_denied() {
    shoot("permission-denied", &denied(Rule::Missing));
}

#[test]
fn permission_denied_in_the_sandbox() {
    let mut s = denied(Rule::Missing);
    assert!(!window(&s).get_sandboxed());
    s.sandboxed = true;
    let ui = window(&s);
    assert!(ui.get_sandboxed() && ui.get_asks_for_rule());
    save("permission-denied-sandbox", &ui);
}

#[test]
fn permission_outdated() {
    let mut s = connected(XINPUT);
    s.rule = Rule::Outdated;
    shoot("permission-outdated", &s);
}

#[test]
fn permission_installing() {
    let mut s = denied(Rule::Missing);
    s.install_started();
    shoot("permission-installing", &s);
}

#[test]
fn permission_install_failed() {
    let mut s = denied(Rule::Missing);
    s.install_started();
    s.install_finished(Err(
        "The password prompt was closed, or the password was not accepted.".to_owned()
    ));
    shoot("permission-install-failed", &s);
}

#[test]
fn permission_still_denied() {
    shoot("permission-still-denied", &denied(Rule::Current));
}

#[test]
fn permission_checked_and_still_denied() {
    let mut s = denied(Rule::Missing);
    s.check_started();
    shoot("permission-checked-denied", &s);
    s.sandboxed = true;
    shoot("permission-checked-denied-sandbox", &s);
}

#[test]
fn permission_checking() {
    let mut s = denied(Rule::Missing);
    s.check_started();
    s.read_started(PORT);
    shoot("permission-checking", &s);
}

/// A connected controller the fix is offered for.
fn offered() -> AppState {
    let mut s = connected(XINPUT);
    s.fix_verdict(PORT, true);
    s
}

#[test]
fn fix_offer_bar() {
    let ui = window(&offered());
    assert!(ui.get_fix_offered() && !ui.get_fix_open());
    save("fix-bar", &ui);
}

#[test]
fn fix_offer_dialog_in_both_builds() {
    let mut s = offered();
    s.fix_open = true;
    shoot("fix-dialog", &s);
    s.sandboxed = true;
    shoot("fix-dialog-sandbox", &s);
    s.sandboxed = false;
    s.fix_install_started();
    shoot("fix-dialog-installing", &s);
    s.fix_install_finished(Err("The password prompt was closed.".to_owned()));
    shoot("fix-dialog-failed", &s);
}

#[test]
fn clean_slot() {
    shoot("buttons-clean", &connected(XINPUT));
}

#[test]
fn edited_slot() {
    let mut s = connected(SWITCH);
    s.set_output("r1", "disabled");
    s.set_output("rp", "top face");
    s.set_output("bottom face", "screenshot");
    s.set_name("Edited");
    shoot("buttons-edited", &s);
}

#[test]
fn slot_started_from_default() {
    let mut s = connected(DINPUT);
    s.select(2, 2);
    s.start_from_default();
    shoot("buttons-from-default", &s);
}

#[test]
fn slot_with_an_unrecognised_row() {
    let mut s = connected(DINPUT);
    s.select(2, 1);
    let pad = s.active_mut().unwrap().slots.get_mut(&(DINPUT, 2)).unwrap().pad.as_mut().unwrap();
    let left = pad.button_mappings.iter_mut().find(|m| m.source == "d-pad left").unwrap();
    left.target = controller_core::description::UNRECOGNISED_OUTPUT.to_owned();
    shoot("buttons-unrecognised", &s);
}

#[test]
fn slot_with_a_macro() {
    let mut s = connected(XINPUT);
    let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger_buttons: vec!["rp".to_owned()],
        path: "xinput-slot1-macro0-Buttons.json".to_owned(),
    });
    // Tall enough to show the back view and the paddle rows.
    let tall = |name: &str, s: &AppState| {
        let ui = window(s);
        ui.window().set_size(PhysicalSize::new(1280, 1300));
        save(name, &ui);
    };
    tall("buttons-macro", &s);
    s.set_output("rp", "left face");
    tall("buttons-macro-removed", &s);
}

#[test]
fn a_click_on_the_drawing_selects_the_button_row() {
    let s = connected(XINPUT);
    let ui = window(&s);
    let d = s.description();
    ui.on_hit(move |view, x, y| crate::buttons::hit(d, usize::try_from(view).unwrap(), x, y));
    // The left bumper on the front view at this size.
    click(&ui, 428.0, 205.0);
    let l1 = s.description().buttons.iter().position(|b| b.id == "l1").unwrap();
    assert_eq!(ui.get_selected_row(), i32::try_from(l1).unwrap());
}

/// Clicks the window at (`x`, `y`).
fn click(ui: &AppWindow, x: f32, y: f32) {
    use slint::platform::{PointerEventButton, WindowEvent};
    let position = slint::LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn picking_another_slot_clears_the_selected_button() {
    let ui = window(&connected(XINPUT));
    ui.set_selected_row(4);
    // XInput slot 1, the slot on screen, in the sidebar.
    click(&ui, 130.0, 105.0);
    assert_eq!(ui.get_selected_row(), 4, "the same slot keeps it");
    // XInput slot 2.
    click(&ui, 130.0, 143.0);
    assert_eq!(ui.get_selected_row(), -1);
}

/// The editor at `width`×800, with the toolbar fitted to it.
fn toolbar_at(width: u32) -> AppWindow {
    let s = connected(XINPUT);
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(width, 800));
    fit_toolbar(&ui);
    ui
}

#[test]
fn toolbar_is_icon_only_below_1200() {
    let ui = toolbar_at(1100);
    assert!(ui.get_compact());
    save("toolbar-1100", &ui);
}

#[test]
fn toolbar_has_text_from_1200() {
    let ui = toolbar_at(1300);
    assert!(!ui.get_compact());
    save("toolbar-1300", &ui);
}

/// Tab `tab` of `state`, in dark colours when `dark`.
fn tab(name: &str, state: &AppState, tab: i32, dark: bool) {
    let ui = window(state);
    if dark {
        ui.global::<crate::ui::Palette<'_>>().set_color_scheme(slint::language::ColorScheme::Dark);
        render_views(state.description(), &ui);
    }
    ui.set_tab(tab);
    save(&format!("{name}-{}", if dark { "dark" } else { "light" }), &ui);
}

/// A Switch slot with an edit on every settings tab.
fn edited_settings() -> AppState {
    let mut s = connected(SWITCH);
    s.set_number("/sticks/left_min_pct", 12.0);
    s.set_number("/sticks/right_max_pct", 85.0);
    s.set_flag("/sticks/invert_right_y", true);
    s.set_flag("/sticks/swap_sticks", true);
    s.set_number("/triggers/left_threshold_pct", 40.0);
    s.set_flag("/triggers/swap_triggers", true);
    s.set_number("/vibration/left_level", 1.0);
    s
}

#[test]
fn settings_tabs() {
    let clean = connected(XINPUT);
    let edited = edited_settings();
    for dark in [false, true] {
        for (i, name) in [(2, "sticks"), (3, "triggers"), (4, "vibration")] {
            tab(&format!("{name}-clean"), &clean, i, dark);
            tab(&format!("{name}-edited"), &edited, i, dark);
        }
    }
}

#[test]
fn a_slider_drag_survives_the_render_after_each_move() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let state = Rc::new(std::cell::RefCell::new(connected(XINPUT)));
    let ui = window(&state.borrow());
    ui.set_tab(2);
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_number_changed(move |field, value| {
        s.borrow_mut().set_number(&field, value);
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    let low = || {
        let s = state.borrow();
        let pct = s.slot(XINPUT, 1).shown().unwrap().setting("/sticks/left_min_pct").cloned();
        pct.and_then(|v| v.as_i64()).unwrap()
    };
    let start = low();
    // The low knob of the left stick's dead zone at 1280×800, then two moves right.
    let at = |x: f32| slint::LogicalPosition::new(x, 219.0);
    let button = PointerEventButton::Left;
    let x0 = 294.0 + 9.0 + 271.0 * f32::from(u8::try_from(start).unwrap()) / 100.0;
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0) });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at(x0), button });
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0 + 27.0) });
    let first = low();
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(x0 + 54.0) });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at(x0 + 54.0), button });
    assert!(first > start, "{start} -> {first}");
    assert!(low() > first, "the drag stopped after one render: {first} -> {}", low());
}

/// A Switch file with a Screenshot output, waiting to go into `XInput` slot 3.
fn cross_mode_import() -> AppState {
    let mut s = connected(XINPUT);
    let mut p = s.defaults().get(&SWITCH).unwrap().clone();
    "switch-slot-1-index-0".clone_into(&mut p.id);
    "Racing".clone_into(&mut p.name);
    if let Some(m) = p.button_mappings.iter_mut().find(|m| m.source == "r4") {
        "screenshot".clone_into(&mut m.target);
    }
    p.macro_refs.push(controller_core::model::MacroRef {
        trigger_buttons: vec!["right face".to_owned()],
        path: "m.json".to_owned(),
    });
    let text = crate::files::export_text(&p).unwrap();
    s.import((XINPUT, 3), "profile-switch-slot-1-index-0.json", Ok(text));
    assert!(s.pending_import.is_some());
    s
}

#[test]
fn import_warning() {
    let s = cross_mode_import();
    tab("import-warning", &s, 0, false);
    tab("import-warning", &s, 0, true);
}

#[test]
fn imported_with_a_note() {
    let mut s = cross_mode_import();
    s.confirm_import();
    shoot("import-done", &s);
    s.import((XINPUT, 3), "bad.json", Ok("{}".to_owned()));
    shoot("import-failed", &s);
}

/// An `XInput` file with two macros, one started by a button it remaps and one with a
/// trigger glide, waiting to go into Switch slot 2, which holds a macro.
fn cross_mode_macros_import() -> AppState {
    use controller_core::model::GlideControl;
    let mut s = connected(XINPUT);
    crate::state::tests::hold_macros(
        &mut s,
        (SWITCH, 2),
        &[crate::state::tests::macro_def(0, "Dash", &["r4"], 2)],
    );
    let mut p = s.defaults().get(&XINPUT).unwrap().clone();
    "xinput-slot-1-index-0".clone_into(&mut p.id);
    "Racing".clone_into(&mut p.name);
    if let Some(m) = p.button_mappings.iter_mut().find(|m| m.source == "r4") {
        "bottom face".clone_into(&mut m.target);
    }
    let mut ramp = crate::state::tests::macro_def(0, "Ramp", &["r4", "r3"], 3);
    ramp.steps[0].trigger_left = 100;
    ramp.steps[0].glide = [GlideControl::LeftTrigger].into();
    ramp.steps[1].trigger_left = 255;
    p.macros = Some(vec![ramp, crate::state::tests::macro_def(1, "Hop", &["lp"], 2)]);
    let text = crate::files::export_text(&p).unwrap();
    s.import((SWITCH, 2), "profile-xinput-slot-1-index-0.json", Ok(text));
    assert!(s.pending_import.is_some());
    s
}

#[test]
fn import_warning_with_macros() {
    let mut s = cross_mode_macros_import();
    tab("import-warning-macros", &s, 0, false);
    tab("import-warning-macros", &s, 0, true);
    s.confirm_import();
    shoot("import-done-macros", &s);
}

/// An `XInput` file with one macro that maps R4 to A, waiting to go into `XInput` slot
/// 2, whose macro R4 starts and which has an unwritten rename: the same-mode dialog
/// with Import without macros.
#[test]
fn import_same_mode_with_macros() {
    let mut s = connected(XINPUT);
    hold_macros(&mut s, (XINPUT, 2), &[macro_def(0, "Dash", &["r4"], 2)]);
    s.select(0, 1);
    s.set_name("Edited");
    let mut p = s.defaults().get(&XINPUT).unwrap().clone();
    "xinput-slot-1-index-0".clone_into(&mut p.id);
    "Racing".clone_into(&mut p.name);
    if let Some(m) = p.button_mappings.iter_mut().find(|m| m.source == "r4") {
        "bottom face".clone_into(&mut m.target);
    }
    p.macros = Some(vec![macro_def(0, "Ramp", &["r3"], 3)]);
    let text = crate::files::export_text(&p).unwrap();
    s.import((XINPUT, 2), "profile-xinput-slot-1-index-0.json", Ok(text));
    assert!(s.pending_import.is_some());
    tab("import-same-mode-macros", &s, 0, false);
    tab("import-same-mode-macros", &s, 0, true);
}

/// The left and right edge of the drawing's frame, along a row through the views.
fn frame_edges(ui: &AppWindow) -> (usize, usize) {
    let shot = ui.window().take_snapshot().unwrap();
    let width = usize::try_from(shot.width()).unwrap();
    let row = &shot.as_slice()[400 * width..401 * width];
    let page = row[275];
    let left = (270..width).find(|&x| row[x] != page).unwrap();
    let right = (left + 5..width).find(|&x| row[x] == page).unwrap();
    (left, right)
}

#[test]
fn the_drawing_keeps_its_size_from_slot_to_slot() {
    let mut s = connected(XINPUT);
    let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger_buttons: vec!["rp".to_owned()],
        path: "xinput-slot1-macro3-ABCDEFGHIJKLMNO.json".to_owned(),
    });
    let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 2)).unwrap().pad.as_mut().unwrap();
    pad.button_mappings[0].target = controller_core::description::UNRECOGNISED_OUTPUT.to_owned();
    for width in [1000, 1280, 1600] {
        let mut edges = Vec::new();
        for (mode, slot) in [(0, 0), (0, 1), (1, 0), (1, 1), (2, 0), (2, 1)] {
            s.select(mode, slot);
            let ui = window(&s);
            ui.window().set_size(PhysicalSize::new(width, 900));
            edges.push(frame_edges(&ui));
        }
        assert!(edges.windows(2).all(|p| p[0] == p[1]), "at {width} px: {edges:?}");
    }
}

/// `XInput` slot 1 with edits in every tab and a macro removed, the review shown.
fn reviewing() -> AppState {
    let mut s = connected(XINPUT);
    let pad = s.active_mut().unwrap().slots.get_mut(&(XINPUT, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger_buttons: vec!["rp".to_owned()],
        path: "xinput-slot1-macro0-Buttons.json".to_owned(),
    });
    s.set_name("Edited");
    s.set_output("r1", "disabled");
    s.set_output("rp", "left face");
    s.set_number("/sticks/left_min_pct", 20.0);
    s.set_flag("/triggers/swap_triggers", true);
    s.begin_review().unwrap();
    s.review_read(PORT, true);
    s
}

/// Between the click and the review, the footer says the controller is being checked.
#[test]
fn footer_while_checking_the_slot() {
    let mut s = connected(XINPUT);
    s.set_name("Edited");
    s.begin_review().unwrap();
    s.read_started(PORT);
    let ui = window(&s);
    assert!(ui.get_checking_slot() && !ui.get_can_write());
    save("checking-slot", &ui);
}

#[test]
fn review_dialog() {
    shoot("review", &reviewing());
}

/// `XInput` slot 2 with only its macros edited: one changed, one removed, one added.
fn macro_edits() -> AppState {
    let mut s = connected(XINPUT);
    let held = [macro_def(0, "Jump combo", &["rp", "lp"], 12), macro_def(1, "Dash", &["lp"], 3)];
    hold_macros(&mut s, (XINPUT, 2), &held);
    s.select(0, 1);
    s.edit_macros(|m| {
        m[0].steps.extend(vec![controller_core::model::MacroStep::default(); 3]);
        m[0].trigger_buttons.truncate(1);
        m.remove(1);
        m.push(macro_def(2, "", &[], 4));
    });
    s.begin_review().unwrap();
    s.review_read(PORT, true);
    s
}

#[test]
fn review_of_macro_edits() {
    shoot("review-macros", &macro_edits());
}

#[test]
fn review_of_profile_and_macro_edits() {
    let mut s = macro_edits();
    s.cancel_review();
    s.set_output("r1", "disabled");
    s.begin_review().unwrap();
    s.review_read(PORT, true);
    shoot("review-profile-and-macros", &s);
}

#[test]
fn failure_with_a_saved_macro() {
    let mut s = macro_edits();
    let (_, jobs) = s.confirm_review().unwrap();
    let mut bad = controller_core::model::WriteResult::failure(
        XINPUT,
        jobs[0].slot,
        controller_core::ErrorCategory::WriteFailure,
        "Write failed. Rollback failed.",
    );
    bad.rollback_attempted = true;
    bad.backup_file_path = Some("/home/x/.local/state/8b/backups/xinput-slot2.bin".to_owned());
    bad.macro_backup_paths =
        vec!["/home/x/.local/state/8b/backups/xinput-slot2-macro0-Jump combo.json".to_owned()];
    s.write_finished(PORT, &[bad], None);
    shoot("write-failed-macro", &s);
}

#[test]
fn review_of_an_empty_slot_with_leftover_macros() {
    let mut s = connected(DINPUT);
    s.select(2, 2);
    s.start_from_default();
    s.set_leftover(PORT, [((DINPUT, 3), 2)].into());
    s.begin_review().unwrap();
    s.review_read(PORT, true);
    shoot("review-new", &s);
}

/// The review of a long change list at the smallest window the app allows keeps the
/// primary button inside the window, in the card's footer.
#[test]
fn review_footer_stays_on_screen_in_a_small_window() {
    let ui = window(&reviewing());
    ui.window().set_size(PhysicalSize::new(1024, 640));
    let mut info = ui.get_review();
    let long: Vec<_> = (0..30)
        .map(|i| Change { what: format!("Button {i}").into(), from: "A".into(), to: "B".into() })
        .collect();
    info.changes = Rc::new(slint::VecModel::from(long)).into();
    info.total = 30;
    ui.set_review(info);
    save("review-small-window", &ui);

    let shot = ui.window().take_snapshot().unwrap();
    let width = usize::try_from(shot.width()).unwrap();
    // The light theme's primary button fill, `Theme.accent-strong`.
    let lower_half = &shot.as_slice()[320 * width..];
    assert!(
        lower_half.iter().filter(|p| (p.r, p.g, p.b) == (0x1b, 0x74, 0xa8)).count() > 200,
        "no primary button on screen"
    );
}

#[test]
fn clear_dialog() {
    let mut s = reviewing();
    s.cancel_review();
    s.begin_clear();
    shoot("clear", &s);
}

#[test]
fn clear_dialog_without_body_rows() {
    let mut s = connected(XINPUT);
    s.begin_clear();
    shoot("clear-bare", &s);
}

#[test]
fn writing_sheet_and_failure() {
    let mut s = reviewing();
    let (_, mut jobs) = s.confirm_review().unwrap();
    let job = jobs.remove(0);
    shoot("writing", &s);
    let mut bad = controller_core::model::WriteResult::failure(
        XINPUT,
        job.slot,
        controller_core::ErrorCategory::WriteFailure,
        "Write failed at chunk 12/53. Rollback failed. Original profile saved to the file below.",
    );
    bad.backup_file_path = Some(
        "/home/lucas/.local/state/8b/backups/backup-xinput-slot-1-20261005-120000.bin".to_owned(),
    );
    s.write_finished(PORT, &[bad], Some("steam also has the controller open."));
    shoot("write-failed", &s);
}

/// `XInput` slots 1 and 2 and `DInput` slot 1, each with edits in every tab, with
/// Write all pressed and the controller read again.
fn batch_reviewing() -> AppState {
    let mut s = connected(XINPUT);
    for (mode, slot) in [(0, 0), (0, 1), (2, 0)] {
        s.select(mode, slot);
        s.set_name("Edited");
        s.set_output("r1", "disabled");
        s.set_output("l1", "disabled");
        s.set_number("/sticks/left_min_pct", 12.0);
        s.set_number("/sticks/right_max_pct", 85.0);
        s.set_flag("/sticks/invert_left_x", true);
        s.set_flag("/triggers/swap_triggers", true);
        s.set_number("/vibration/left_level", 1.0);
    }
    s.select(0, 0);
    s.begin_batch().unwrap();
    s.review_read(PORT, true);
    s
}

#[test]
fn the_footer_button_names_its_slot() {
    let ui = window(&connected(XINPUT));
    assert_eq!(ui.get_write_label(), "Write XInput slot 1…");
    save("footer-write-label", &ui);
}

#[test]
fn the_sidebar_bar_counts_the_edited_slots() {
    let mut s = connected(XINPUT);
    let clean = window(&s);
    assert_eq!(clean.get_edited_count(), 0);
    let blank = clean.window().take_snapshot().unwrap();
    let width = usize::try_from(blank.width()).unwrap();
    // The bottom of the sidebar is the slot list's white until a bar takes it.
    let corner = |ui: &AppWindow| {
        let shot = ui.window().take_snapshot().unwrap();
        shot.as_slice()[790 * width + 100]
    };
    let plain = corner(&clean);

    for (mode, slot) in [(0, 1), (0, 2), (2, 0)] {
        s.select(mode, slot);
        if s.slot(XINPUT, 3).pad.is_none() && (mode, slot) == (0, 2) {
            s.start_from_default();
        } else {
            s.set_name("Edited");
        }
    }
    let ui = window(&s);
    assert_eq!(ui.get_edited_count(), 3);
    assert_eq!(ui.get_edited_caption(), "3 slots have unsaved edits");
    assert_eq!(ui.get_write_all_label(), "Write all 3 slots…");
    assert!(ui.get_can_write_all());
    assert_ne!(corner(&ui), plain, "the bar fills the foot of the sidebar");
    save("sidebar-write-all", &ui);

    s.select(0, 1);
    s.discard();
    s.select(0, 2);
    s.discard();
    let ui = window(&s);
    assert_eq!(ui.get_edited_caption(), "1 slot has unsaved edits");
    assert_eq!(ui.get_write_all_label(), "Write 1 slot…");
}

#[test]
fn the_sidebar_bar_checks_the_controller_first() {
    let mut s = connected(XINPUT);
    s.set_name("Edited");
    s.begin_batch().unwrap();
    s.read_started(PORT);
    let ui = window(&s);
    assert!(ui.get_checking_all() && !ui.get_can_write_all() && !ui.get_can_write());
    save("sidebar-write-all-checking", &ui);
}

#[test]
fn the_batch_review() {
    let mut s = batch_reviewing();
    shoot("batch-review", &s);
    s.toggle_batch_slot(1);
    s.toggle_batch_slot(2);
    shoot("batch-review-open", &s);
}

#[test]
fn the_batch_review_keeps_its_buttons_in_a_small_window() {
    let mut s = batch_reviewing();
    s.toggle_batch_slot(1);
    s.toggle_batch_slot(2);
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(1024, 640));
    save("batch-review-small-window", &ui);

    let shot = ui.window().take_snapshot().unwrap();
    let width = usize::try_from(shot.width()).unwrap();
    // The light theme's primary button fill, `Theme.accent-strong`.
    let lower_half = &shot.as_slice()[320 * width..];
    assert!(
        lower_half.iter().filter(|p| (p.r, p.g, p.b) == (0x1b, 0x74, 0xa8)).count() > 200,
        "no primary button on screen"
    );
}

#[test]
fn the_batch_review_opens_and_closes_a_row_on_a_click() {
    let ui = window(&batch_reviewing());
    let toggled = Rc::new(std::cell::Cell::new(-1));
    let t = Rc::clone(&toggled);
    ui.on_batch_slot_toggled(move |row| t.set(row));
    // The header of the first row, in the card the review centres in the window.
    click(&ui, 640.0, 257.0);
    assert_eq!(toggled.get(), 0);
}

/// The batch of 3 slots, writing in one run.
fn batch_writing() -> AppState {
    let mut s = batch_reviewing();
    s.confirm_batch().unwrap();
    s
}

#[test]
fn the_batch_writing_dialog() {
    let s = batch_writing();
    assert_eq!(window(&s).get_batch().at, 0);
    shoot("batch-writing", &s);
}

#[test]
fn the_batch_failure_dialog_with_a_backup() {
    let mut s = batch_writing();
    let jobs = s.write.running.as_ref().unwrap().1.clone();
    // The first slot's bank went out; the second's failed and took the third with it.
    let done = controller_core::model::WriteResult::success(jobs[0].mode, jobs[0].slot, "done");
    let mut bad = controller_core::model::WriteResult::failure(
        jobs[1].mode,
        jobs[1].slot,
        controller_core::ErrorCategory::WriteFailure,
        "Write failed at chunk 12/53. Rollback failed. Original profile saved to the file below.",
    );
    bad.backup_file_path = Some(
        "/home/lucas/.local/state/8b/backups/backup-xinput-slot-2-20261005-120000.bin".to_owned(),
    );
    let results = [done, bad.clone(), bad];
    s.write_finished(PORT, &results, Some("steam also has the controller open."));
    s.batch_after_write();
    assert_eq!(window(&s).get_batch().at, 1);
    shoot("batch-failed", &s);
}

/// A controller in `XInput` with each of `slots` (mode index, 0-based slot) renamed,
/// and the window asked to close.
fn closing(slots: &[(usize, usize)]) -> AppState {
    let mut s = connected(XINPUT);
    for (i, &(mode, slot)) in slots.iter().enumerate() {
        s.select(mode, slot);
        s.set_name(&format!("Edit {}", i + 1));
    }
    s.select(0, 0);
    s.close_requested();
    s
}

#[test]
fn the_close_question_for_one_slot() {
    let s = closing(&[(0, 1)]);
    let info = window(&s).get_closing();
    assert!(info.shown);
    assert_eq!((info.total, info.write), (1, 1));
    shoot("close-one-slot", &s);
}

#[test]
fn the_close_question_for_three_slots() {
    let s = closing(&[(0, 1), (1, 1), (2, 0)]);
    assert_eq!(window(&s).get_closing().write, 3);
    shoot("close-three-slots", &s);
}

#[test]
fn the_close_question_without_write_for_an_unplugged_controller() {
    let mut s = closing(&[(0, 1), (2, 0)]);
    s.presence(PORT, None);
    assert_eq!(window(&s).get_closing().write, 0);
    shoot("close-no-write", &s);
}

#[test]
fn the_close_question_over_two_controllers() {
    let mut s = closing(&[(0, 0), (1, 1), (2, 0)]);
    s.presence("3-2", Some(DINPUT));
    s.read_finished("3-2", Ok(full_read()));
    s.pick_controller(1);
    s.select(2, 1);
    s.set_name("Other pad");
    s.close_requested();
    let info = window(&s).get_closing();
    assert_eq!((info.total, info.write), (4, 1));
    shoot("close-two-controllers", &s);
}

#[test]
fn the_close_question_over_the_batch_review() {
    let mut s = batch_reviewing();
    s.close_requested();
    let ui = window(&s);
    assert!(ui.get_batch().stage == 1 && ui.get_closing().shown);
    assert_eq!(ui.get_closing().write, 0, "another dialog is open");
    save("close-over-review", &ui);
}

slint::slint! {
    import { DialogShell } from "ui/parts.slint";
    export component Shell inherits Window {
        in property <length> body-height;
        out property <length> card-x: d.card-x;
        out property <length> card-y: d.card-y;
        d := DialogShell {
            title: "t";
            Rectangle { height: root.body-height; }
        }
    }
}

slint::slint! {
    import { DialogShell } from "ui/parts.slint";
    import { Button, LineEdit } from "ui/look/breeze/widgets.slint";
    export component EnterShell inherits Window {
        in property <string> accept-text: "OK";
        in property <bool> accept-enabled: true;
        in property <bool> destructive;
        in-out property <int> accepted;
        in-out property <int> pressed;
        in-out property <string> field;
        public function focus-field() { le.focus(); }
        public function focus-button() { b.focus(); }
        DialogShell {
            title: "t";
            accept-text: root.accept-text;
            accept-enabled: root.accept-enabled;
            destructive: root.destructive;
            accepted => { root.accepted += 1; }
            VerticalLayout {
                le := LineEdit { text <=> root.field; }
                b := Button { text: "Inner"; clicked => { root.pressed += 1; } }
            }
        }
    }
}

/// Presses Enter in `ui`, then releases it. The main Enter key and the keypad's both
/// send `Key.Return`. With `repeat`, the key also auto-repeats once while held.
fn enter(ui: &EnterShell, repeat: bool) {
    use slint::platform::{Key, WindowEvent};
    let text: slint::SharedString = Key::Return.into();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    if repeat {
        ui.window().dispatch_event(WindowEvent::KeyPressRepeated { text: text.clone() });
    }
    ui.window().dispatch_event(WindowEvent::KeyReleased { text });
}

fn enter_shell() -> EnterShell {
    let _ = slint::platform::set_platform(Box::new(Headless));
    let ui = EnterShell::new().unwrap();
    ui.show().unwrap();
    ui.window().set_size(PhysicalSize::new(800, 600));
    ui
}

#[test]
fn enter_accepts_a_dialog() {
    let ui = enter_shell();
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 1);
}

#[test]
fn enter_does_nothing_while_accept_is_disabled() {
    let ui = enter_shell();
    ui.set_accept_enabled(false);
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 0);
}

#[test]
fn enter_does_nothing_without_an_accept_button() {
    let ui = enter_shell();
    ui.set_accept_text("".into());
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 0);
}

#[test]
fn enter_closes_a_one_button_close_dialog() {
    let ui = enter_shell();
    ui.set_accept_text("Close".into());
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 1);
}

#[test]
fn enter_in_a_field_stays_in_the_field() {
    let ui = enter_shell();
    ui.invoke_focus_field();
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 0);
}

#[test]
fn enter_on_a_focused_button_presses_that_button() {
    let ui = enter_shell();
    ui.invoke_focus_button();
    enter(&ui, false);
    assert_eq!((ui.get_pressed(), ui.get_accepted()), (1, 0));
}

#[test]
fn a_held_enter_accepts_once() {
    let ui = enter_shell();
    // The key that opened a dialog may still be held: its repeats must not confirm.
    enter(&ui, true);
    assert_eq!(ui.get_accepted(), 1, "the press, not its repeat");
}

#[test]
fn enter_does_not_accept_a_destructive_dialog() {
    let ui = enter_shell();
    ui.set_destructive(true);
    enter(&ui, false);
    assert_eq!(ui.get_accepted(), 0);
}

slint::slint! {
    import { RangeSlider } from "ui/parts.slint";
    export component RangeShell inherits Window {
        width: 300px;
        height: 100px;
        in-out property <float> low: 40;
        in-out property <float> high: 60;
        in property <bool> enabled: true;
        in-out property <int> moves;
        public function focus-slider() { r.focus(); }
        r := RangeSlider {
            x: 20px;
            y: 30px;
            width: 200px;
            low <=> root.low;
            high <=> root.high;
            enabled: root.enabled;
            low-changed(_) => { root.moves += 1; }
            high-changed(_) => { root.moves += 1; }
        }
    }
}

fn range_shell() -> RangeShell {
    let _ = slint::platform::set_platform(Box::new(Headless));
    let ui = RangeShell::new().unwrap();
    ui.show().unwrap();
    ui.window().set_size(PhysicalSize::new(300, 100));
    ui.invoke_focus_slider();
    ui
}

/// Presses and releases `key`, held with Shift when `shift`.
fn press_key(ui: &RangeShell, key: slint::platform::Key, shift: bool) {
    use slint::platform::{Key, WindowEvent};
    let shift_text: slint::SharedString = Key::Shift.into();
    if shift {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: shift_text.clone() });
    }
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    if shift {
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: shift_text });
    }
}

#[test]
fn the_range_slider_jumps_by_key() {
    use slint::platform::Key;
    let ui = range_shell();
    let ends = || (ui.get_low(), ui.get_high());
    // Without Shift the keys move the low end, with Shift the high end. A page is a
    // tenth of the range.
    press_key(&ui, Key::PageDown, false);
    assert_eq!(ends(), (30.0, 60.0));
    press_key(&ui, Key::PageUp, false);
    assert_eq!(ends(), (40.0, 60.0));
    press_key(&ui, Key::Home, false);
    assert_eq!(ends(), (0.0, 60.0));
    press_key(&ui, Key::End, false);
    assert_eq!(ends(), (60.0, 60.0), "the low end stops at the high end");
    press_key(&ui, Key::End, true);
    assert_eq!(ends(), (60.0, 100.0));
    press_key(&ui, Key::PageDown, true);
    assert_eq!(ends(), (60.0, 90.0));
    press_key(&ui, Key::Home, true);
    assert_eq!(ends(), (60.0, 60.0), "the high end stops at the low end");
}

#[test]
fn the_range_slider_sends_nothing_for_a_move_that_keeps_its_value() {
    use slint::platform::Key;
    let ui = range_shell();
    press_key(&ui, Key::Home, false);
    assert_eq!(ui.get_moves(), 1);
    press_key(&ui, Key::Home, false);
    press_key(&ui, Key::LeftArrow, false);
    assert_eq!(ui.get_moves(), 1);
}

#[test]
fn a_disabled_range_slider_ignores_keys() {
    use slint::platform::Key;
    let ui = range_shell();
    ui.set_enabled(false);
    press_key(&ui, Key::Home, false);
    assert_eq!((ui.get_low(), ui.get_moves()), (40.0, 0));
}

#[test]
fn a_focused_range_slider_frames_both_knobs() {
    let ui = range_shell();
    ui.set_low(0.0);
    ui.set_high(100.0);
    // The frame is 2px wide and starts 2px outside a knob: its centre is 3px out. The
    // low knob spans x 20 to 38 and the high knob x 202 to 220, both centred on y 41.
    let pixel = |ui: &RangeShell, x: u32, y: u32| {
        let shot = ui.window().take_snapshot().unwrap();
        shot.as_slice()[usize::try_from(y * shot.width() + x).unwrap()]
    };
    let framed = (pixel(&ui, 17, 41), pixel(&ui, 223, 41));
    ui.set_enabled(false);
    let bare = (pixel(&ui, 17, 41), pixel(&ui, 223, 41));
    assert_ne!(framed.0, bare.0, "no frame left of the low knob");
    assert_ne!(framed.1, bare.1, "no frame right of the high knob");
    assert_eq!(bare.0, bare.1);
}

/// The dialog card sits on whole physical pixels at any window size, scale and content
/// height. A half-pixel offset blurs its text.
#[test]
fn dialog_card_sits_on_whole_pixels() {
    use slint::platform::WindowEvent;
    let _ = slint::platform::set_platform(Box::new(Headless));
    let ui = Shell::new().unwrap();
    ui.show().unwrap();
    for scale in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
        ui.window().dispatch_event(WindowEvent::ScaleFactorChanged { scale_factor: scale });
        for (w, h) in [(1280, 800), (1281, 801), (1024, 640), (1117, 723)] {
            for body in [90.0, 91.0, 90.5, 91.3] {
                ui.window().set_size(PhysicalSize::new(w, h));
                ui.set_body_height(body);
                let _ = ui.window().take_snapshot();
                for v in [ui.get_card_x(), ui.get_card_y()] {
                    let px = v * scale;
                    assert!(
                        (px - px.round()).abs() < 1e-3,
                        "{px} physical px at scale {scale}, window {w}x{h}, body {body}"
                    );
                }
            }
        }
    }
}

/// A click on "Copy diagnostics" puts the diagnostics on the clipboard, and the label
/// says "Copied".
#[test]
fn copy_diagnostics_fills_the_clipboard() {
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{Global as _, LogicalPosition};
    let mut s = new_state();
    s.worker_stopped = true;
    let ui = window(&s);
    Diagnostics::get(&ui).on_text(|| "run header\nlog line".into());
    save("stopped", &ui);
    let at = LogicalPosition::new(440.0, 444.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert_eq!(CLIPBOARD.with_borrow(Clone::clone), "run header\nlog line");
    save("stopped-copied", &ui);
}

/// A long log on the clipboard leaves the read-error bar its own height.
#[test]
fn copying_a_long_log_keeps_the_bar_size() {
    for dark in [false, true] {
        copy_from_the_bar(dark);
    }
}

/// Clicks "Copy diagnostics" in a two-line read-error bar, in dark colours when `dark`.
fn copy_from_the_bar(dark: bool) {
    use slint::platform::{PointerEventButton, WindowEvent};
    use slint::{Global as _, LogicalPosition};
    let mut s = new_state();
    s.presence(PORT, Some(XINPUT));
    // Two lines in the bar, as with the holders' names.
    s.read_finished(
        PORT,
        Err("The controller did not answer in time. steam, vivaldi-bin and winedevice.exe \
             also have the controller open. Close them and try again. The controller stays \
             plugged in."
            .to_owned()),
    );
    let ui = window(&s);
    if dark {
        ui.global::<crate::ui::Palette<'_>>().set_color_scheme(slint::language::ColorScheme::Dark);
    }
    let theme = if dark { "dark" } else { "light" };
    let long: String = (0..200).fold(String::new(), |mut out, i| {
        let _ = writeln!(out, "log line {i}");
        out
    });
    Diagnostics::get(&ui).on_text(move || long.clone().into());
    let red = |ui: &AppWindow| {
        let shot = ui.window().take_snapshot().unwrap();
        shot.as_slice().iter().filter(|p| u16::from(p.r) > u16::from(p.g.max(p.b)) + 10).count()
    };
    let before = red(&ui);
    // Hovered, so the frame shows the button's height beside "Try again".
    let at = LogicalPosition::new(1100.0, 39.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at });
    save(&format!("bar-before-copy-{theme}"), &ui);
    ui.window().dispatch_event(WindowEvent::PointerPressed {
        position: at,
        button: PointerEventButton::Left,
    });
    ui.window().dispatch_event(WindowEvent::PointerReleased {
        position: at,
        button: PointerEventButton::Left,
    });
    assert!(CLIPBOARD.with_borrow(|c| c.starts_with("log line 0")), "the click copied");
    save(&format!("bar-after-copy-{theme}"), &ui);
    let after = red(&ui);
    assert!(after < before * 2, "the bar grew: {before} red pixels before the copy, {after} after");
}

/// `XInput` slot 2 holding "Jump combo" on five buttons, and a macro with no name on R4
/// played 3 times, with a new macro staged in macro slot 3 and L4 mapped to A.
fn macros_tab() -> AppState {
    let mut s = connected(XINPUT);
    let held = [macro_def(0, "Jump combo", &["rp", "lp"], 12), macro_def(1, "Dash", &["r4"], 3)];
    hold_macros(&mut s, (XINPUT, 2), &held);
    s.select(0, 1);
    s.set_plays(1, crate::macro_tab::Plays::Times);
    s.set_play_count(1, 3);
    s.set_play_pause(1, 120);
    s.new_macro(2);
    for b in ["r3", "l3", "turbo"] {
        s.add_macro_button(0, b);
    }
    s.rename_macro(1, "");
    s.set_output("l4", "bottom face");
    s
}

#[test]
fn number_field_arrows_and_keys_step_to_the_next_multiple() {
    let s = macros_tab();
    let ui = window(&s);
    ui.set_tab(1);
    ui.set_selected_macro(1);
    render(&s, &ui);
    let got = Rc::new(std::cell::RefCell::new(Vec::new()));
    let g = Rc::clone(&got);
    ui.on_macro_pause_changed(move |_, v| g.borrow_mut().push(v));
    // The arrows of the Interval field, which holds 120 ms, at this size.
    click(&ui, 847.0, 281.0);
    click(&ui, 847.0, 295.0);
    // A typed value off the step, then the Up key from the focused field.
    ui.set_macro_rows(slint::ModelRc::from(Rc::new(slint::VecModel::from(
        crate::macro_tab::macro_rows(&s)
            .into_iter()
            .map(|mut r| {
                r.pause = 122;
                r
            })
            .collect::<Vec<_>>(),
    ))));
    click(&ui, 800.0, 288.0);
    ui.window().dispatch_event(slint::platform::WindowEvent::KeyPressed {
        text: slint::platform::Key::UpArrow.into(),
    });
    assert_eq!(*got.borrow(), [125, 115, 125]);
}

#[test]
fn the_selected_step_edits_its_duration_in_the_table() {
    let s = macros_tab();
    let ui = window(&s);
    ui.set_tab(1);
    render(&s, &ui);
    let got = Rc::new(std::cell::RefCell::new(Vec::new()));
    let g = Rc::clone(&got);
    ui.on_step_duration_changed(move |n, step, ms| g.borrow_mut().push((n, step, ms)));
    // The up arrow of the field in the first row, at this size.
    click(&ui, 1223.0, 412.0);
    assert_eq!(*got.borrow(), [(0, 0, 105)]);
}

#[test]
fn a_deleted_macro_offers_undo() {
    let mut s = macros_tab();
    s.delete_macro(1);
    let ui = window(&s);
    ui.set_tab(1);
    ui.set_selected_macro(1);
    render(&s, &ui);
    save("macros-deleted", &ui);
}

/// The Macros tab of [`macros_tab`] at macro slot `n`, saved as `name`.
fn macros_at(name: &str, s: &AppState, n: i32) {
    let ui = window(s);
    ui.set_tab(1);
    ui.set_selected_macro(n);
    render(s, &ui);
    save(name, &ui);
}

#[test]
fn an_invalid_or_warned_macro_says_why() {
    use controller_core::model::{GlideControl, MacroStep};
    let mut s = macros_tab();
    // Macro 2 has no name, so it is written as "Macro 2". Macro 1 moves the left stick
    // under the floor.
    s.set_number("/sticks/left_min_pct", 0.0);
    s.edit_macro(0, |m| m.steps[0].left_stick_x = 0);
    macros_at("macros-no-name", &s, 1);
    macros_at("macros-warned", &s, 0);
    // Macro 3 glides edge to edge three times: more steps than the controller holds.
    s.edit_macro(2, |m| {
        m.steps = (0..4_u8)
            .map(|i| MacroStep {
                duration_ms: u16::MAX,
                left_stick_x: if i % 2 == 0 { 0 } else { u8::MAX },
                glide: (i < 3).then_some(GlideControl::LeftStick).into_iter().collect(),
                ..MacroStep::default()
            })
            .collect();
    });
    macros_at("macros-too-many-steps", &s, 2);
    let ui = window(&s);
    ui.global::<crate::ui::Palette<'_>>().set_color_scheme(slint::language::ColorScheme::Dark);
    render_views(s.description(), &ui);
    ui.set_tab(1);
    ui.set_selected_macro(2);
    render(&s, &ui);
    save("macros-too-many-steps-dark", &ui);
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(1024, 640));
    fit_toolbar(&ui);
    ui.set_tab(1);
    ui.set_selected_macro(2);
    render(&s, &ui);
    save("macros-too-many-steps-narrow", &ui);
    // On another tab, the footer's Show leads to the refused macro.
    ui.set_tab(0);
    render(&s, &ui);
    save("buttons-macro-refused-narrow", &ui);
}

#[test]
fn a_macro_file_shows_what_changes_before_it_imports() {
    let mut s = macros_tab();
    let mut def = macro_def(1, "Ramp from Switch", &["r4", "l4", "rp", "l3"], 40);
    def.mode = SWITCH;
    def.steps[0].pressed_buttons = vec!["bottom face".to_owned()];
    let text = crate::macro_files::macro_text(&def, "8bitdo-pro3").unwrap();
    s.import_macro(1, "switch-slot1-macro1-Ramp_from_Switch.json", Ok(text));
    macros_at("macro-import", &s, 1);
    s.import_macro(1, "bad.json", Ok("{}".to_owned()));
    macros_at("macro-import-failed", &s, 1);
    s.cancel_macro_import();
    // Reasons long enough to wrap: each row grows to its text, nothing overlaps.
    let mut long = macro_def(1, &"Slide".repeat(30), &["r4"], 2);
    long.repeat_count = 0;
    s.import_macro(
        1,
        "long.json",
        Ok(crate::macro_files::macro_text(&long, "8bitdo-pro3").unwrap()),
    );
    macros_at("macro-import-long-reasons", &s, 1);
    s.cancel_macro_import();
    macros_at("macros-empty-slot", &s, 3);
}

#[test]
fn macros_tab_shows_each_macro_slot_and_the_selected_header() {
    let s = macros_tab();
    for dark in [false, true] {
        let ui = window(&s);
        if dark {
            ui.global::<crate::ui::Palette<'_>>()
                .set_color_scheme(slint::language::ColorScheme::Dark);
            render_views(s.description(), &ui);
        }
        ui.set_tab(1);
        let theme = if dark { "dark" } else { "light" };
        for (macro_slot, name) in [(0, "first"), (1, "times"), (3, "empty")] {
            ui.set_selected_macro(macro_slot);
            render(&s, &ui);
            save(&format!("macros-{name}-{theme}"), &ui);
        }
    }
    // R4 on the front view, under the pointer: its tip says a step cannot hold it.
    let ui = window(&s);
    let state = Rc::new(macros_tab());
    let st = Rc::clone(&state);
    ui.on_hit(move |v, x, y| {
        crate::buttons::hit(st.description(), usize::try_from(v).unwrap(), x, y)
    });
    ui.on_macro_hit(move |v, n, i, x, y| {
        crate::step_panel::action_hit(
            &state,
            usize::try_from(v).unwrap(),
            u8::try_from(n).unwrap(),
            usize::try_from(i).unwrap(),
            x,
            y,
        )
    });
    ui.set_tab(1);
    ui.set_selected_macro(1);
    let at = slint::LogicalPosition::new(472.0, 394.0);
    ui.window().dispatch_event(slint::platform::WindowEvent::PointerMoved { position: at });
    save("macros-hover", &ui);
    // The smallest window still fits the header of a macro that plays a number of times.
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(1024, 640));
    fit_toolbar(&ui);
    ui.set_tab(1);
    ui.set_selected_macro(1);
    save("macros-narrow", &ui);
    // A macro started by no button: the muted chip, then the picker, on one line.
    ui.set_selected_macro(2);
    save("macros-narrow-no-button", &ui);
}

/// `XInput` slot 2 holding "Slide jump" on PL: A held over two steps, LT pressed half and
/// gliding to full, the left stick gliding to the right, then a pause.
fn steps_tab() -> AppState {
    use controller_core::model::{GlideControl, MacroStep};
    let mut s = connected(XINPUT);
    let mut def = macro_def(0, "Slide jump", &["lp"], 0);
    let a = || vec!["bottom face".to_owned()];
    def.steps = vec![
        MacroStep { duration_ms: 300, pressed_buttons: a(), ..MacroStep::default() },
        MacroStep {
            duration_ms: 60,
            pressed_buttons: a(),
            trigger_left: 128,
            glide: [GlideControl::LeftTrigger].into(),
            ..MacroStep::default()
        },
        MacroStep {
            duration_ms: 500,
            trigger_left: 255,
            left_stick_x: 127,
            glide: [GlideControl::LeftStick].into(),
            ..MacroStep::default()
        },
        MacroStep { duration_ms: 120, left_stick_x: 255, ..MacroStep::default() },
        MacroStep { duration_ms: 200, ..MacroStep::default() },
    ];
    hold_macros(&mut s, (XINPUT, 2), &[def]);
    s.select(0, 1);
    s
}

#[test]
fn step_table_shows_lanes_glides_and_pauses() {
    let s = steps_tab();
    for dark in [false, true] {
        let ui = window(&s);
        if dark {
            ui.global::<crate::ui::Palette<'_>>()
                .set_color_scheme(slint::language::ColorScheme::Dark);
            render_views(s.description(), &ui);
        }
        ui.set_tab(1);
        ui.set_selected_step(2);
        // The app renders again when the selection changes; the test has no callbacks.
        render(&s, &ui);
        save(&format!("steps-{}", if dark { "dark" } else { "light" }), &ui);
    }
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(1024, 640));
    fit_toolbar(&ui);
    ui.set_tab(1);
    save("steps-narrow", &ui);
}

/// The Macros tab of [`steps_tab`] at step `step`, with the stick pad edits wired to
/// the state and a render after each, as the app does.
fn stick_pads_at(step: i32) -> (Rc<std::cell::RefCell<AppState>>, AppWindow) {
    let state = Rc::new(std::cell::RefCell::new(steps_tab()));
    let ui = window(&state.borrow());
    // Tall enough to show the whole panel: picking a step leaves the page still.
    ui.window().set_size(PhysicalSize::new(1280, 1200));
    ui.set_tab(1);
    render(&state.borrow(), &ui);
    ui.set_selected_step(step);
    render(&state.borrow(), &ui);
    slint::platform::update_timers_and_animations();
    let to = |v: i32| usize::try_from(v).unwrap();
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_stick_dragged(move |slot, step, stick, dx, dy, snap| {
        let slot = u8::try_from(slot).unwrap();
        s.borrow_mut().drag_stick(slot, to(step), to(stick), [dx, dy], snap);
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_stick_glided(move |slot, step, stick, on| {
        s.borrow_mut().set_stick_glide(u8::try_from(slot).unwrap(), to(step), to(stick), on);
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_trigger_set(move |slot, step, k, v, apart| {
        s.borrow_mut().set_trigger(u8::try_from(slot).unwrap(), to(step), to(k), v, apart);
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    (state, ui)
}

#[test]
fn a_quick_stick_drag_survives_the_render_after_each_move() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (state, ui) = stick_pads_at(2);
    let left = || {
        let step = &state.borrow().macro_steps(0).unwrap()[2];
        (step.left_stick_x, step.left_stick_y)
    };
    // From the left gate's centre at this size, right and then up, with no pause for
    // the page to decide on a pan.
    let at = |x: f32, y: f32| slint::LogicalPosition::new(x, y);
    let button = PointerEventButton::Left;
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(688.0, 684.0) });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at(688.0, 684.0), button });
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(714.0, 684.0) });
    let first = left();
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(714.0, 658.0) });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at(714.0, 658.0), button });
    assert!(first.0 > 180 && first.1 == 127, "halfway right: {first:?}");
    assert!(left().1 < 127, "the drag stopped after one render: {:?}", left());
}

#[test]
fn a_trigger_slider_drag_survives_the_render_after_each_move() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (state, ui) = stick_pads_at(2);
    let lt = || state.borrow().macro_steps(0).unwrap()[2].trigger_left;
    // Along the LT slider at this size, right, then right again.
    let at = |x: f32| slint::LogicalPosition::new(x, 830.0);
    let button = PointerEventButton::Left;
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(640.0) });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position: at(640.0), button });
    let pressed = lt();
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(690.0) });
    let first = lt();
    ui.window().dispatch_event(WindowEvent::PointerMoved { position: at(730.0) });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position: at(730.0), button });
    assert!(pressed < 20, "near the left end: {pressed}");
    assert!((110..=170).contains(&first), "halfway: {first}");
    assert!(lt() > 200, "the drag stopped after one render: {}", lt());
    assert_eq!(state.borrow_mut().undo_step(0), Some(2), "the drag is one undo");
    assert_eq!(lt(), 255);
}

#[test]
fn the_trigger_slider_moves_by_key() {
    use slint::platform::{Key, WindowEvent};
    let (state, ui) = stick_pads_at(2);
    let lt = || state.borrow().macro_steps(0).unwrap()[2].trigger_left;
    let press = |key: Key| {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
    };
    // A click on the LT knob, at its full press, focuses the slider and keeps 255.
    click(&ui, 739.0, 834.0);
    assert_eq!(lt(), 255);
    press(Key::Home);
    assert_eq!(lt(), 0);
    press(Key::RightArrow);
    assert_eq!(lt(), 1);
    press(Key::PageUp);
    assert_eq!(lt(), 27, "a tenth of the range");
    press(Key::End);
    assert_eq!(lt(), 255);
}

#[test]
fn the_arriving_trigger_links_back_to_the_step_that_glides_in() {
    let (_state, ui) = stick_pads_at(2);
    // "Glides in from step 2" under LT, at this size.
    click(&ui, 686.0, 866.0);
    assert_eq!(ui.get_selected_step(), 1);
}

#[test]
fn each_stick_has_its_own_snap() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let (state, ui) = stick_pads_at(2);
    let press = |x: f32, y: f32| {
        let position = slint::LogicalPosition::new(x, y);
        let button = PointerEventButton::Left;
        ui.window().dispatch_event(WindowEvent::PointerMoved { position });
        ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
        ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
    };
    let at = |left: bool| {
        let step = &state.borrow().macro_steps(0).unwrap()[2];
        if left {
            [step.left_stick_x, step.left_stick_y]
        } else {
            [step.right_stick_x, step.right_stick_y]
        }
    };
    // The right stick's box off, then a press near each gate's north-east corner, at this
    // size.
    click(&ui, 955.0, 763.0);
    press(1043.0, 645.0);
    press(723.0, 645.0);
    assert_ne!(at(false), [255, 0], "the right stick no longer snaps");
    assert_eq!(at(true), [255, 0], "the left one still does");
}

#[test]
fn the_glide_box_follows_the_selected_step() {
    use controller_core::model::GlideControl;
    let (state, ui) = stick_pads_at(2);
    let glides = |i: usize| {
        state.borrow().macro_steps(0).unwrap()[i].glide.contains(&GlideControl::LeftStick)
    };
    let pick = |step: i32| {
        ui.set_selected_step(step);
        render(&state.borrow(), &ui);
        slint::platform::update_timers_and_animations();
    };
    // The left stick's Glide box at this size. Step 3 glides: a click stops it. Step 4
    // does not: a click starts it. Back on step 3 the box must show off again, or the
    // next click sends "off" and nothing changes.
    click(&ui, 694.0, 601.0);
    pick(3);
    click(&ui, 694.0, 601.0);
    assert!(!glides(2) && glides(3));
    pick(2);
    click(&ui, 694.0, 601.0);
    assert!(glides(2), "the box kept step 4's tick");
}

#[test]
fn the_arriving_step_links_back_to_the_step_that_glides_in() {
    let (_state, ui) = stick_pads_at(3);
    // "Glides in from step 3" under the left pad, at this size.
    click(&ui, 822.0, 730.0);
    assert_eq!(ui.get_selected_step(), 2);
}

#[test]
fn tab_to_glide_shows_its_tip_and_a_click_does_not() {
    use slint::platform::{Key, WindowEvent};
    let (_state, ui) = stick_pads_at(3);
    let tab = || {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Tab.into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Tab.into() });
    };
    // A click on the right stick's disabled Glide box, then Tab back from Centre.
    click(&ui, 1015.0, 601.0);
    let before = ui.window().take_snapshot().unwrap();
    tab();
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: Key::Backtab.into() });
    ui.window().dispatch_event(WindowEvent::KeyReleased { text: Key::Backtab.into() });
    save("glide-focus-tip", &ui);
    let after = ui.window().take_snapshot().unwrap();
    // The tip sits under the box, over the right gate: its area changes after Tab.
    let area = |shot: &slint::SharedPixelBuffer<slint::Rgba8Pixel>| {
        (621..648_u32)
            .flat_map(|y| (1010..1250_u32).map(move |x| (x, y)))
            .map(|(x, y)| shot.as_slice()[usize::try_from(y * shot.width() + x).unwrap()])
            .collect::<Vec<_>>()
    };
    assert_ne!(area(&before), area(&after), "no tip after Tab");
}

#[test]
fn step_panel_shows_both_sticks_a_glide_and_the_floor_warning() {
    let mut s = steps_tab();
    s.set_number("/sticks/left_min_pct", 0.0);
    for dark in [false, true] {
        let ui = window(&s);
        if dark {
            ui.global::<crate::ui::Palette<'_>>()
                .set_color_scheme(slint::language::ColorScheme::Dark);
            render_views(s.description(), &ui);
        }
        ui.set_tab(1);
        render(&s, &ui);
        ui.set_selected_step(2);
        render(&s, &ui);
        slint::platform::update_timers_and_animations();
        save(&format!("step-panel-{}", if dark { "dark" } else { "light" }), &ui);
        // Step 2 glides LT to step 3's full press: a ring on the slider.
        ui.set_selected_step(1);
        render(&s, &ui);
        slint::platform::update_timers_and_animations();
        save(&format!("step-panel-trigger-glide-{}", if dark { "dark" } else { "light" }), &ui);
    }
    let ui = window(&s);
    ui.window().set_size(PhysicalSize::new(1024, 640));
    fit_toolbar(&ui);
    ui.set_tab(1);
    render(&s, &ui);
    ui.set_selected_step(3);
    render(&s, &ui);
    slint::platform::update_timers_and_animations();
    save("step-panel-narrow", &ui);
}

#[test]
fn a_trigger_glide_meets_the_next_bar_edge_to_edge() {
    let mut s = steps_tab();
    // Step 2 glides LT from 128 to 200: a bar 10.06px wide at this size, so the glide's
    // end and the next row's bar must round alike.
    s.edit_step_values(0, 2, None, |step| step.trigger_left = 200);
    let ui = window(&s);
    ui.set_tab(1);
    render(&s, &ui);
    let shot = ui.window().take_snapshot().unwrap();
    // How much of the LT lane row `y` covers with ink, in pixels.
    let width = |y: u32| -> f32 {
        (785..820_u32)
            .map(|x| shot.as_slice()[usize::try_from(y * shot.width() + x).unwrap()])
            .map(|p| (255.0 - f32::from(p.g)) / (255.0 - 116.0))
            .filter(|c| *c > 0.05)
            .sum()
    };
    // The glide's last pixel row, then the next step's first one, over the row line.
    let (end, next) = (width(440), width(442));
    assert!((end - next).abs() < 0.01, "the glide ends {end}px wide, the bar is {next}px");
    assert!((next - 10.0).abs() < 0.01, "a whole pixel width: {next}");
}

/// The Macros tab of [`steps_tab`] at `w`×`h`, with step edits wired to the state and a
/// render after each, as the app does.
fn steps_at(w: u32, h: u32) -> (Rc<std::cell::RefCell<AppState>>, AppWindow) {
    let state = Rc::new(std::cell::RefCell::new(steps_tab()));
    let ui = window(&state.borrow());
    ui.window().set_size(PhysicalSize::new(w, h));
    fit_toolbar(&ui);
    ui.set_tab(1);
    render(&state.borrow(), &ui);
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_step_edited(move |n, step, edit| {
        let edit = crate::step_table::StepEdit::at(edit).unwrap();
        let step = usize::try_from(step).unwrap();
        let picked = s.borrow_mut().edit_step(u8::try_from(n).unwrap(), step, edit);
        let ui = weak.upgrade().unwrap();
        ui.set_selected_step(i32::try_from(picked.unwrap()).unwrap());
        render(&s.borrow(), &ui);
    });
    (state, ui)
}

/// Clicks (`x`, `y`) and lets the change handlers run, as a drawn frame does.
fn click_drawn(ui: &AppWindow, x: f32, y: f32) {
    ui.window().take_snapshot().unwrap();
    click(ui, x, y);
    slint::platform::update_timers_and_animations();
    ui.window().take_snapshot().unwrap();
}

#[test]
fn add_step_stays_under_the_pointer() {
    for (w, h) in [(1024, 768), (1280, 1200)] {
        let (state, ui) = steps_at(w, h);
        // "Add step" in the table's toolbar, at this size.
        let x = if w < 1280 { 576.0 } else { 672.0 };
        for _ in 0..6 {
            click_drawn(&ui, x, 310.0);
        }
        assert_eq!(state.borrow().macro_steps(0).unwrap().len(), 11, "at {w}×{h}");
    }
}

#[test]
fn picking_a_step_keeps_the_page_still() {
    for (w, h) in [(1024, 768), (1280, 1200)] {
        let (_state, ui) = steps_at(w, h);
        // Every row of the table, out of order, at both sizes.
        for (row, y) in [(1, 422.0), (3, 502.0), (4, 542.0), (0, 382.0), (2, 462.0)] {
            click_drawn(&ui, 650.0, y);
            assert_eq!(ui.get_selected_step(), row, "at {w}×{h}");
        }
    }
}

#[test]
fn picking_a_step_from_the_panel_keeps_the_page_still() {
    use slint::platform::WindowEvent;
    let (_state, ui) = steps_at(1024, 768);
    // Focus in the left stick's X field, then the page scrolled until the table's top
    // rows are under the tab bar: taking focus back must not scroll the page to show
    // the whole table.
    click_drawn(&ui, 727.0, 665.0);
    let position = slint::LogicalPosition::new(760.0, 500.0);
    ui.window().dispatch_event(WindowEvent::PointerScrolled {
        position,
        delta_x: 0.0,
        delta_y: -300.0,
    });
    click_drawn(&ui, 650.0, 162.0);
    assert_eq!(ui.get_selected_step(), 2);
    click_drawn(&ui, 650.0, 202.0);
    assert_eq!(ui.get_selected_step(), 3);
}

#[test]
fn the_table_keys_work_after_a_row_click() {
    use slint::platform::{Key, WindowEvent};
    let (state, ui) = steps_at(1024, 768);
    let press = |key: Key| {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        ui.window().take_snapshot().unwrap();
    };
    click_drawn(&ui, 650.0, 422.0);
    press(Key::DownArrow);
    assert_eq!(ui.get_selected_step(), 2);
    for _ in 0..6 {
        press(Key::Insert);
    }
    assert_eq!(state.borrow().macro_steps(0).unwrap().len(), 11);
    press(Key::End);
    assert_eq!(ui.get_selected_step(), 10);
    save("steps-keyboard-end", &ui);
}

#[test]
fn shift_tab_from_a_row_leaves_the_table() {
    use slint::platform::{Key, WindowEvent};
    let (_state, ui) = steps_at(1024, 768);
    let press = |key: Key| {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: key.into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: key.into() });
        ui.window().take_snapshot().unwrap();
    };
    // The table takes keys only while its selected row shows the focus frame.
    click_drawn(&ui, 650.0, 422.0);
    press(Key::Backtab);
    press(Key::DownArrow);
    assert_eq!(ui.get_selected_step(), 1);
}

/// The step tab of `steps_tab` at 1280×800 with the controller views wired to the state,
/// as the app wires them.
fn views_at() -> (Rc<std::cell::RefCell<AppState>>, AppWindow) {
    let state = Rc::new(std::cell::RefCell::new(steps_tab()));
    let ui = window(&state.borrow());
    fit_toolbar(&ui);
    ui.set_tab(1);
    render(&state.borrow(), &ui);
    let st = Rc::clone(&state);
    ui.on_macro_hit(move |v, n, i, x, y| {
        let (v, n, i) = (usize::try_from(v).unwrap(), u8::try_from(n).unwrap(), usize::try_from(i));
        crate::step_panel::action_hit(&st.borrow(), v, n, i.unwrap(), x, y)
    });
    let st = Rc::clone(&state);
    ui.on_macro_neighbour(move |v, from, dir| {
        crate::step_panel::view_neighbour(&st.borrow(), usize::try_from(v).unwrap(), from, dir)
    });
    let (st, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_step_chip_toggled(move |n, step, chip| {
        let (n, step, chip) = (
            u8::try_from(n).unwrap(),
            usize::try_from(step).unwrap(),
            usize::try_from(chip).unwrap(),
        );
        st.borrow_mut().toggle_step_button(n, step, chip);
        render(&st.borrow(), &weak.upgrade().unwrap());
    });
    (state, ui)
}

#[test]
fn the_views_take_the_keyboard() {
    use slint::platform::{Key, WindowEvent};
    let (state, ui) = views_at();
    let press = |text: slint::SharedString| {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text });
        ui.window().take_snapshot().unwrap();
    };
    let held = || state.borrow().macro_steps(0).unwrap()[0].pressed_buttons.clone();
    assert_eq!(held(), ["bottom face"]);
    // A click on A in the front view releases it and leaves the ring there.
    click_drawn(&ui, 518.0, 438.0);
    assert_eq!(held(), Vec::<String>::new());
    save("views-keyboard-ring", &ui);
    // Up goes to Y, Space holds it; Left goes to X, Enter holds it.
    press(Key::UpArrow.into());
    press(" ".into());
    assert_eq!(held(), ["top face"]);
    press(Key::LeftArrow.into());
    press(Key::Return.into());
    assert_eq!(held(), ["top face", "left face"]);
}

#[test]
fn typing_a_duration_in_the_table_survives_the_render_after_each_digit() {
    use slint::platform::{Key, WindowEvent};
    let (state, ui) = steps_at(1280, 1200);
    let s = Rc::clone(&state);
    let weak = ui.as_weak();
    ui.on_step_duration_changed(move |n, step, ms| {
        s.borrow_mut().set_step_duration(
            u8::try_from(n).unwrap(),
            usize::try_from(step).unwrap(),
            ms,
        );
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    let ms = || state.borrow().macro_steps(0).unwrap()[0].duration_ms;
    // The first row's field holds 300; its text, at this size.
    click_drawn(&ui, 1180.0, 382.0);
    let type_key = |text: &str| {
        ui.window().dispatch_event(WindowEvent::KeyPressed { text: text.into() });
        ui.window().dispatch_event(WindowEvent::KeyReleased { text: text.into() });
        ui.window().take_snapshot().unwrap();
    };
    for key in [Key::End, Key::Backspace, Key::Backspace, Key::Backspace] {
        type_key(&String::from(char::from(key)));
    }
    for (digit, want) in [("4", 4), ("5", 45), ("6", 456)] {
        type_key(digit);
        assert_eq!(ms(), want, "the field lost its focus after {digit}");
    }
}
