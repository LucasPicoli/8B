//! `8b`: the desktop editor for the profiles stored on 8BitDo controllers.

// A binary crate: `pub` marks what other modules use.
#![allow(unreachable_pub)]
// `tr!` fills named placeholders at run time, after the translation.
#![allow(clippy::literal_string_with_formatting_args)]

mod batch;
mod buttons;
mod chooser;
mod closing;
mod controllers;
mod embedded;
mod files;
mod fix;
mod holders;
mod keepalive;
mod language;
mod locked;
mod logging;
mod macro_check;
mod macro_files;
mod macro_revert;
mod macro_steps;
mod macro_tab;
mod macros;
mod plain;
mod portal;
mod preferences;
mod reasons;
mod render;
mod review;
mod settings;
mod state;
mod step_panel;
mod step_table;
mod stick_pad;
mod trigger_pad;
mod udev;
mod worker;
mod writes;

#[cfg(test)]
mod snapshots;
#[cfg(test)]
mod string_coverage;

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

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use controller_core::devices;
use controller_core::model::macros::macro_file_name;
use controller_core::model::Mode;
use controller_core::service::read::{leftover_macros, macro_summaries};
use controller_core::transport::lock::Unlocked;
use controller_core::Error;
use log::{info, warn};
use slint::winit_030::winit::window::UserAttentionType;
use slint::winit_030::WinitWindowAccessor as _;
use slint::{CloseRequestResponse, ComponentHandle as _, Timer, TimerMode, Weak};
use tr::tr;

use crate::buttons::{hit, picked_output, render_views};
use crate::closing::CloseOutcome;
use crate::macro_tab::{added_button, chip_button, Plays};
use crate::macros::named;
use crate::render::{fit_toolbar, render, sentence};
use crate::state::{AppState, Notice, Rule};
use crate::step_panel::{action_hit, view_neighbour};
use crate::step_table::StepEdit;
use crate::ui::{AppWindow, Diagnostics, Relock};
use crate::worker::{Command, Event, Failure};
use crate::writes::WriteJob;

/// Asks the worker to read the controller on `port`, and once asked, marks the read
/// started.
fn read(commands: &Sender<Command>, state: &mut AppState, port: &str) {
    if state.send(commands, Command::ReadAll(port.to_owned())) {
        state.read_started(port);
    }
}

/// Asks the worker to read the steps of the macros of the controller on `port` that
/// are still unread. The window stays usable while they load.
fn read_macro_steps(commands: &Sender<Command>, state: &mut AppState, port: &str) {
    let unread = state.unread_macros(port);
    if !unread.is_empty() {
        state.send(commands, Command::ReadMacroSteps(port.to_owned(), unread));
    }
}

/// Why a read failed, as the red bar says it, with the programs that hold the
/// controller.
fn read_failure(state: &AppState, error: &Error, holders: &[String]) -> String {
    let plain = plain::read_failure(error);
    match holders::hint(holders, state.sandboxed) {
        Some(names) => format!("{plain} {names}"),
        None => plain,
    }
}

/// Asks the worker to read every controller that is present.
fn read_present(commands: &Sender<Command>, state: &mut AppState) {
    let present: Vec<String> =
        state.controllers.iter().filter(|c| c.mode.is_some()).map(|c| c.port.clone()).collect();
    for port in present {
        read(commands, state, &port);
    }
}

