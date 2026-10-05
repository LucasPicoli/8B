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

use crate::render::render;
use crate::state::tests::{description, full_read};
use crate::state::AppState;
use crate::ui::AppWindow;

struct Headless;

impl Platform for Headless {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer))
    }
}

/// Renders `state` at 1280×800 and checks the picture is not blank.
fn shoot(name: &str, state: &AppState) {
    // Each test runs on its own thread, and the platform is per thread.
    let _ = slint::platform::set_platform(Box::new(Headless));
    let ui = AppWindow::new().unwrap();
    render(state, &ui);
    ui.window().set_size(PhysicalSize::new(1280, 800));
    ui.show().unwrap();
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
    let mut s = AppState::new(description());
    s.presence(Some(mode));
    s.read_finished(Ok(full_read()));
    s
}

#[test]
fn no_controller() {
    shoot("no-controller", &AppState::new(description()));
}

#[test]
fn first_read_failed() {
    let mut s = AppState::new(description());
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
    let mut s = AppState::new(description());
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
