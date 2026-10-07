//! `8b`: the desktop editor for the profiles stored on 8BitDo controllers.

// A binary crate: `pub` marks what other modules use.
#![allow(unreachable_pub)]

mod batch;
mod buttons;
mod chooser;
mod closing;
mod controllers;
mod files;
mod holders;
mod portal;
mod render;
mod review;
mod settings;
mod state;
mod udev;
mod worker;
mod writes;

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
use controller_core::service::read::leftover_macros;
use controller_core::transport::{DeviceIo, HidrawDevice};
use controller_core::Error;
use slint::{CloseRequestResponse, ComponentHandle as _, Timer, TimerMode, Weak};

use crate::buttons::{hit, picked_output, render_views};
use crate::closing::CloseOutcome;
use crate::render::{fit_toolbar, render, sentence};
use crate::state::{AppState, Notice};
use crate::ui::AppWindow;
use crate::worker::{Command, Event, Failure};
use crate::writes::WriteJob;

/// Asks the worker to read the controller on `port`, and once asked, marks the read
/// started.
fn read(commands: &Sender<Command>, state: &mut AppState, port: &str) {
    if commands.send(Command::ReadAll(port.to_owned())).is_ok() {
        state.read_started(port);
    }
}

/// Applies one worker event, with a line on stderr. A new presence starts a read
/// of every bank.
fn handle(state: &mut AppState, event: Event, commands: &Sender<Command>) {
    match &event {
        Event::Presence { port, mode } => {
            eprintln!("8b: controller on {port} {}", mode.map_or("gone", Mode::label));
        }
        Event::Read { port, result: Ok(read) } => {
            eprintln!("8b: read {} slots from {port}", read.profiles.len());
        }
        Event::Read { port, result: Err(f) } => {
            eprintln!("8b: read from {port} failed: {}; held by {:?}", f.error, f.holders);
        }
        Event::Written { port, results, .. } => {
            for result in results {
                eprintln!("8b: write to {port}: {}", result.message);
            }
        }
        Event::Installed(Ok(())) => eprintln!("8b: udev rule installed"),
        Event::Installed(Err(e)) => eprintln!("8b: udev rule install failed: {e}"),
    }
    match event {
        Event::Presence { port, mode } => {
            state.presence(&port, mode);
            if mode.is_some() {
                read(commands, state, &port);
            }
        }
        Event::Read { port, result: Err(Failure { error: Error::PermissionDenied(_), .. }) } => {
            state.read_denied(&port, udev::rule_state());
            state.review_read(&port, false);
        }
        Event::Read { port, result: Err(Failure { error: Error::UnsupportedModel(_), .. }) } => {
            state.read_foreign(&port);
            state.review_read(&port, false);
        }
        Event::Read { port, result: Ok(read) } => {
            let leftover = leftover_macros(&Pro3, &read);
            state.read_finished(&port, Ok(read));
            state.set_leftover(&port, leftover);
            state.review_read(&port, true);
            let write = state.skip_empty_review();
            send_write(commands, state, write);
        }
        Event::Read { port, result: Err(Failure { error, holders }) } => {
            let message = holders::sentence(&holders).map_or_else(
                || error.to_string(),
                |names| format!("{} {names}", sentence(&error.to_string())),
            );
            state.read_finished(&port, Err(message));
            state.review_read(&port, false);
        }
        // Whatever the outcome, read the slot back: what the controller holds now is
        // what the window should show.
        // A batch is one command, so one read follows it.
        Event::Written { port, results, holders } => {
            state.write_finished(&port, &results, holders::sentence(&holders).as_deref());
            state.batch_after_write();
            read(commands, state, &port);
        }
        Event::Installed(result) => {
            let ok = result.is_ok();
            state.install_finished(result);
            // The rule applies to the nodes already present: read again, no replug.
            if ok {
                let present: Vec<String> = state
                    .controllers
                    .iter()
                    .filter(|c| c.mode.is_some())
                    .map(|c| c.port.clone())
                    .collect();
                for port in present {
                    read(commands, state, &port);
                }
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

/// Wires "Try again", the controller picker and the answers to the move and
/// changed-slot dialogs. `change` applies a change to the state and renders.
fn wire_controllers(
    ui: &AppWindow,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    commands: Sender<Command>,
) {
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_read_again(move || {
        c(&|s| {
            if let Some(port) = s.retry_port() {
                read(&tx, s, &port);
            }
        });
    });
    let c = change.clone();
    ui.on_controller_picked(move |i| c(&|s| s.pick_controller(usize::try_from(i).unwrap_or(0))));
    let c = change.clone();
    ui.on_move_answered(move |row| {
        c(&|s| {
            let from = usize::try_from(row).ok().and_then(|r| s.move_candidates().get(r).copied());
            s.answer_move(from);
        });
    });
    let c = change.clone();
    ui.on_changed_picked(move |row, keep| {
        c(&|s| {
            let slot = usize::try_from(row).ok().and_then(|r| s.changed_slots().get(r).copied());
            if let Some(slot) = slot {
                s.choose_changed(slot, keep);
            }
        });
    });
    let (c, tx) = (change.clone(), commands);
    ui.on_changed_applied(move || {
        c(&|s| {
            s.apply_changed();
            let write = s.skip_empty_review();
            send_write(&tx, s, write);
        });
    });
}

/// Sends a write to the worker. A write the worker cannot take does not start.
fn send_write(
    commands: &Sender<Command>,
    state: &mut AppState,
    write: Option<(String, Vec<WriteJob>)>,
) {
    if let Some((port, jobs)) = write {
        if commands.send(Command::Write(port, jobs)).is_err() {
            state.write.running = None;
            state.write.batch = None;
        }
    }
}

/// Opens `folder` in the file manager. Not a hardware path, so a failure only goes
/// to stderr.
fn open_folder(folder: &std::path::Path) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(folder).spawn() {
        eprintln!("8b: could not open {}: {e}", folder.display());
    }
}

/// Wires the footer's write button, Clear slot, Write all and the answers to their
/// dialogs.
fn wire_writes(
    ui: &AppWindow,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    commands: &Sender<Command>,
    backups: PathBuf,
) {
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_write_clicked(move || {
        c(&|s| {
            if let Some(port) = s.begin_review() {
                read(&tx, s, &port);
            }
        });
    });
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_review_confirmed(move || {
        c(&|s| {
            let write = s.confirm_review();
            send_write(&tx, s, write);
        });
    });
    let c = change.clone();
    ui.on_review_cancelled(move || c(&AppState::cancel_review));
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_write_all_clicked(move || {
        c(&|s| {
            if let Some(port) = s.begin_batch() {
                read(&tx, s, &port);
            }
        });
    });
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_batch_confirmed(move || {
        c(&|s| {
            let write = s.confirm_batch();
            send_write(&tx, s, write);
        });
    });
    let c = change.clone();
    ui.on_batch_cancelled(move || c(&AppState::end_batch));
    let c = change.clone();
    ui.on_batch_slot_toggled(move |row| {
        c(&|s| s.toggle_batch_slot(usize::try_from(row).unwrap_or(usize::MAX)));
    });
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_batch_retried(move || {
        c(&|s| {
            let write = s.retry_batch();
            send_write(&tx, s, write);
        });
    });
    let c = change.clone();
    ui.on_batch_closed(move || c(&AppState::end_batch));
    let c = change.clone();
    ui.on_clear_clicked(move || c(&AppState::begin_clear));
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_clear_confirmed(move || {
        c(&|s| {
            let write = s.confirm_clear();
            send_write(&tx, s, write);
        });
    });
    let c = change.clone();
    ui.on_clear_cancelled(move || c(&AppState::cancel_clear));
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_write_retried(move || {
        c(&|s| {
            let write = s.retry_write();
            send_write(&tx, s, write);
        });
    });
    let c = change.clone();
    ui.on_failure_closed(move || c(&AppState::dismiss_failure));
    ui.on_open_backup_folder(move || open_folder(&backups));
}

/// Wires every close request (the title-bar button, Alt+F4) through the close check,
/// and the answers to the close question.
fn wire_close(
    ui: &AppWindow,
    state: &Rc<RefCell<AppState>>,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    commands: Sender<Command>,
) {
    let (weak, s) = (ui.as_weak(), Rc::clone(state));
    ui.window().on_close_requested(move || {
        let Some(ui) = weak.upgrade() else { return CloseRequestResponse::HideWindow };
        let mut state = s.borrow_mut();
        let outcome = state.close_requested();
        render(&state, &ui);
        if outcome == CloseOutcome::Close {
            CloseRequestResponse::HideWindow
        } else {
            CloseRequestResponse::KeepWindowShown
        }
    });
    let c = change.clone();
    ui.on_close_cancelled(move || c(&AppState::cancel_close));
    let c = change.clone();
    ui.on_close_write(move || {
        c(&|s| {
            if let Some(port) = s.write_before_close() {
                read(&commands, s, &port);
            }
        });
    });
    let weak = ui.as_weak();
    ui.on_close_discard(move || {
        if let Some(ui) = weak.upgrade() {
            if let Err(e) = ui.hide() {
                eprintln!("8b: could not close the window: {e}");
            }
        }
    });
}

/// Wires the edits: the button picker, the name field, the settings, the import
/// warning, the message bar and Discard.
fn wire_edits(ui: &AppWindow, change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static)) {
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
    let c = change.clone();
    ui.on_start_from_default(move || c(&AppState::start_from_default));
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let description = Pro3.description()?;
    let ui = AppWindow::new()?;
    // Wayland matches the window to its `.desktop` file by this ID, for the task bar icon.
    // Slint has no platform until the first window exists, and the ID must precede `show`.
    slint::set_xdg_app_id("io.github.LucasPicoli._8B")?;
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

    let backups = writes::backup_dir(std::env::var_os("XDG_STATE_HOME"), std::env::var_os("HOME"));
    let (events_tx, events) = mpsc::channel();
    let weak = ui.as_weak();
    let commands = worker::spawn(
        Box::new(|port: &str| Box::new(HidrawDevice::at(port)) as Box<dyn DeviceIo + Send>),
        PathBuf::from(worker::SYSFS_USB),
        description.config_ports.clone(),
        udev::install_rule,
        backups.clone(),
        move |event| {
            if events_tx.send(event).is_ok() {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_worker_event());
            }
        },
    )?;
    let (install_tx, retry_tx, write_tx, close_tx) =
        (commands.clone(), commands.clone(), commands.clone(), commands.clone());

    let weak = ui.as_weak();
    let s = Rc::clone(&state);
    ui.on_worker_event(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = s.borrow_mut();
        while let Ok(event) = events.try_recv() {
            handle(&mut state, event, &commands);
        }
        state.settle_close();
        render(&state, &ui);
    });

    wire_files(&ui, &state);

    // The close check reads the state outside a change.
    let close_state = Rc::clone(&state);
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
    wire_controllers(&ui, &change, retry_tx);
    wire_writes(&ui, &change, &write_tx, backups);
    wire_close(&ui, &close_state, &change, close_tx);
    let c = change.clone();
    ui.on_rule_skipped(move || c(&|s| s.rule_skipped = true));
    wire_edits(&ui, &change);

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