/// Applies one worker event, with a line on stderr. A new presence starts a read
/// of every bank.
fn handle(state: &mut AppState, event: Event, commands: &Sender<Command>) {
    match &event {
        Event::Presence { port, mode } => {
            info!("controller on {port} {}", mode.as_ref().map_or("gone", Mode::as_str));
        }
        Event::Read { port, result: Ok((model, read)) } => {
            let name = model.description().map_or("unknown model", |d| d.short_name.as_str());
            info!("read {} slots from the {name} on {port}", read.profiles.len());
        }
        Event::Read { port, result: Err(f) } => {
            warn!("read from {port} failed: {}; held by {:?}", f.error, f.holders);
        }
        Event::MacroSteps { port, result: Ok(read) } => {
            info!("read the steps of {} macros from {port}", read.len());
        }
        Event::MacroSteps { port, result: Err(f) } => {
            warn!("macro step read from {port} failed: {}; held by {:?}", f.error, f.holders);
        }
        Event::Written { port, results, .. } => {
            for result in results {
                if result.success {
                    info!("write to {port}: {}", result.message);
                } else {
                    warn!("write to {port} failed: {}", result.message);
                }
            }
        }
        Event::Locked(_) | Event::Unplugged(_) | Event::Unlock { .. } | Event::Relock { .. } => {}
        Event::Installed(Ok(())) => info!("udev rule installed"),
        Event::Installed(Err(e)) => warn!("udev rule install failed: {e}"),
        Event::Fix { port, looping } => {
            info!("controller on {port} needs the keepalive fix; it reconnected while watched: {looping}");
        }
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
        Event::Read { port, result: Ok((model, read)) } => {
            let leftover = leftover_macros(model, &read);
            let macros = macro_summaries(model, &read);
            state.set_model(&port, model);
            state.read_finished(&port, Ok(read));
            state.set_leftover(&port, leftover);
            state.set_macros(&port, macros);
            read_macro_steps(commands, state, &port);
            state.review_read(&port, true);
            let write = state.skip_empty_review();
            send_write(commands, state, write);
        }
        Event::Read { port, result: Err(Failure { error, holders }) } => {
            let message = read_failure(state, &error, &holders);
            state.read_finished(&port, Err(message));
            state.review_read(&port, false);
        }
        Event::MacroSteps { port, result: Ok(read) } => state.steps_read(&port, read),
        // The red bar's "Try again" reads every bank, then the steps still unread.
        Event::MacroSteps { port, result: Err(Failure { error, holders }) } => {
            let message = read_failure(state, &error, &holders);
            state.read_finished(&port, Err(message));
        }
        // Whatever the outcome, read the slot back: what the controller holds now is
        // what the window should show.
        // A batch is one command, so one read follows it.
        Event::Written { port, results, holders } => {
            let hint = holders::hint(&holders, state.sandboxed);
            state.write_finished(&port, &results, hint.as_deref());
            state.batch_after_write();
            read(commands, state, &port);
            // The relock the question chained: the read above goes first, then the relock.
            if let Some(port) = state.take_chained_relock() {
                state.relock_target_sent(commands, &port);
            }
        }
        // The sandbox cannot see the fix files, so it goes by the pad reconnecting.
        Event::Fix { port, looping } => {
            let offer = if state.sandboxed { looping } else { !udev::keepalive_installed() };
            state.fix_verdict(&port, offer);
        }
        Event::Locked(_) | Event::Unplugged(_) | Event::Unlock { .. } | Event::Relock { .. } => {
            lock_event(state, event, commands);
        }
        Event::Installed(result) => {
            let ok = result.is_ok();
            state.install_finished(result);
            // The rule applies to the nodes already present: read again, no replug.
            if ok {
                read_present(commands, state);
            }
        }
    }
}

/// Applies a worker event about locked controllers.
fn lock_event(state: &mut AppState, event: Event, commands: &Sender<Command>) {
    match event {
        Event::Locked(pads) => state.locked_seen(pads),
        Event::Unplugged(port) => state.unplugged(&port),
        Event::Unlock { port, result, mode } => {
            match &result {
                Ok(done) => info!("unlock of {port}: {done:?}"),
                Err(f) => warn!("unlock of {port} failed: {}; held by {:?}", f.error, f.holders),
            }
            state.unlock_finished(&port, result.as_ref());
            match result {
                Ok(Unlocked::Port(_)) => {
                    // The config port is up on the same port path: list it now.
                    state.presence(&port, mode);
                    if mode.is_some() {
                        read(commands, state, &port);
                    }
                }
                Err(Failure { error: Error::PermissionDenied(_), .. }) => {
                    state.read_denied(&port, udev::rule_state());
                }
                _ => {}
            }
        }
        Event::Relock { port, result } => match result {
            Ok(()) => {
                info!("relocked {port}");
                state.relocked(&port);
            }
            Err(f) => {
                warn!("relock of {port} failed: {}", f.error);
                state.relock_failed(&f);
            }
        },
        _ => {}
    }
}

