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
use crate::render::render;
use crate::state::tests::{connected as read, full_read, new_state};
use crate::state::AppState;
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
    let ui = window(state);
    let shot = ui.window().take_snapshot().unwrap();
    assert_eq!((shot.width(), shot.height()), (1280, 800));
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

fn connected(mode: Mode) -> AppState {
    let mut s = new_state();
    s.presence(Some(mode));
    s.read_finished(Ok(full_read()));
    s
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
fn sidebar_with_read_note() {
    shoot("read-note", &connected(Mode::XInput));
}

#[test]
fn sidebar_after_closing_the_note() {
    let mut s = connected(Mode::DInput);
    s.close_read_note();
    s.select(2, 2);
    shoot("empty-slot", &s);
}

#[test]
fn unplugged() {
    let mut s = connected(Mode::Switch);
    s.close_read_note();
    s.presence(None);
    shoot("unplugged", &s);
}

#[test]
fn read_failed() {
    let mut s = connected(Mode::XInput);
    s.close_read_note();
    s.read_finished(Err("device communication timed out".to_owned()));
    shoot("read-failed", &s);
}

fn denied(rule_installed: bool) -> AppState {
    let mut s = new_state();
    s.presence(Some(Mode::XInput));
    s.read_started();
    s.read_denied(rule_installed);
    s
}

#[test]
fn permission_denied() {
    shoot("permission-denied", &denied(false));
}

#[test]
fn permission_installing() {
    let mut s = denied(false);
    s.install_started();
    shoot("permission-installing", &s);
}

#[test]
fn permission_install_failed() {
    let mut s = denied(false);
    s.install_started();
    s.install_finished(Err(
        "The password prompt was closed, or the password was not accepted.".to_owned()
    ));
    shoot("permission-install-failed", &s);
}

#[test]
fn permission_still_denied() {
    shoot("permission-still-denied", &denied(true));
}

#[test]
fn clean_slot() {
    shoot("buttons-clean", &read(Mode::XInput));
}

#[test]
fn edited_slot() {
    let mut s = read(Mode::Switch);
    s.set_output("r1", "disabled");
    s.set_output("rp", "top face");
    s.set_output("bottom face", "screenshot");
    s.set_name("Edited");
    shoot("buttons-edited", &s);
}

#[test]
fn slot_started_from_default() {
    let mut s = read(Mode::DInput);
    s.select(2, 2);
    s.start_from_default();
    shoot("buttons-from-default", &s);
}

#[test]
fn slot_with_an_unrecognised_row() {
    let mut s = read(Mode::DInput);
    s.select(2, 1);
    let pad = s.slots.get_mut(&(Mode::DInput, 2)).unwrap().pad.as_mut().unwrap();
    let left = pad.button_mappings.iter_mut().find(|m| m.source == "d-pad left").unwrap();
    left.target = controller_core::description::UNRECOGNISED_OUTPUT.to_owned();
    shoot("buttons-unrecognised", &s);
}

#[test]
fn a_click_on_the_drawing_selects_the_button_row() {
    let s = read(Mode::XInput);
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
    let ui = window(&read(Mode::XInput));
    ui.set_selected_row(4);
    // XInput slot 1, the slot on screen, in the sidebar.
    click(&ui, 130.0, 105.0);
    assert_eq!(ui.get_selected_row(), 4, "the same slot keeps it");
    // XInput slot 2.
    click(&ui, 130.0, 143.0);
    assert_eq!(ui.get_selected_row(), -1);
}
