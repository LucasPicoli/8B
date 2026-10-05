//! Smoke tests: each screen renders into memory with the software renderer, no
//! display needed. Set `GUI_SHOTS=<dir>` to keep the pictures as PPM files.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::HashSet;
use std::io::Write as _;
use std::rc::Rc;

use controller_core::model::Mode;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle as _, PhysicalSize, PlatformError};

use crate::buttons::render_views;
use crate::render::{fit_toolbar, render};
use crate::state::tests::{connected, new_state};
use crate::state::{AppState, Rule};
use crate::ui::AppWindow;

struct Headless;

impl Platform for Headless {
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
    s.presence(Some(Mode::XInput));
    s.read_finished(Err("device communication timed out".to_owned()));
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
    s.presence(None);
    shoot("unplugged", &s);
}

#[test]
fn read_failed() {
    let mut s = connected(Mode::XInput);
    s.read_finished(Err("device communication timed out".to_owned()));
    shoot("read-failed", &s);
}

fn denied(rule: Rule) -> AppState {
    let mut s = new_state();
    s.presence(Some(Mode::XInput));
    s.read_started();
    s.read_denied(rule);
    s
}

#[test]
fn permission_denied() {
    shoot("permission-denied", &denied(Rule::Missing));
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
    let pad = s.slots.get_mut(&(Mode::DInput, 2)).unwrap().pad.as_mut().unwrap();
    let left = pad.button_mappings.iter_mut().find(|m| m.source == "d-pad left").unwrap();
    left.target = controller_core::description::UNRECOGNISED_OUTPUT.to_owned();
    shoot("buttons-unrecognised", &s);
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

#[test]
fn about() {
    let s = connected(Mode::XInput);
    let ui = window(&s);
    ui.set_version("0.1.0".into());
    ui.set_dialog("about".into());
    save("about", &ui);
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