/// After the window closes and the close question is answered: returns every unlocked
/// pad to its locked mode, one after another. A failure is logged; the process ends
/// anyway.
fn relock_on_close(state: &AppState) {
    worker::relock_all(&worker::SystemLocks, &state.relock_targets());
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
        warn!("no file dialog: {e}");
    }
}

/// The message for a dialog the portal could not show.
fn no_dialog(e: &str) -> Notice {
    Notice { error: true, title: tr!("The file dialog did not open."), body: sentence(e) }
}

/// Asks for a file and loads it into slot `slot`'s edits.
fn import(ui: Weak<AppWindow>, jobs: Sender<Job>, slot: (Mode, u8)) {
    on_thread(ui, jobs, move || {
        let read =
            chooser::open_json(&tr!("Import a profile")).map(|p| p.as_deref().map(read_file));
        Box::new(move |state: &mut AppState| match read {
            Ok(None) => {}
            Ok(Some((name, text))) => state.import(slot, &name, text),
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// Asks where to save the selected slot's profile and macros, as shown, and saves them.
/// While the macro steps are being read, it says so and opens no dialog.
fn export(ui: Weak<AppWindow>, jobs: Sender<Job>, state: &AppState) {
    let Some(slot) = state.selected_slot() else { return };
    let shown = state.slot(slot.0, slot.1);
    let Some(text) = embedded::export_file(&shown, state.macro_names()) else { return };
    let (unsaved, name) = (shown.unsaved(), files::file_name(slot.0, slot.1));
    on_thread(ui, jobs, move || {
        let saved = match text {
            Err(e) => Ok(Some(Err(e))),
            Ok(t) => chooser::save_json(&tr!("Export the profile"), &name).map(|p| {
                p.map(|p| {
                    std::fs::write(&p, t)
                        .map_err(|e| sentence(&e.to_string()))
                        .map(|()| files::display_name(&p))
                })
            }),
        };
        Box::new(move |state: &mut AppState| match saved {
            Ok(None) => {}
            Ok(Some(result)) => state.exported(slot, unsaved, result),
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// The file name and text of `path`, or why it could not be read.
fn read_file(path: &std::path::Path) -> (String, Result<String, String>) {
    (files::display_name(path), std::fs::read_to_string(path).map_err(|e| sentence(&e.to_string())))
}

/// Asks for a macro file and shows what it holds before it joins macro slot `n`.
fn import_macro(ui: Weak<AppWindow>, jobs: Sender<Job>, n: u8) {
    on_thread(ui, jobs, move || {
        let read = chooser::open_json(&tr!("Import a macro")).map(|p| p.as_deref().map(read_file));
        Box::new(move |state: &mut AppState| match read {
            Ok(None) => {}
            Ok(Some((name, text))) => {
                state.import_macro(n, &name, text);
                // Logged, so "Copy diagnostics" in the dialog carries every reason.
                if let Some(Err(problems)) = state.pending_macro_import.as_ref().map(|p| &p.outcome)
                {
                    warn!("could not import macro file {name}: {}", problems.join("; "));
                }
            }
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// Asks where to save the macro in macro slot `n`, as shown, and saves it under the
/// name the CLI's `export` gives it.
fn export_macro(ui: Weak<AppWindow>, jobs: Sender<Job>, state: &AppState, n: u8) {
    let Some((mode, number)) = state.selected_slot() else { return };
    let slot = state.slot(mode, number);
    let (Some(def), Some(default)) = (slot.shown_macro(n), state.defaults().get(&mode)) else {
        return;
    };
    let def = &named(def.clone(), state.macro_names());
    let (text, unsaved) = (macro_files::macro_text(def, &default.device), slot.macro_unsaved(n));
    let name = macro_file_name(mode, number, def);
    on_thread(ui, jobs, move || {
        let saved = chooser::save_json(&tr!("Export the macro"), &name).map(|p| {
            p.map(|p| {
                text.and_then(|t| std::fs::write(&p, t).map_err(|e| sentence(&e.to_string())))
                    .map(|()| files::display_name(&p))
            })
        });
        Box::new(move |state: &mut AppState| match saved {
            Ok(None) => {}
            Ok(Some(result)) => state.exported_macro(n, unsaved, result),
            Err(e) => state.notice = Some(no_dialog(&e)),
        })
    });
}

/// Wires Import and Export: each dialog runs on a short thread, and its answer
/// comes back through `file-done`. Returns the sender other slow jobs use to do the
/// same.
fn wire_files(ui: &AppWindow, state: &Rc<RefCell<AppState>>) -> Sender<Job> {
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
    let tx = jobs_tx.clone();
    ui.on_export_file(move || export(weak.clone(), tx.clone(), &s.borrow()));
    let (weak, tx) = (ui.as_weak(), jobs_tx.clone());
    ui.on_macro_import_file(move |n| import_macro(weak.clone(), tx.clone(), macro_slot(n)));
    let (weak, s, tx) = (ui.as_weak(), Rc::clone(state), jobs_tx.clone());
    ui.on_macro_export_file(move |n| {
        export_macro(weak.clone(), tx.clone(), &s.borrow(), macro_slot(n));
    });
    jobs_tx
}

/// Wires the keepalive fix offer: the dialog, "Not now" and the `pkexec` install, which
/// runs on a short thread like a file dialog.
fn wire_fix(
    ui: &AppWindow,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    jobs: Sender<Job>,
) {
    let c = change.clone();
    ui.on_fix_opened(move || c(&|s| s.fix_open = true));
    let c = change.clone();
    // Outside the sandbox the files can be read, so a command run in a terminal clears
    // the offer. The sandbox cannot see them: the bar stays until "Not now" or a replug.
    ui.on_fix_closed(move || c(&|s| s.fix_closed(!s.sandboxed && udev::keepalive_installed())));
    let c = change.clone();
    ui.on_fix_put_off(move || c(&AppState::fix_put_off));
    let (c, weak) = (change.clone(), ui.as_weak());
    ui.on_fix_install(move || {
        c(&AppState::fix_install_started);
        on_thread(weak.clone(), jobs.clone(), || {
            let result = udev::install_keepalive();
            Box::new(move |state: &mut AppState| state.fix_install_finished(result))
        });
    });
}

/// Wires the locked panel's "Unlock" and the Relock entry with its question.
fn wire_locks(
    ui: &AppWindow,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    commands: &Sender<Command>,
) {
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_unlock_clicked(move || {
        c(&|s| {
            if let Some(port) = s.shown_lock().map(|p| p.port.clone()) {
                s.unlock_requested(&tx, &port);
            }
        });
    });
    ui.on_guide_clicked(|| open_external(locked::GUIDE_URL.as_ref()));
    let (c, tx) = (change.clone(), commands.clone());
    ui.global::<Relock<'_>>().on_chosen(move || c(&|s| s.relock_clicked(&tx)));
    let c = change.clone();
    ui.on_relock_cancelled(move || c(&AppState::relock_cancelled));
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_relock_discard(move || c(&|s| s.relock_discarded(&tx)));
    let (c, tx) = (change.clone(), commands.clone());
    ui.on_relock_write(move || {
        c(&|s| {
            if let Some(port) = s.relock_write() {
                read(&tx, s, &port);
            }
        });
    });
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
    ui.on_controller_picked(move |i| c(&|s| s.pick_entry(usize::try_from(i).unwrap_or(0))));
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
        if !state.send(commands, Command::Write(port, jobs)) {
            state.write.running = None;
            state.write.batch = None;
        }
    }
}

/// Opens `target`, a folder or a URL, with the desktop's default program. Not a
/// hardware path, so a failure only goes to stderr.
fn open_external(target: &std::ffi::OsStr) {
    if let Err(e) = std::process::Command::new("xdg-open").arg(target).spawn() {
        warn!("could not open {}: {e}", target.to_string_lossy());
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
    let c = change.clone();
    ui.on_review_section_toggled(move |section| {
        c(&|s| s.toggle_review_section(usize::try_from(section).unwrap_or(usize::MAX)));
    });
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
    ui.on_open_backup_folder(move || open_external(backups.as_os_str()));
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
            return CloseRequestResponse::HideWindow;
        }
        // A close from the task bar reaches a window in the back: flag it there, since
        // Wayland lets no app raise itself.
        ui.window().with_winit_window(|w| {
            if !w.has_focus() {
                w.request_user_attention(Some(UserAttentionType::Informational));
            }
        });
        CloseRequestResponse::KeepWindowShown
    });
    let c = change.clone();
    ui.on_close_cancelled(move || c(&AppState::cancel_close));
    let c = change.clone();
    ui.on_stopped_closed(move || c(&|s| s.worker_stopped = false));
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
                warn!("could not close the window: {e}");
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
    ui.on_import_without_macros(move || c(&AppState::import_without_macros));
    let c = change.clone();
    ui.on_macro_import_confirmed(move || c(&AppState::confirm_macro_import));
    let c = change.clone();
    ui.on_macro_import_cancelled(move || c(&AppState::cancel_macro_import));
    let c = change.clone();
    ui.on_notice_closed(move || c(&|s| s.notice = None));
    let c = change.clone();
    ui.on_discard(move || c(&AppState::discard));
    let c = change.clone();
    ui.on_start_from_default(move || c(&AppState::start_from_default));
}

/// The macro slot a Macros tab callback names.
fn macro_slot(n: i32) -> u8 {
    u8::try_from(n).unwrap_or(u8::MAX)
}

/// Wires what the controller views on the Macros tab ask of `state`: the button a click
/// toggles, and where an arrow key moves the focus. Both read `state` outside a change.
fn wire_views(ui: &AppWindow, state: Rc<RefCell<AppState>>) {
    let neighbour_state = Rc::clone(&state);
    ui.on_macro_hit(move |view, n, step, x, y| {
        let view = usize::try_from(view).unwrap_or(usize::MAX);
        let step = usize::try_from(step).unwrap_or(usize::MAX);
        action_hit(&state.borrow(), view, macro_slot(n), step, x, y)
    });
    ui.on_macro_neighbour(move |view, from, dir| {
        let view = usize::try_from(view).unwrap_or(usize::MAX);
        view_neighbour(&neighbour_state.borrow(), view, from, dir)
    });
}

/// Wires the stick edits of the step panel: each names the macro slot, the step and the
/// stick.
fn wire_sticks(ui: &AppWindow, change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static)) {
    let c = change.clone();
    ui.on_stick_set(move |n, step, stick, x, y, apart| {
        let (Ok(step), Ok(stick)) = (usize::try_from(step), usize::try_from(stick)) else { return };
        c(&|s| s.set_stick(macro_slot(n), step, stick, [x, y], apart));
    });
    let c = change.clone();
    ui.on_stick_dragged(move |n, step, stick, x, y, snap| {
        let (Ok(step), Ok(stick)) = (usize::try_from(step), usize::try_from(stick)) else { return };
        c(&|s| s.drag_stick(macro_slot(n), step, stick, [x, y], snap));
    });
    let c = change.clone();
    ui.on_stick_glided(move |n, step, stick, on| {
        let (Ok(step), Ok(stick)) = (usize::try_from(step), usize::try_from(stick)) else { return };
        c(&|s| s.set_stick_glide(macro_slot(n), step, stick, on));
    });
    let c = change.clone();
    ui.on_stick_nudged(move |n, step, stick, dx, dy| {
        let (Ok(step), Ok(stick)) = (usize::try_from(step), usize::try_from(stick)) else { return };
        c(&|s| s.nudge_stick(macro_slot(n), step, stick, dx, dy));
    });
}

/// Wires the Macros tab: each edit names the macro slot it is for.
fn wire_macros(
    ui: &AppWindow,
    change: &(impl Fn(&dyn Fn(&mut AppState)) + Clone + 'static),
    state: Rc<RefCell<AppState>>,
) {
    wire_views(ui, state);
    let c = change.clone();
    ui.on_macro_shown(move || c(&|_| {}));
    let c = change.clone();
    let weak = ui.as_weak();
    ui.on_step_edited(move |n, step, edit| {
        let (Some(edit), Ok(step)) = (StepEdit::at(edit), usize::try_from(step)) else { return };
        let selected = Cell::new(None);
        c(&|s| selected.set(s.edit_step(macro_slot(n), step, edit)));
        if let (Some(i), Some(ui)) = (selected.get(), weak.upgrade()) {
            ui.set_selected_step(i32::try_from(i).unwrap_or(0));
        }
    });
    let c = change.clone();
    let weak = ui.as_weak();
    ui.on_step_undone(move |n| {
        let selected = Cell::new(None);
        c(&|s| selected.set(s.undo_step(macro_slot(n))));
        if let (Some(i), Some(ui)) = (selected.get(), weak.upgrade()) {
            ui.set_selected_step(i32::try_from(i).unwrap_or(0));
        }
    });
    let c = change.clone();
    ui.on_step_duration_changed(move |n, step, ms| {
        let Ok(step) = usize::try_from(step) else { return };
        c(&|s| s.set_step_duration(macro_slot(n), step, ms));
    });
    let c = change.clone();
    ui.on_step_chip_toggled(move |n, step, chip| {
        let (Ok(step), Ok(chip)) = (usize::try_from(step), usize::try_from(chip)) else { return };
        c(&|s| s.toggle_step_button(macro_slot(n), step, chip));
    });
    wire_sticks(ui, change);
    let c = change.clone();
    ui.on_trigger_set(move |n, step, k, v, apart| {
        let (Ok(step), Ok(k)) = (usize::try_from(step), usize::try_from(k)) else { return };
        c(&|s| s.set_trigger(macro_slot(n), step, k, v, apart));
    });
    let c = change.clone();
    ui.on_trigger_glided(move |n, step, k, on| {
        let (Ok(step), Ok(k)) = (usize::try_from(step), usize::try_from(k)) else { return };
        c(&|s| s.set_trigger_glide(macro_slot(n), step, k, on));
    });
    let c = change.clone();
    ui.on_floor_raised(move |n| c(&|s| s.raise_to_stick_floor(macro_slot(n))));
    let c = change.clone();
    ui.on_macro_created(move |n| c(&|s| s.new_macro(macro_slot(n))));
    let c = change.clone();
    ui.on_macro_reverted(move |n| c(&|s| s.revert_macro(macro_slot(n))));
    let c = change.clone();
    ui.on_macro_deleted(move |n| c(&|s| s.delete_macro(macro_slot(n))));
    let c = change.clone();
    ui.on_macro_renamed(move |n, name| c(&|s| s.rename_macro(macro_slot(n), &name)));
    let c = change.clone();
    ui.on_macro_plays_picked(move |n, p| {
        if let Some(plays) = Plays::at(p) {
            c(&|s| s.set_plays(macro_slot(n), plays));
        }
    });
    let c = change.clone();
    ui.on_macro_count_changed(move |n, v| c(&|s| s.set_play_count(macro_slot(n), v)));
    let c = change.clone();
    ui.on_macro_pause_changed(move |n, v| c(&|s| s.set_play_pause(macro_slot(n), v)));
    let c = change.clone();
    ui.on_macro_button_added(move |n, choice| {
        let Ok(choice) = usize::try_from(choice) else { return };
        c(&|s| {
            if let Some(button) = added_button(s, macro_slot(n), choice) {
                s.add_macro_button(macro_slot(n), &button);
            }
        });
    });
    let c = change.clone();
    ui.on_macro_button_removed(move |n, chip| {
        let Ok(chip) = usize::try_from(chip) else { return };
        c(&|s| {
            if let Some(button) = chip_button(s, macro_slot(n), chip) {
                s.remove_macro_button(macro_slot(n), &button);
            }
        });
    });
}

/// Answers the window's globals: the diagnostics text and a screen reader's numbers.
fn wire_globals(ui: &AppWindow) {
    ui.global::<Diagnostics<'_>>().on_text(|| logging::diagnostics().into());
    render::read_accessible_numbers(ui);
}

#[allow(clippy::too_many_lines)] // the one place that wires the window
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let sandboxed = udev::sandboxed();
    logging::init(sandboxed);
    let model = *devices::models().first().ok_or("no controller model is built in")?;
    let description = model.description()?;
    let ui = AppWindow::new()?;
    // Wayland matches the window to its `.desktop` file by this ID, for the task bar icon.
    // Slint has no platform until the first window exists, and the ID must precede `show`.
    slint::set_xdg_app_id("io.github.LucasPicoli._8B")?;
    let defaults = description.modes.iter().map(|m| (m.id, model.default_profile(m.id))).collect();
    let mut first = AppState::new(description, defaults);
    first.sandboxed = sandboxed;
    // The sandbox cannot see the host's rule files: ask only when a read is denied.
    first.rule = if sandboxed { Rule::Current } else { udev::rule_state() };
    let state = Rc::new(RefCell::new(first));
    language::wire(&state, &ui);
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
        Box::new(|port: &str| devices::open(Some(port))),
        PathBuf::from(worker::SYSFS_USB),
        devices::config_ports(),
        worker::Hooks::system(),
        backups.clone(),
        sandboxed,
        move |event| {
            if events_tx.send(event).is_ok() {
                let _ = weak.upgrade_in_event_loop(|ui| ui.invoke_worker_event());
            }
        },
    )?;
    let (install_tx, check_tx, retry_tx, write_tx, close_tx, lock_tx) = (
        commands.clone(),
        commands.clone(),
        commands.clone(),
        commands.clone(),
        commands.clone(),
        commands.clone(),
    );

    let weak = ui.as_weak();
    let s = Rc::clone(&state);
    ui.on_worker_event(move || {
        let Some(ui) = weak.upgrade() else { return };
        let mut state = s.borrow_mut();
        while let Ok(event) = events.try_recv() {
            handle(&mut state, event, &commands);
        }
        state.settle_close();
        state.settle_relock();
        render(&state, &ui);
    });

    let jobs = wire_files(&ui, &state);

    // The close check reads the state outside a change.
    let close_state = Rc::clone(&state);
    // So do the hit test and the theme change, for the shown controller's views.
    let hit_state = Rc::clone(&state);
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
            if s.send(&install_tx, Command::InstallUdevRule) {
                s.install_started();
            }
        });
    });
    let c = change.clone();
    ui.on_check_again(move || {
        c(&|s| {
            s.check_started();
            s.recheck_locks(&check_tx);
            read_present(&check_tx, s);
        });
    });
    wire_controllers(&ui, &change, retry_tx);
    wire_writes(&ui, &change, &write_tx, backups);
    wire_close(&ui, &close_state, &change, close_tx);
    wire_locks(&ui, &change, &lock_tx);
    let c = change.clone();
    ui.on_rule_skipped(move || c(&|s| s.rule_skipped = true));
    wire_edits(&ui, &change);
    wire_macros(&ui, &change, Rc::clone(&hit_state));
    wire_fix(&ui, &change, jobs);

    let s = Rc::clone(&hit_state);
    ui.on_hit(move |view, x, y| {
        hit(s.borrow().description(), usize::try_from(view).unwrap_or(usize::MAX), x, y)
    });

    wire_globals(&ui);

    let weak = ui.as_weak();
    ui.on_theme_changed(move || {
        if let Some(ui) = weak.upgrade() {
            render_views(hit_state.borrow().description(), &ui);
        }
    });

    ui.run()?;
    relock_on_close(&close_state.borrow());
    Ok(())
}
