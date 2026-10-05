//! `8b`: the desktop editor for the profiles stored on 8BitDo controllers.

// A binary crate: `pub` marks what other modules use.
#![allow(unreachable_pub)]

mod render;
mod state;
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

use controller_core::device::ControllerSpec as _;
use controller_core::devices::pro3::Pro3;
use controller_core::model::Mode;
use controller_core::transport::HidrawDevice;
use slint::ComponentHandle as _;

use crate::render::render;
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
    }
    match event {
        Event::Presence(mode) => {
            state.presence(mode);
            if mode.is_some() && commands.send(Command::ReadAll).is_ok() {
                state.read_started();
            }
        }
        Event::Read(result) => state.read_finished(result.map_err(|e| e.to_string())),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let description = Pro3.description()?;
    let ui = AppWindow::new()?;
    ui.set_version(env!("CARGO_PKG_VERSION").into());
    let state = Rc::new(RefCell::new(AppState::new(description)));
    render(&state.borrow(), &ui);

    let (events_tx, events) = mpsc::channel();
    let weak = ui.as_weak();
    let commands = worker::spawn(
        Box::new(HidrawDevice::open()?),
        PathBuf::from(worker::SYSFS_USB),
        description.config_ports.clone(),
        move |event| {
            if events_tx.send(event).is_ok() {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_worker_event());
            }
        },
    )?;

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

    let weak = ui.as_weak();
    let s = Rc::clone(&state);
    ui.on_slot_picked(move |mode, slot| {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = s.borrow_mut();
        state.select(usize::try_from(mode).unwrap_or(0), usize::try_from(slot).unwrap_or(0));
        render(&state, &ui);
    });

    let weak = ui.as_weak();
    ui.on_read_note_closed(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = state.borrow_mut();
        state.close_read_note();
        render(&state, &ui);
    });

    ui.run()?;
    Ok(())
}
