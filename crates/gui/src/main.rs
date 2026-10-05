//! `8b`: the desktop editor for the profiles stored on 8BitDo controllers.

// A binary crate: `pub` marks what other modules use.
#![allow(unreachable_pub)]

mod buttons;
mod chooser;
mod files;
mod holders;
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
use std::thread;
use std::time::Duration;

use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
use controller_core::devices::pro3::Pro3;
use controller_core::model::Mode;
use controller_core::transport::HidrawDevice;
use controller_core::Error;
use slint::{ComponentHandle as _, Timer, TimerMode, Weak};

use crate::buttons::{hit, picked_output, render_views};
use crate::render::{fit_toolbar, render, sentence};
use crate::state::{AppState, Notice};
use crate::ui::AppWindow;
use crate::worker::{Command, Event, Failure};

/// Sends `command` to the worker, and once it is sent, marks it `started`.
fn send(
    commands: &Sender<Command>,
    command: Command,
    s: &mut AppState,
    started: fn(&mut AppState),
) {
    if commands.send(command).is_ok() {
        started(s);
    }
}

/// Applies one worker event, with a line on stderr. A new presence starts a read
/// of every bank.
fn handle(state: &mut AppState, event: Event, commands: &Sender<Command>) {
    match &event {
        Event::Presence(mode) => eprintln!("8b: controller {}", mode.map_or("gone", Mode::label)),
        Event::Read(Ok(read)) => eprintln!("8b: read {} slots", read.profiles.len()),
        Event::Read(Err(f)) => eprintln!("8b: read failed: {}; held by {:?}", f.error, f.holders),
        Event::Installed(Ok(())) => eprintln!("8b: udev rule installed"),
        Event::Installed(Err(e)) => eprintln!("8b: udev rule install failed: {e}"),
    }
    match event {
        Event::Presence(mode) => {
            state.presence(mode);
            if mode.is_some() {
                send(commands, Command::ReadAll, state, AppState::read_started);
            }
        }
        Event::Read(Err(Failure { error: Error::PermissionDenied(_), .. })) => {
            state.read_denied(udev::rule_state());
        }
        Event::Read(Ok(read)) => state.read_finished(Ok(read)),
        Event::Read(Err(Failure { error, holders })) => {
            let message = holders::sentence(&holders).map_or_else(
                || error.to_string(),
                |names| format!("{} {names}", sentence(&error.to_string())),
            );
            state.read_finished(Err(message));
        }
        Event::Installed(result) => {
            let ok = result.is_ok();
            state.install_finished(result);
            // The rule applies to the node already present: read again, no replug.
            if ok {
                send(commands, Command::ReadAll, state, AppState::read_started);
            }
        }
    }
}

/// A dialog's answer, applied to the state on the event loop.
type Job = Box<dyn FnOnce(&mut AppState) + Send>;

/// Runs `ask` on a short thread, so the window stays live while a dialog is open,
/// and hands its answer to the event loop.
fn on_thread(ui: Weak<AppWindow>, jobs: Sender<Job>, ask: impl FnOnce() -> Job + Send + 'static) {
    let spawned = thread::Builder::new().name("file dialog".to_owned()).spawn(move || {
        if jobs.send(ask()).is_ok() {
            let _ = ui.upgrade_in_event_loop(|ui| ui.invoke_file_done());
        }
    });
    if let Err(e) = spawned {
        eprintln!("8b: no file dialog: {e}");
    }
}

/// The message for a dialog the portal could not show.
fn no_dialog(e: &str) -> Notice {
    Notice { error: true, title: "The file dialog did not open.".to_owned(), body: sentence(e) }
}

/// Asks for a file and loads it into slot `slot`'s edits.
fn import(ui: Weak<AppWindow>, jobs: Sender<Job>, slot: (Mode, u8)) {
    on_thread(ui, jobs, move || {
        let picked = chooser::open_json("Import a profile");
        let read = picked.map(|p| {
            p.map(|p| {
                (
                    files::display_name(&p),
                    std::fs::read_to_string(&p).map_err(|e| sentence(&e.to_string())),
                )
            })
        });
        Box::new(move |state: &mut AppState| match read {
            Ok(None) => {}
            Ok(Some((name, text))) => state.import(slot, &name, text),
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// Asks where to save the selected slot's profile, as shown, and saves it.
fn export(ui: Weak<AppWindow>, jobs: Sender<Job>, state: &AppState) {
    let Some(slot) = state.selected_slot() else { return };
    let shown = state.slot(slot.0, slot.1);
    let Some(profile) = shown.shown() else { return };
    let (text, unsaved, name) =
        (files::export_text(profile), shown.unsaved(), files::file_name(slot.0, slot.1));
    on_thread(ui, jobs, move || {
        let saved = chooser::save_json("Export the profile", &name).map(|p| {
            p.map(|p| {
                text.and_then(|t| std::fs::write(&p, t).map_err(|e| sentence(&e.to_string())))
                    .map(|()| files::display_name(&p))
            })
        });
        Box::new(move |state: &mut AppState| match saved {
            Ok(None) => {}
            Ok(Some(result)) => state.exported(slot, unsaved, result),
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// Wires Import and Export: each dialog runs on a short thread, and its answer
/// comes back through `file-done`.
fn wire_files(ui: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let (jobs_tx, jobs) = mpsc::channel::<Job>();
    let weak = ui.as_weak();
    let s = Rc::clone(state);
    ui.on_file_done(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = s.borrow_mut();
        while let Ok(job) = jobs.try_recv() {
            job(&mut state);
        }
        render(&state, &ui);
    });
    let (weak, s, tx) = (ui.as_weak(), Rc::clone(state), jobs_tx.clone());
    ui.on_import_file(move || {
        if let Some(slot) = s.borrow().selected_slot() {
            import(weak.clone(), tx.clone(), slot);
        }
    });
    let (weak, s) = (ui.as_weak(), Rc::clone(state));
    ui.on_export_file(move || export(weak.clone(), jobs_tx.clone(), &s.borrow()));
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
    let (install_tx, retry_tx) = (commands.clone(), commands.clone());

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

    wire_files(&ui, &state);

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
        c(&|s| send(&install_tx, Command::InstallUdevRule, s, AppState::install_started));
    });
    let c = change.clone();
    ui.on_read_again(move || c(&|s| send(&retry_tx, Command::ReadAll, s, AppState::read_started)));
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
    ui.on_import_confirmed(move || c(&AppState::confirm_import));
    let c = change.clone();
    ui.on_import_cancelled(move || c(&AppState::cancel_import));
    let c = change.clone();
    ui.on_notice_closed(move || c(&|s| s.notice = None));
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
