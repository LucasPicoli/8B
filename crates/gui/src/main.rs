//! `8b`: the desktop editor for the profiles stored on 8BitDo controllers.

// A binary crate: `pub` marks what other modules use.
#![allow(unreachable_pub)]

mod buttons;
mod portal;
mod render;
mod settings;
mod state;
mod udev;
mod worker;

#[cfg(test)]
mod snapshots;

// The generated code trips these; hand-written code keeps the workspace lints.
#[allow(
    unreachable_pub,
    missing_docs,
    clippy::all,
    clippy::pedantic,
    clippy::nursery,
    clippy::unwrap_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::todo
)]
mod ui {
    slint::include_modules!();
}

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::time::Duration;

use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
use controller_core::devices::pro3::Pro3;
use controller_core::model::Mode;
use controller_core::transport::HidrawDevice;
use controller_core::Error;
use slint::{ComponentHandle as _, Timer, TimerMode};

use crate::buttons::{hit, picked_output, render_views};
use crate::render::{fit_toolbar, render};
use crate::state::AppState;
use crate::ui::AppWindow;
use crate::worker::{Command, Event};

/// Applies one worker event, with a line on stderr. A new presence starts a read
/// of every bank.
fn handle(state: &mut AppState, event: Event, commands: &Sender<Command>) {
    match &event {
        Event::Presence(mode) => eprintln!("8b: controller {}", mode.map_or("gone", Mode::label)),
        Event::Read(Ok(read)) => eprintln!("8b: read {} slots", read.profiles.len()),
        Event::Read(Err(e)) => eprintln!("8b: read failed: {e}"),
        Event::Installed(Ok(())) => eprintln!("8b: udev rule installed"),
        Event::Installed(Err(e)) => eprintln!("8b: udev rule install failed: {e}"),
    }
    match event {
        Event::Presence(mode) => {
            state.presence(mode);
            if mode.is_some() && commands.send(Command::ReadAll).is_ok() {
                state.read_started();
            }
        }
        Event::Read(Err(Error::PermissionDenied(_))) => state.read_denied(udev::rule_state()),
        Event::Read(result) => state.read_finished(result.map_err(|e| e.to_string())),
        Event::Installed(result) => {
            let ok = result.is_ok();
            state.install_finished(result);
            // The rule applies to the node already present: read again, no replug.
            if ok && commands.send(Command::ReadAll).is_ok() {
                state.read_started();
            }
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let description = Pro3.description()?;
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let defaults = description.modes.iter().map(|m| (m.id, Pro3.default_profile(m.id))).collect();
    let mut first = AppState::new(description, defaults);
    first.rule = udev::rule_state();
    let state = Rc::new(RefCell::new(first));
    render_views(description, &ui);
    render(&state.borrow(), &ui);
    portal::follow_accent(ui.as_weak());

    // ponytail: polls every 200 ms; Slint gives Rust no resize callback.
    let width_timer = Timer::default();
    let weak = ui.as_weak();
    width_timer.start(TimerMode::Repeated, Duration::from_millis(200), move || {
        if let Some(ui) = weak.upgrade() {
            fit_toolbar(&ui);
        }
    });

    let (events_tx, events) = mpsc::channel();
    let weak = ui.as_weak();
    let commands = worker::spawn(
        Box::new(HidrawDevice::open()?),
        PathBuf::from(worker::SYSFS_USB),
        description.config_ports.clone(),
        udev::install_rule,
        move |event| {
            if events_tx.send(event).is_ok() {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_worker_event());
            }
        },
    )?;
    let install_tx = commands.clone();

    let weak = ui.as_weak();
    let s = Rc::clone(&state);
    ui.on_worker_event(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = s.borrow_mut();
        while let Ok(event) = events.try_recv() {
            handle(&mut state, event, &commands);
        }
        render(&state, &ui);
    });

    // Every change from the window: apply it to the state, then render.
    let weak = ui.as_weak();
    let change = move |apply: &dyn Fn(&mut AppState)| {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = state.borrow_mut();
        apply(&mut state);
        render(&state, &ui);
    };
    let c = change.clone();
    ui.on_slot_picked(move |mode, slot| {
        let (mode, slot) = (usize::try_from(mode).unwrap_or(0), usize::try_from(slot).unwrap_or(0));
        c(&|s| s.select(mode, slot));
    });
    let c = change.clone();
    ui.on_install_rule(move || {
        c(&|s| {
            if install_tx.send(Command::InstallUdevRule).is_ok() {
                s.install_started();
            }
        });
    });
    let c = change.clone();
    ui.on_rule_skipped(move || c(&|s| s.rule_skipped = true));
    let c = change.clone();
    ui.on_output_picked(move |row, choice| {
        let (Ok(row), Ok(choice)) = (usize::try_from(row), usize::try_from(choice)) else { return };
        c(&|s| {
            if let Some((button, output)) = picked_output(s, row, choice) {
                s.set_output(&button, &output);
            }
        });
    });
    let c = change.clone();
    ui.on_name_edited(move |name| c(&|s| s.set_name(&name)));
    let c = change.clone();
    ui.on_number_changed(move |field, value| c(&|s| s.set_number(&field, value)));
    let c = change.clone();
    ui.on_flag_changed(move |field, on| c(&|s| s.set_flag(&field, on)));
    let c = change.clone();
    ui.on_discard(move || c(&AppState::discard));
    ui.on_start_from_default(move || change(&AppState::start_from_default));

    ui.on_hit(move |view, x, y| {
        hit(description, usize::try_from(view).unwrap_or(usize::MAX), x, y)
    });

    let weak = ui.as_weak();
    ui.on_theme_changed(move || {
        if let Some(ui) = weak.upgrade() {
            render_views(description, &ui);
        }
    });

    ui.run()?;
    Ok(())
}
