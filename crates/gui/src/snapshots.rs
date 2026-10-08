//! Smoke tests: each screen renders into memory with the software renderer, no
//! display needed. Set `GUI_SHOTS=<dir>` to keep the pictures as PPM files.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use std::collections::HashSet;
use std::fmt::Write as _;
use std::io::Write as _;
use std::rc::Rc;

use controller_core::model::Mode;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle as _, PhysicalSize, PlatformError};

use crate::buttons::render_views;
use crate::render::{fit_toolbar, render};
use crate::state::tests::{connected, full_read, new_state, PORT};
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
    render_views(state.description, &ui);
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
    s.presence(PORT, Some(Mode::XInput));
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    shoot("first-read-failed", &s);
}

#[test]
fn sidebar_with_an_empty_slot() {
    let mut s = connected(Mode::DInput);
    s.select(2, 2);
    shoot("empty-slot", &s);
}

#[test]
fn unplugged() {
    let mut s = connected(Mode::Switch);
    s.set_name("Edited");
    s.presence(PORT, None);
    shoot("unplugged", &s);
}

/// The Pro 3 on [`PORT`] with an edit in Switch slot 1, plus a second one in
/// `mode` on port `3-2`.
fn two_controllers(mode: Mode) -> AppState {
    let mut s = connected(Mode::Switch);
    s.set_name("Edited");
    s.presence("3-2", Some(mode));
    s.read_finished("3-2", Ok(full_read()));
    s
}

#[test]
fn controller_dropdown() {
    shoot("controller-dropdown", &two_controllers(Mode::Switch));
}

#[test]
fn move_edits_from_one() {
    let mut s = connected(Mode::Switch);
    s.set_name("Edited");
    s.presence(PORT, None);
    s.presence("3-2", Some(Mode::XInput));
    s.read_finished("3-2", Ok(full_read()));
    shoot("move-edits-one", &s);
}

#[test]
fn move_edits_from_several() {
    let mut s = two_controllers(Mode::XInput);
    s.pick_controller(1);
    s.set_name("Other");
    s.presence(PORT, None);
    s.presence("3-2", None);
    s.presence("3-3", Some(Mode::DInput));
    s.read_finished("3-3", Ok(full_read()));
    shoot("move-edits-several", &s);
}

#[test]
fn new_port_read_failed_behind_an_unplugged_entry() {
    let mut s = connected(Mode::Switch);
    s.set_name("Edited");
    s.presence(PORT, None);
    s.presence("3-2", Some(Mode::DInput));
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
    let mut s = connected(Mode::XInput);
    s.set_name("Mine");
    s.select(0, 1);
    s.set_name("Mine too");
    let mut read = full_read();
    for p in &mut read.profiles {
        if p.mode == Mode::XInput && p.source_slot < 3 {
            p.canonical.name = "Theirs".to_owned();
        }
    }
    s.read_finished(PORT, Ok(read));
    shoot("slots-changed", &s);
}

#[test]
fn read_failed() {
    let mut s = connected(Mode::XInput);
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    shoot("read-failed", &s);
}

#[test]
fn read_failed_with_holders() {
    let mut s = connected(Mode::Switch);
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
    let mut s = connected(Mode::Switch);
    s.sandboxed = true;
    let hint = crate::holders::hint(&[], s.sandboxed).unwrap();
    s.read_finished(PORT, Err(format!("Device communication timed out. {hint}")));
    shoot("read-failed-sandbox", &s);
}

#[test]
fn read_failed_trying_again() {
    let mut s = connected(Mode::Switch);
    s.read_finished(PORT, Err("device communication timed out".to_owned()));
    s.read_started(PORT);
    shoot("read-failed-trying-again", &s);
}

fn denied(rule: Rule) -> AppState {
    let mut s = new_state();
    s.presence(PORT, Some(Mode::XInput));
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
    let mut s = connected(Mode::XInput);
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
    let mut s = connected(Mode::XInput);
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
    shoot("buttons-clean", &connected(Mode::XInput));
}

#[test]
fn edited_slot() {
    let mut s = connected(Mode::Switch);
    s.set_output("r1", "disabled");
    s.set_output("rp", "top face");
    s.set_output("bottom face", "screenshot");
    s.set_name("Edited");
    shoot("buttons-edited", &s);
}

#[test]
fn slot_started_from_default() {
    let mut s = connected(Mode::DInput);
    s.select(2, 2);
    s.start_from_default();
    shoot("buttons-from-default", &s);
}

#[test]
fn slot_with_an_unrecognised_row() {
    let mut s = connected(Mode::DInput);
    s.select(2, 1);
    let pad =
        s.active_mut().unwrap().slots.get_mut(&(Mode::DInput, 2)).unwrap().pad.as_mut().unwrap();
    let left = pad.button_mappings.iter_mut().find(|m| m.source == "d-pad left").unwrap();
    left.target = controller_core::description::UNRECOGNISED_OUTPUT.to_owned();
    shoot("buttons-unrecognised", &s);
}

#[test]
fn slot_with_a_macro() {
    let mut s = connected(Mode::XInput);
    let pad =
        s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger: "rp".to_owned(),
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
    let s = connected(Mode::XInput);
    let ui = window(&s);
    let d = s.description;
    ui.on_hit(move |view, x, y| crate::buttons::hit(d, usize::try_from(view).unwrap(), x, y));
    // The left bumper on the front view at this size.
    click(&ui, 428.0, 190.0);
    let l1 = s.description.buttons.iter().position(|b| b.id == "l1").unwrap();
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
    let ui = window(&connected(Mode::XInput));
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
    let s = connected(Mode::XInput);
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
        render_views(state.description, &ui);
    }
    ui.set_tab(tab);
    save(&format!("{name}-{}", if dark { "dark" } else { "light" }), &ui);
}

/// A Switch slot with an edit on every settings tab.
fn edited_settings() -> AppState {
    let mut s = connected(Mode::Switch);
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
    let clean = connected(Mode::XInput);
    let edited = edited_settings();
    for dark in [false, true] {
        for (i, name) in [(1, "sticks"), (2, "triggers"), (3, "vibration")] {
            tab(&format!("{name}-clean"), &clean, i, dark);
            tab(&format!("{name}-edited"), &edited, i, dark);
        }
    }
}

#[test]
fn a_slider_drag_survives_the_render_after_each_move() {
    use slint::platform::{PointerEventButton, WindowEvent};
    let state = Rc::new(std::cell::RefCell::new(connected(Mode::XInput)));
    let ui = window(&state.borrow());
    ui.set_tab(1);
    let (s, weak) = (Rc::clone(&state), ui.as_weak());
    ui.on_number_changed(move |field, value| {
        s.borrow_mut().set_number(&field, value);
        render(&s.borrow(), &weak.upgrade().unwrap());
    });
    let low = || state.borrow().slot(Mode::XInput, 1).shown().unwrap().sticks.left_min_pct;
    let start = low();
    // The low knob of the left stick's dead zone at 1280×800, then two moves right.
    let at = |x: f32| slint::LogicalPosition::new(x, 203.0);
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
    let mut s = connected(Mode::XInput);
    let mut p = s.defaults.get(&Mode::Switch).unwrap().clone();
    "switch-slot-1-index-0".clone_into(&mut p.id);
    "Racing".clone_into(&mut p.name);
    if let Some(m) = p.button_mappings.iter_mut().find(|m| m.source == "r4") {
        "screenshot".clone_into(&mut m.target);
    }
    p.macro_refs.push(controller_core::model::MacroRef {
        trigger: "right face".to_owned(),
        path: "m.json".to_owned(),
    });
    let text = crate::files::export_text(&p).unwrap();
    s.import((Mode::XInput, 3), "profile-switch-slot-1-index-0.json", Ok(text));
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
    s.import((Mode::XInput, 3), "bad.json", Ok("{}".to_owned()));
    shoot("import-failed", &s);
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
    let mut s = connected(Mode::XInput);
    let pad =
        s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger: "rp".to_owned(),
        path: "xinput-slot1-macro3-ABCDEFGHIJKLMNO.json".to_owned(),
    });
    let pad =
        s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 2)).unwrap().pad.as_mut().unwrap();
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
    let mut s = connected(Mode::XInput);
    let pad =
        s.active_mut().unwrap().slots.get_mut(&(Mode::XInput, 1)).unwrap().pad.as_mut().unwrap();
    pad.macro_refs.push(controller_core::model::MacroRef {
        trigger: "rp".to_owned(),
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
    let mut s = connected(Mode::XInput);
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

#[test]
fn review_of_an_empty_slot_with_leftover_macros() {
    let mut s = connected(Mode::DInput);
    s.select(2, 2);
    s.start_from_default();
    s.set_leftover(PORT, [((Mode::DInput, 3), 2)].into());
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
    let mut s = connected(Mode::XInput);
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
        Mode::XInput,
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
    let mut s = connected(Mode::XInput);
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
    let ui = window(&connected(Mode::XInput));
    assert_eq!(ui.get_write_label(), "Write XInput slot 1…");
    save("footer-write-label", &ui);
}

#[test]
fn the_sidebar_bar_counts_the_edited_slots() {
    let mut s = connected(Mode::XInput);
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
        if s.slot(Mode::XInput, 3).pad.is_none() && (mode, slot) == (0, 2) {
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
    let mut s = connected(Mode::XInput);
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
    click(&ui, 640.0, 216.0);
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
    let mut s = connected(Mode::XInput);
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
    s.presence("3-2", Some(Mode::DInput));
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
    s.presence(PORT, Some(Mode::XInput));
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
