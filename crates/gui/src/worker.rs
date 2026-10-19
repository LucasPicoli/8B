//! The worker thread: the one owner of the controllers. It runs commands one at a
//! time, and between them it watches sysfs for controllers coming and going, each
//! keyed by its USB port path.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use controller_core::detect::{config_hidraw, scan_sysfs_all};
use controller_core::device::{ConfigPort, Model};
use controller_core::devices::{locked_ports, LockedController};
use controller_core::model::{Mode, ProfileReadResult, WriteResult};
use controller_core::service::read::read_macro_steps;
use controller_core::transport::lock::{self, Unlocked};
use controller_core::transport::DeviceIo;
use controller_core::Error;
use log::{info, warn};

use crate::holders::{holders, PROC};
use crate::keepalive::Keepalive;
use crate::writes::{self, WriteJob};

/// Where the kernel lists USB devices.
pub const SYSFS_USB: &str = "/sys/bus/usb/devices";

/// How often the worker looks for controllers while idle.
const POLL: Duration = Duration::from_millis(500);

/// How often the worker tries again to hold the event node of a pad whose first try
/// failed, between polls. See [`Keepalive::pending`].
const HOLD_RETRY: Duration = Duration::from_millis(50);

/// Polls in a row that must agree before a presence change is reported, about 2 s.
/// A slide-switch move re-enumerates the controller several times in about a
/// second, and a kernel driver may probe the new USB id first: a read sent 1 s after
/// the Switch id appeared, during the hid-nintendo probe, got no reply.
const SETTLE_POLLS: u8 = 4;

/// How long a failed read waits before its one retry. A program that held the
/// controller for a moment, as Wine does when Steam starts, may have let go by then.
const RETRY_AFTER: Duration = Duration::from_secs(2);

/// One sighting of the controller: its current mode and its USB device number. A
/// replug gets a new device number, so even a fast one counts as a new presence.
type Sighting = (Mode, Option<u16>);

/// The USB device number the kernel gave the device at `sysfs_path`.
fn devnum(sysfs_path: &str) -> Option<u16> {
    std::fs::read_to_string(Path::new(sysfs_path).join("devnum")).ok()?.trim().parse().ok()
}

/// Work for the worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Read every bank of the controller on this USB port path.
    ReadAll(String),
    /// Read the steps of these macros, each with its mode and 1-based slot, from the
    /// controller on this USB port path, in one session.
    ReadMacroSteps(String, Vec<MacroAt>),
    /// Write these slots of the controller on this USB port path, in one session.
    Write(String, Vec<WriteJob>),
    /// Install the udev rule that grants access to the controller.
    InstallUdevRule,
    /// Unlock the locked controller on this USB port path. Unlocks queue behind
    /// whatever the worker is doing, one pad at a time.
    Unlock(String),
    /// Return an unlocked controller to its locked mode.
    Relock(Target),
}

/// A controller on a USB port path, with its model.
#[derive(Debug, Clone)]
pub struct Target {
    /// The USB port path, such as `8-5`.
    pub port: String,
    /// The model of the controller.
    pub model: &'static dyn Model,
}

impl PartialEq for Target {
    fn eq(&self, other: &Self) -> bool {
        self.port == other.port && std::ptr::addr_eq(self.model, other.model)
    }
}

impl Eq for Target {}

/// What the worker does to locked controllers. [`SystemLocks`] talks to the real bus; a
/// test fake answers from a script.
pub trait Locks: Send {
    /// The locked controllers attached now, sorted by port path.
    fn locked(&self) -> Vec<LockedController>;
    /// Unlocks `locked`; see [`controller_core::transport::lock::unlock`].
    ///
    /// # Errors
    /// As the core call.
    fn unlock(&self, locked: &LockedController) -> Result<Unlocked, Error>;
    /// Relocks the controller of `model` on `port`; see
    /// [`controller_core::transport::lock::relock`].
    ///
    /// # Errors
    /// As the core call.
    fn relock(&self, model: &dyn Model, port: &str) -> Result<(), Error>;
}

/// The real bus.
#[derive(Debug, Clone, Copy)]
pub struct SystemLocks;

impl Locks for SystemLocks {
    fn locked(&self) -> Vec<LockedController> {
        locked_ports()
    }

    fn unlock(&self, locked: &LockedController) -> Result<Unlocked, Error> {
        lock::unlock(locked)
    }

    fn relock(&self, model: &dyn Model, port: &str) -> Result<(), Error> {
        lock::relock(model, port)
    }
}

/// Relocks each of `targets` in turn, waiting for one before it starts the next. A
/// failure is logged and the rest still run, so a close never waits on a stuck pad.
pub fn relock_all(locks: &dyn Locks, targets: &[Target]) {
    for t in targets {
        match locks.relock(t.model, &t.port) {
            Ok(()) => info!("relocked {}", t.port),
            Err(e) => warn!("relock of {} failed: {e}", t.port),
        }
    }
}

pub use controller_core::service::read::MacroAt;

/// A read that failed, retry included.
#[derive(Debug)]
pub struct Failure {
    /// Why the read failed.
    pub error: Error,
    /// The other programs that have the controller's config node open.
    pub holders: Vec<String>,
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Event {
    /// The controller on `port` appeared in a current mode, or went away (`None`).
    Presence {
        /// The USB port path, such as `8-5`.
        port: String,
        /// The current mode, `None` once gone.
        mode: Option<Mode>,
    },
    /// The result of [`Command::ReadAll`] for `port`.
    Read {
        /// The USB port path the read went to.
        port: String,
        /// The model the controller named and the read, or why it failed.
        result: Result<(&'static dyn Model, ProfileReadResult), Failure>,
    },
    /// The result of [`Command::ReadMacroSteps`] for `port`: the macros with their
    /// steps, or why the read failed.
    MacroSteps {
        /// The USB port path the read went to.
        port: String,
        /// The macros of the command with their steps read, or why the read failed.
        result: Result<Vec<MacroAt>, Failure>,
    },
    /// The result of [`Command::Write`] for `port`.
    Written {
        /// The USB port path the write went to.
        port: String,
        /// How each write went, in the order of the command, rollback included.
        results: Vec<WriteResult>,
        /// When it failed: the other programs that have the controller's config node
        /// open.
        holders: Vec<String>,
    },
    /// The locked controllers attached changed: this is the whole list now.
    Locked(Vec<Target>),
    /// The result of [`Command::Unlock`] for `port`. Success is the config port, or
    /// the USB id of a pad no description lists.
    Unlock {
        /// The USB port path.
        port: String,
        /// How it went. On failure the holders are the programs with the pad's USB node
        /// open.
        result: Result<Unlocked, Failure>,
        /// On success with a config port: its mode, so the window lists the pad without
        /// waiting for a settled sighting.
        mode: Option<Mode>,
    },
    /// A pad that came back from an unlock as an unsupported USB id left the bus: nothing
    /// is on its port path any more.
    Unplugged(
        /// The USB port path.
        String,
    ),
    /// The result of [`Command::Relock`] for `port`. On success the pad is gone from
    /// its config port, and shows in [`Event::Locked`] next.
    Relock {
        /// The USB port path.
        port: String,
        /// How it went.
        result: Result<(), Failure>,
    },
    /// The result of [`Command::InstallUdevRule`]: why it failed, as a sentence.
    Installed(Result<(), String>),
    /// A controller whose description needs the keepalive fix is connected, and 8B has
    /// watched it. Sent once per connection.
    Fix {
        /// The USB port path.
        port: String,
        /// The controller reconnected while 8B watched it (sandbox only).
        looping: bool,
    },
}

/// Installs the udev rule; [`crate::udev::install_rule`] outside tests.
pub type Installer = fn() -> Result<(), String>;

/// What the worker calls out to besides the controllers' transports.
pub struct Hooks {
    /// Installs the udev rule.
    pub install: Installer,
    /// Finds, unlocks and relocks locked controllers.
    pub locks: Box<dyn Locks>,
}

/// The device that talks to the controller on a USB port path.
pub type Opener = Box<dyn Fn(&str) -> Box<dyn DeviceIo + Send> + Send>;

impl Hooks {
    /// The udev rule installer and the real bus.
    #[must_use]
    pub fn system() -> Self {
        Self { install: crate::udev::install_rule, locks: Box::new(SystemLocks) }
    }
}

/// Starts the worker on its own thread. `emit` runs on that thread for each event.
/// A failed write that cannot be undone saves the old profile in `backup_dir`. The
/// worker stops when the returned sender is dropped.
///
/// # Errors
/// Returns the OS error if the thread cannot be started.
pub fn spawn(
    open: Opener,
    sysfs: PathBuf,
    ports: Vec<ConfigPort>,
    hooks: Hooks,
    backup_dir: PathBuf,
    sandboxed: bool,
    emit: impl Fn(Event) + Send + 'static,
) -> io::Result<Sender<Command>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new().name("8b-worker".to_owned()).spawn(move || {
        run(
            &open,
            &rx,
            &Site { sysfs: &sysfs, ports: &ports, backup_dir: &backup_dir, sandboxed },
            &hooks,
            &emit,
        );
    })?;
    Ok(tx)
}

/// Where the worker looks for controllers, and where it keeps backups.
struct Site<'a> {
    sysfs: &'a Path,
    ports: &'a [ConfigPort],
    backup_dir: &'a Path,
    /// In the Flatpak sandbox `/proc` shows no other program, so it is not scanned.
    sandboxed: bool,
}

/// The other programs that have the config node of the controller on `port` open.
fn holders_of(site: &Site<'_>, port: &str) -> Vec<String> {
    if site.sandboxed {
        return Vec::new();
    }
    scan_sysfs_all(site.sysfs, site.ports)
        .into_iter()
        .find(|usb| usb.port_path() == port)
        .and_then(|usb| config_hidraw(Path::new(&usb.sysfs_path), usb.port.interface))
        .map(|node| holders(Path::new(PROC), &node, std::process::id()))
        .unwrap_or_default()
}

fn run(
    open: &Opener,
    rx: &Receiver<Command>,
    site: &Site<'_>,
    hooks: &Hooks,
    emit: &dyn Fn(Event),
) {
    let (sysfs, ports) = (site.sysfs, site.ports);
    let mut devices: BTreeMap<String, Box<dyn DeviceIo + Send>> = BTreeMap::new();
    let mut presence: BTreeMap<String, Debounce> = BTreeMap::new();
    let mut keepalive = Keepalive::new(site.sandboxed);
    let mut locked_seen: Vec<Target> = Vec::new();
    // Ports whose pad came back from an unlock as an id no description lists.
    let mut unsupported: BTreeSet<String> = BTreeSet::new();
    let mut next_poll = Instant::now() + POLL;
    loop {
        let wait = if keepalive.pending() { HOLD_RETRY } else { POLL };
        match rx.recv_timeout(wait.min(next_poll.saturating_duration_since(Instant::now()))) {
            Ok(Command::ReadAll(port)) => {
                let dev = devices.entry(port.clone()).or_insert_with(|| open(&port));
                match read_with_retry(dev.as_ref()) {
                    // The controller went away mid-read, as a slide-switch move does
                    // while it settles. Not an error: report it gone, and the next
                    // settled presence starts a new read.
                    Err(Error::Disconnected) => {
                        presence.entry(port.clone()).or_default().forget();
                        emit(Event::Presence { port, mode: None });
                    }
                    Ok(read) => match dev.model() {
                        Ok(model) => emit(Event::Read { port, result: Ok((model, read)) }),
                        Err(error) => {
                            let holders = holders_of(site, &port);
                            emit(Event::Read { port, result: Err(Failure { error, holders }) });
                        }
                    },
                    Err(error) => {
                        let holders = holders_of(site, &port);
                        emit(Event::Read { port, result: Err(Failure { error, holders }) });
                    }
                }
            }
            Ok(Command::ReadMacroSteps(port, macros)) => {
                let dev = devices.entry(port.clone()).or_insert_with(|| open(&port));
                match read_macro_steps(dev.as_ref(), macros) {
                    // Gone mid-read, as for a read of every bank.
                    Err(Error::Disconnected) => {
                        presence.entry(port.clone()).or_default().forget();
                        emit(Event::Presence { port, mode: None });
                    }
                    result => {
                        let result = result
                            .map_err(|error| Failure { holders: holders_of(site, &port), error });
                        emit(Event::MacroSteps { port, result });
                    }
                }
            }
            Ok(Command::Write(port, jobs)) => {
                let dev = devices.entry(port.clone()).or_insert_with(|| open(&port));
                let results = writes::run(dev.as_ref(), &jobs, site.backup_dir);
                // The write may have flipped the controller and back, which gave it a
                // new device number. This write caused that: do not report it as a
                // replug, or the window reads the controller a second time.
                mark_seen(&mut presence, site, &port);
                let failed = results.iter().any(|r| !r.success);
                let holders = if failed { holders_of(site, &port) } else { Vec::new() };
                emit(Event::Written { port, results, holders });
            }
            Ok(Command::InstallUdevRule) => emit(Event::Installed((hooks.install)())),
            Ok(command @ (Command::Unlock(_) | Command::Relock(_))) => {
                lock_command(
                    command,
                    hooks,
                    site,
                    (&mut devices, &mut presence, &mut unsupported),
                    emit,
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                let found = scan_sysfs_all(sysfs, ports);
                if Instant::now() < next_poll {
                    keepalive.hold(&found, |usb| devnum(&usb.sysfs_path));
                    continue;
                }
                next_poll = Instant::now() + POLL;
                poll_locks(hooks, sysfs, (&mut locked_seen, &mut unsupported), emit);
                for v in keepalive.sync(&found, |usb| devnum(&usb.sysfs_path)) {
                    emit(Event::Fix { port: v.port, looping: v.looping });
                }
                // A transport that knows the pad's mode beats the USB id, for a pad whose
                // modes share one id.
                let seen: BTreeMap<String, Sighting> = found
                    .iter()
                    .map(|usb| {
                        let port = usb.port_path();
                        let told = devices.get(port).and_then(|d| d.current_mode());
                        let mode = told.unwrap_or(usb.port.mode);
                        (port.to_owned(), (mode, devnum(&usb.sysfs_path)))
                    })
                    .collect();
                for port in seen.keys() {
                    presence.entry(port.clone()).or_default();
                }
                presence.retain(|port, debounce| {
                    let sighting = seen.get(port).copied();
                    if debounce.observe(sighting) {
                        emit(Event::Presence { port: port.clone(), mode: sighting.map(|s| s.0) });
                    }
                    sighting.is_some() || debounce.reported.is_some()
                });
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// The open devices by USB port path.
type Devices = BTreeMap<String, Box<dyn DeviceIo + Send>>;

/// What each port path last reported.
type Presences = BTreeMap<String, Debounce>;

/// Lists the locked controllers when they changed, and reports each unsupported pad whose
/// port path emptied. One that came back as a locked pad was replugged: the window
/// restarts it from the locked list, so no event.
fn poll_locks(
    hooks: &Hooks,
    sysfs: &Path,
    (locked_seen, unsupported): (&mut Vec<Target>, &mut BTreeSet<String>),
    emit: &dyn Fn(Event),
) {
    let locked = locked_targets(hooks.locks.as_ref());
    unsupported.retain(|port| {
        let back = locked.iter().any(|t| &t.port == port);
        let there = sysfs.join(port).exists();
        if !there && !back {
            emit(Event::Unplugged(port.clone()));
        }
        there && !back
    });
    if locked != *locked_seen {
        locked_seen.clone_from(&locked);
        emit(Event::Locked(locked));
    }
}

/// Runs an unlock or a relock. Either moves the pad to another USB id on the same port
/// path, which is not a replug: the presence is taken as it is after.
fn lock_command(
    command: Command,
    hooks: &Hooks,
    site: &Site<'_>,
    (devices, presence, unsupported): (&mut Devices, &mut Presences, &mut BTreeSet<String>),
    emit: &dyn Fn(Event),
) {
    match command {
        Command::Unlock(port) => {
            devices.remove(&port);
            let result = unlock(hooks, site, &port);
            let mode = mark_seen(presence, site, &port);
            if matches!(result, Ok(Unlocked::Unsupported(_))) {
                unsupported.insert(port.clone());
            }
            emit(Event::Unlock { port, result, mode });
        }
        Command::Relock(target) => {
            devices.remove(&target.port);
            let result = hooks
                .locks
                .relock(target.model, &target.port)
                .map_err(|error| Failure { holders: holders_of(site, &target.port), error });
            if result.is_ok() {
                mark_seen(presence, site, &target.port);
            }
            emit(Event::Relock { port: target.port, result });
        }
        _ => {}
    }
}

/// The locked controllers attached, as targets.
fn locked_targets(locks: &dyn Locks) -> Vec<Target> {
    locks.locked().into_iter().map(|l| Target { port: l.port_path, model: l.model }).collect()
}

/// Unlocks the controller on `port`. A pad that left the bus is `Disconnected`.
fn unlock(hooks: &Hooks, site: &Site<'_>, port: &str) -> Result<Unlocked, Failure> {
    let Some(locked) = hooks.locks.locked().into_iter().find(|l| l.port_path == port) else {
        return Err(Failure { error: Error::Disconnected, holders: Vec::new() });
    };
    hooks.locks.unlock(&locked).map_err(|error| {
        let holders = if site.sandboxed {
            Vec::new()
        } else {
            holders(Path::new(PROC), &locked.usb_node, std::process::id())
        };
        Failure { error, holders }
    })
}

/// Takes the controller on `port` as reported as it is now, so a change the worker
/// itself caused is no presence change. Returns its mode in a config port, if any.
fn mark_seen(presence: &mut Presences, site: &Site<'_>, port: &str) -> Option<Mode> {
    let seen = scan_sysfs_all(site.sysfs, site.ports)
        .iter()
        .find(|usb| usb.port_path() == port)
        .map(|usb| (usb.port.mode, devnum(&usb.sysfs_path)));
    presence.entry(port.to_owned()).or_default().reported = seen;
    seen.map(|s| s.0)
}

/// Reads every bank. A failure that may pass, such as a timeout, is tried once more
/// after [`RETRY_AFTER`].
fn read_with_retry(dev: &dyn DeviceIo) -> Result<ProfileReadResult, Error> {
    match dev.read_all_profiles() {
        Err(e @ (Error::Timeout | Error::Usb(_) | Error::Io(_) | Error::Decode(_))) => {
            warn!("read failed: {e}; trying again");
            thread::sleep(RETRY_AFTER);
            dev.read_all_profiles()
        }
        result => result,
    }
}

/// Turns raw polls of one port into presence changes, reported once
/// [`SETTLE_POLLS`] polls in a row agree. Starts as "absent", so a missing
/// controller reports nothing.
#[derive(Debug, Default)]
struct Debounce {
    reported: Option<Sighting>,
    candidate: Option<Sighting>,
    agreeing: u8,
}

impl Debounce {
    /// Treats the controller as absent, so its next settled sighting reports again.
    const fn forget(&mut self) {
        self.reported = None;
        self.agreeing = 0;
    }

    /// Feeds one poll; returns whether `seen` is now the reported presence.
    fn observe(&mut self, seen: Option<Sighting>) -> bool {
        if seen == self.reported {
            self.agreeing = 0;
            return false;
        }
        if seen == self.candidate && self.agreeing > 0 {
            self.agreeing += 1;
        } else {
            self.candidate = seen;
            self.agreeing = 1;
        }
        if self.agreeing < SETTLE_POLLS {
            return false;
        }
        self.reported = seen;
        self.agreeing = 0;
        true
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]
mod tests {
    /// The registered Ultimate.
    fn ultimate_model() -> &'static dyn controller_core::device::Model {
        controller_core::devices::model_named("Ultimate").unwrap()
    }
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

    use controller_core::device::{ControllerSpec as _, ProtocolCodec as _};
    use controller_core::devices::pro3::{Pro3, DINPUT, XINPUT};
    use controller_core::model::{MacroDefinition, MacroSlot, Slot};
    use controller_core::transport::mock::MockOp;
    use controller_core::transport::MockDevice;

    use super::*;
    use crate::writes::WriteOp;

    /// Feeds `seen` until the debounce reports it; returns the polls it took.
    fn settle(d: &mut Debounce, seen: Option<Sighting>) -> u8 {
        (1..=u8::MAX).find(|_| d.observe(seen)).unwrap()
    }

    #[test]
    fn debounce_needs_settle_polls_in_a_row() {
        let mut d = Debounce::default();
        assert!(!d.observe(None));
        let x = Some((XINPUT, Some(5)));
        for _ in 1..SETTLE_POLLS {
            assert!(!d.observe(x));
        }
        assert!(!d.observe(None), "a gap restarts the count");
        assert_eq!(settle(&mut d, x), SETTLE_POLLS);
        assert!(!d.observe(x));
        // A slide-switch move: a gap, then the new mode.
        assert!(!d.observe(None));
        assert_eq!(settle(&mut d, Some((DINPUT, Some(6)))), SETTLE_POLLS);
        assert_eq!(settle(&mut d, None), SETTLE_POLLS);
    }

    /// The port path [`plug`] uses.
    const PORT: &str = "3-1";

    /// Opens `dev` for the first port asked for, and an empty mock after that.
    fn given(dev: MockDevice) -> Opener {
        let dev = Mutex::new(Some(dev));
        Box::new(move |_| Box::new(dev.lock().unwrap().take().unwrap_or_default()))
    }

    /// A fake `/sys/bus/usb/devices` holding one USB device on [`PORT`].
    fn plug(root: &Path, vendor: &str, product: &str) {
        plug_at(root, PORT, vendor, product);
    }

    /// Adds a USB device on port path `port` to a fake `/sys/bus/usb/devices`.
    fn plug_at(root: &Path, port: &str, vendor: &str, product: &str) {
        let dev = root.join(port);
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("idVendor"), format!("{vendor}\n")).unwrap();
        fs::write(dev.join("idProduct"), format!("{product}\n")).unwrap();
    }

    #[test]
    fn a_replug_faster_than_the_debounce_is_a_new_presence() {
        let sysfs = tempfile::tempdir().unwrap();
        plug(sysfs.path(), "2dc8", "310b");
        fs::write(sysfs.path().join("3-1/devnum"), "44\n").unwrap();
        let ports = Pro3.description().unwrap().config_ports.clone();
        let (etx, events) = mpsc::channel();
        let _tx = spawn(
            given(MockDevice::new()),
            sysfs.path().to_owned(),
            ports,
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(XINPUT), .. }));
        // Unplugged and back between two polls: the kernel gave it a new number.
        fs::write(sysfs.path().join("3-1/devnum"), "45\n").unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(XINPUT), .. }));
    }

    /// An installer that fails, as a closed password prompt does.
    #[allow(clippy::unnecessary_wraps)]
    fn refused() -> Result<(), String> {
        Err("The password prompt was closed.".to_owned())
    }

    /// An installer that succeeds without touching the system.
    #[allow(clippy::unnecessary_wraps)]
    const fn installed() -> Result<(), String> {
        Ok(())
    }

    /// A scripted bus: the locked pads it lists, how each unlock ends, and every
    /// relock it was asked for, in order.
    #[derive(Default)]
    struct FakeLocks {
        locked: Mutex<Vec<LockedController>>,
        /// The answers to the next unlocks, first first.
        unlocks: Mutex<Vec<Result<Unlocked, Error>>>,
        /// The ports relocked, in order; a port in `stuck` fails.
        relocks: Arc<Mutex<Vec<String>>>,
        stuck: Vec<String>,
    }

    impl Locks for FakeLocks {
        fn locked(&self) -> Vec<LockedController> {
            self.locked.lock().unwrap().clone()
        }

        fn unlock(&self, locked: &LockedController) -> Result<Unlocked, Error> {
            // The pad leaves the locked list as it unlocks.
            self.locked.lock().unwrap().retain(|l| l.port_path != locked.port_path);
            self.unlocks.lock().unwrap().remove(0)
        }

        fn relock(&self, _: &dyn Model, port: &str) -> Result<(), Error> {
            self.relocks.lock().unwrap().push(port.to_owned());
            if self.stuck.iter().any(|p| p == port) {
                Err(Error::Timeout)
            } else {
                Ok(())
            }
        }
    }

    fn locked_pad(port: &str) -> LockedController {
        LockedController {
            model: ultimate_model(),
            port_path: port.to_owned(),
            usb_node: PathBuf::from("/dev/bus/usb/001/002"),
        }
    }

    fn hooks() -> Hooks {
        Hooks { install: installed, locks: Box::new(FakeLocks::default()) }
    }

    /// A worker over an empty sysfs with `locks`; returns its commands and events.
    fn locking(locks: FakeLocks, sysfs: &Path) -> (Sender<Command>, Receiver<Event>) {
        let ports = ultimate_model().description().unwrap().config_ports.clone();
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(MockDevice::new()),
            sysfs.to_owned(),
            ports,
            Hooks { install: installed, locks: Box::new(locks) },
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        (tx, events)
    }

    fn scripted(pads: &[&str], unlocks: Vec<Result<Unlocked, Error>>) -> FakeLocks {
        FakeLocks {
            locked: Mutex::new(pads.iter().map(|p| locked_pad(p)).collect()),
            unlocks: Mutex::new(unlocks),
            ..FakeLocks::default()
        }
    }

    #[test]
    fn a_locked_pad_is_listed_and_a_second_one_after_it() {
        let sysfs = tempfile::tempdir().unwrap();
        let (_tx, events) = locking(scripted(&["1-2", "1-4"], vec![]), sysfs.path());
        match next(&events) {
            Event::Locked(pads) => {
                let ports: Vec<_> = pads.iter().map(|t| t.port.as_str()).collect();
                assert_eq!(ports, ["1-2", "1-4"]);
            }
            other => panic!("expected the locked list, got {other:?}"),
        }
    }

    #[test]
    fn an_unlock_that_finds_the_config_port_is_not_a_replug() {
        let sysfs = tempfile::tempdir().unwrap();
        let (tx, events) = locking(
            scripted(&["3-1"], vec![Ok(Unlocked::Port(PathBuf::from("/dev/hidraw9")))]),
            sysfs.path(),
        );
        assert!(matches!(next(&events), Event::Locked(_)));
        // The pad shows its config port at once, as the unlock leaves it.
        plug(sysfs.path(), "2dc8", "6007");
        fs::write(sysfs.path().join("3-1/devnum"), "9\n").unwrap();
        tx.send(Command::Unlock(PORT.to_owned())).unwrap();
        loop {
            match next(&events) {
                Event::Unlock { result: Ok(Unlocked::Port(_)), mode, .. } => {
                    assert!(mode.is_some());
                    break;
                }
                Event::Locked(_) => {}
                other => panic!("expected the unlock, got {other:?}"),
            }
        }
        // No presence event follows for the same sighting: the next event is the end of
        // the quiet, here a timeout.
        let later = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(3)).ok())
            .any(|e| matches!(e, Event::Presence { .. }));
        assert!(!later, "an unlock is not a replug");
    }

    #[test]
    fn a_failed_unlock_comes_back_with_its_error() {
        let sysfs = tempfile::tempdir().unwrap();
        let (tx, events) = locking(scripted(&["3-1"], vec![Err(Error::Timeout)]), sysfs.path());
        tx.send(Command::Unlock("3-1".to_owned())).unwrap();
        loop {
            match next(&events) {
                Event::Unlock { result: Err(f), mode, .. } => {
                    assert!(matches!(f.error, Error::Timeout));
                    assert!(mode.is_none());
                    break;
                }
                Event::Locked(_) => {}
                other => panic!("expected the failure, got {other:?}"),
            }
        }
    }

    #[test]
    fn a_pad_no_description_lists_is_reported_with_its_usb_id() {
        use controller_core::device::UsbId;
        let sysfs = tempfile::tempdir().unwrap();
        let other = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let (tx, events) =
            locking(scripted(&["3-1"], vec![Ok(Unlocked::Unsupported(other))]), sysfs.path());
        tx.send(Command::Unlock("3-1".to_owned())).unwrap();
        loop {
            match next(&events) {
                Event::Unlock { result: Ok(Unlocked::Unsupported(usb)), .. } => {
                    assert_eq!(usb, other);
                    break;
                }
                Event::Locked(_) => {}
                other => panic!("expected the unsupported pad, got {other:?}"),
            }
        }
    }

    #[test]
    fn an_unsupported_pad_is_reported_gone_when_its_port_path_empties() {
        use controller_core::device::UsbId;
        let sysfs = tempfile::tempdir().unwrap();
        let other = UsbId { vendor: 0x2dc8, product: 0x1234 };
        let (tx, events) =
            locking(scripted(&["3-1"], vec![Ok(Unlocked::Unsupported(other))]), sysfs.path());
        // The pad sits on its port path with an id no description lists.
        fs::create_dir_all(sysfs.path().join("3-1")).unwrap();
        tx.send(Command::Unlock("3-1".to_owned())).unwrap();
        let first = next(&events);
        assert!(matches!(first, Event::Unlock { .. } | Event::Locked(_)), "{first:?}");
        let still_there = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(2)).ok())
            .any(|e| matches!(e, Event::Unplugged(_)));
        assert!(!still_there, "the pad is still plugged in");
        fs::remove_dir_all(sysfs.path().join("3-1")).unwrap();
        let gone = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(3)).ok())
            .find(|e| matches!(e, Event::Unplugged(_)));
        assert!(matches!(gone, Some(Event::Unplugged(port)) if port == "3-1"));
    }

    #[test]
    fn an_unlock_of_a_pad_that_left_says_so() {
        let sysfs = tempfile::tempdir().unwrap();
        let (tx, events) = locking(scripted(&[], vec![]), sysfs.path());
        tx.send(Command::Unlock("3-1".to_owned())).unwrap();
        match next(&events) {
            Event::Unlock { result: Err(f), .. } => assert!(matches!(f.error, Error::Disconnected)),
            other => panic!("expected the failure, got {other:?}"),
        }
    }

    #[test]
    fn a_relock_by_command_reports_and_is_not_a_replug() {
        let sysfs = tempfile::tempdir().unwrap();
        let locks = scripted(&[], vec![]);
        let log = Arc::clone(&locks.relocks);
        let (tx, events) = locking(locks, sysfs.path());
        tx.send(Command::Relock(Target { port: "3-1".to_owned(), model: ultimate_model() }))
            .unwrap();
        assert!(matches!(next(&events), Event::Relock { result: Ok(()), .. }));
        assert_eq!(*log.lock().unwrap(), ["3-1"]);
    }

    #[test]
    fn closing_relocks_in_order_and_goes_on_after_a_failure() {
        let locks = FakeLocks { stuck: vec!["1-3".to_owned()], ..FakeLocks::default() };
        let targets: Vec<_> = ["1-2", "1-3", "1-4"]
            .map(|p| Target { port: p.to_owned(), model: ultimate_model() })
            .into();
        relock_all(&locks, &targets);
        assert_eq!(*locks.relocks.lock().unwrap(), ["1-2", "1-3", "1-4"]);
    }

    /// The next event, leaving out the keepalive verdicts: [`a_pad_that_needs_the_fix_is_reported`]
    /// covers them.
    fn next(events: &Receiver<Event>) -> Event {
        loop {
            match events.recv_timeout(Duration::from_secs(5)).expect("an event") {
                Event::Fix { .. } => {}
                event => return event,
            }
        }
    }

    #[test]
    fn a_pad_that_needs_the_fix_is_reported_and_one_that_does_not_is_not() {
        let sysfs = tempfile::tempdir().unwrap();
        plug_at(sysfs.path(), "3-1", "2dc8", "310b");
        plug_at(sysfs.path(), "3-2", "2dc8", "6009");
        let ports = Pro3.description().unwrap().config_ports.clone();
        let (etx, events) = mpsc::channel();
        let _tx = spawn(
            given(MockDevice::new()),
            sysfs.path().to_owned(),
            ports,
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        let fixes: Vec<_> = std::iter::from_fn(|| events.recv_timeout(Duration::from_secs(3)).ok())
            .filter_map(|e| match e {
                Event::Fix { port, looping } => Some((port, looping)),
                _ => None,
            })
            .collect();
        assert_eq!(fixes, [("3-1".to_owned(), false)]);
    }

    #[test]
    fn worker_reports_presence_and_reads_on_command() {
        let sysfs = tempfile::tempdir().unwrap();
        let ports = Pro3.description().unwrap().config_ports.clone();
        let dev = MockDevice::new().with_profiles(crate::state::tests::full_read());
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            ports,
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();

        let start = Instant::now();
        plug(sysfs.path(), "2dc8", "6009");
        assert!(matches!(next(&events), Event::Presence { mode: Some(DINPUT), .. }));
        assert!(start.elapsed() >= POLL, "one sighting is not enough");

        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        match next(&events) {
            Event::Read { result: Ok((_, read)), .. } => assert_eq!(read.profiles.len(), 9),
            other => panic!("expected a read, got {other:?}"),
        }

        fs::remove_dir_all(sysfs.path().join("3-1")).unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: None, .. }));
    }

    #[test]
    fn a_mode_the_transport_reports_beats_the_usb_id() {
        let sysfs = tempfile::tempdir().unwrap();
        plug(sysfs.path(), "2dc8", "6009");
        let ports = Pro3.description().unwrap().config_ports.clone();
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::full_read())
            .with_current_mode(XINPUT);
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            ports,
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        // Before a read opens the transport, only the USB id speaks.
        assert!(matches!(next(&events), Event::Presence { mode: Some(DINPUT), .. }));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
        assert!(matches!(next(&events), Event::Presence { mode: Some(XINPUT), .. }));
    }

    #[test]
    fn a_disconnect_mid_read_reports_the_presence_again() {
        let sysfs = tempfile::tempdir().unwrap();
        plug(sysfs.path(), "2dc8", "310b");
        let ports = Pro3.description().unwrap().config_ports.clone();
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::full_read())
            .fail_next_read(Error::Disconnected);
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            ports,
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(XINPUT), .. }));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        // No failed read comes back: the controller goes, comes back, and rereads.
        assert!(matches!(next(&events), Event::Presence { mode: None, .. }));
        assert!(matches!(next(&events), Event::Presence { mode: Some(XINPUT), .. }));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
    }

    #[test]
    fn read_errors_reach_the_ui() {
        let sysfs = tempfile::tempdir().unwrap();
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(MockDevice::new()),
            sysfs.path().to_owned(),
            vec![],
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(
            next(&events),
            Event::Read { result: Err(Failure { error: Error::NoDevice, .. }), .. }
        ));
    }

    /// A worker with no controller in sysfs reading from `dev`.
    fn reader(dev: MockDevice) -> (Sender<Command>, Receiver<Event>, tempfile::TempDir) {
        let sysfs = tempfile::tempdir().unwrap();
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            vec![],
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        (tx, events, sysfs)
    }

    #[test]
    fn a_timeout_is_retried_once_after_a_pause() {
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::full_read())
            .fail_next_read(Error::Timeout);
        let (tx, events, _sysfs) = reader(dev);
        let start = Instant::now();
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
        assert!(start.elapsed() >= RETRY_AFTER);
    }

    #[test]
    fn a_failed_retry_reaches_the_ui() {
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::full_read())
            .fail_next_read(Error::Timeout)
            .fail_next_read(Error::Timeout);
        let (tx, events, _sysfs) = reader(dev);
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        // No controller in the fake sysfs, so no node to look up holders for.
        assert!(matches!(
            next(&events),
            Event::Read { result: Err(Failure { error: Error::Timeout, holders }), .. } if holders.is_empty()
        ));
        // "Try again" sends the read once more.
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
    }

    #[test]
    fn a_denied_read_is_followed_by_the_install_and_a_good_read() {
        let sysfs = tempfile::tempdir().unwrap();
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::full_read())
            .fail_next_read(Error::PermissionDenied("/dev/hidraw3".to_owned()));
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            vec![],
            hooks(),
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(
            next(&events),
            Event::Read { result: Err(Failure { error: Error::PermissionDenied(_), .. }), .. }
        ));
        tx.send(Command::InstallUdevRule).unwrap();
        assert!(matches!(next(&events), Event::Installed(Ok(()))));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
    }

    #[test]
    fn a_failed_install_reaches_the_ui() {
        let sysfs = tempfile::tempdir().unwrap();
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(MockDevice::new()),
            sysfs.path().to_owned(),
            vec![],
            Hooks { install: refused, locks: Box::new(FakeLocks::default()) },
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        tx.send(Command::InstallUdevRule).unwrap();
        assert!(matches!(next(&events), Event::Installed(Err(e)) if e.contains("closed")));
    }

    #[test]
    fn two_controllers_come_go_and_read_apart() {
        let sysfs = tempfile::tempdir().unwrap();
        plug_at(sysfs.path(), "3-1", "2dc8", "310b");
        plug_at(sysfs.path(), "3-2", "2dc8", "6009");
        let ports = Pro3.description().unwrap().config_ports.clone();
        let open: Opener = Box::new(|port| {
            let dev = MockDevice::new();
            Box::new(if port == "3-2" {
                dev.with_profiles(crate::state::tests::full_read())
            } else {
                dev
            })
        });
        let (etx, events) = mpsc::channel();
        let tx =
            spawn(open, sysfs.path().to_owned(), ports, hooks(), PathBuf::new(), false, move |e| {
                let _ = etx.send(e);
            })
            .unwrap();
        let seen: BTreeSet<_> = (0..2)
            .map(|_| match next(&events) {
                Event::Presence { port, mode: Some(mode) } => (port, mode),
                other => panic!("expected a presence, got {other:?}"),
            })
            .collect();
        assert_eq!(seen, BTreeSet::from([("3-1".to_owned(), XINPUT), ("3-2".to_owned(), DINPUT)]));
        tx.send(Command::ReadAll("3-2".to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { port, result: Ok(_) } if port == "3-2"));
        tx.send(Command::ReadAll("3-1".to_owned())).unwrap();
        assert!(matches!(
            next(&events),
            Event::Read { port, result: Err(Failure { error: Error::NoDevice, .. }) } if port == "3-1"
        ));
        fs::remove_dir_all(sysfs.path().join("3-1")).unwrap();
        assert!(matches!(next(&events), Event::Presence { port, mode: None } if port == "3-1"));
    }

    /// The `XInput` slot 1 fixture profile, as a job writing it to slot 1.
    fn upload_job() -> WriteJob {
        let text = fs::read_to_string("../../fixtures/pro3/xinput-slot1.profile.json").unwrap();
        WriteJob {
            mode: XINPUT,
            slot: Slot::new(1).unwrap(),
            op: WriteOp::Upload {
                profile: Some(serde_json::from_str(&text).unwrap()),
                drop_macros: vec![],
                macro_edits: vec![],
            },
        }
    }

    /// A worker over `dev`, with the folder a failed rollback saves into.
    fn writer(dev: MockDevice) -> (Sender<Command>, Receiver<Event>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let sysfs = tempfile::tempdir().unwrap();
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            given(dev),
            sysfs.path().to_owned(),
            vec![],
            hooks(),
            dir.path().join("backups"),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        (tx, events, dir)
    }

    #[test]
    fn a_write_reports_its_result() {
        let (tx, events, _dir) =
            writer(MockDevice::new().with_profiles(crate::state::tests::readable()));
        tx.send(Command::Write(PORT.to_owned(), vec![upload_job()])).unwrap();
        match next(&events) {
            Event::Written { port, results, holders } => {
                let result = &results[0];
                assert_eq!(port, PORT);
                assert!(result.success, "{}", result.message);
                assert_eq!(holders, Vec::<String>::new());
            }
            other => panic!("expected a write, got {other:?}"),
        }
        let clear = WriteJob { op: WriteOp::Clear, ..upload_job() };
        tx.send(Command::Write(PORT.to_owned(), vec![clear])).unwrap();
        assert!(matches!(next(&events), Event::Written { results, .. } if results[0].success));
    }

    #[test]
    fn several_slots_in_one_command_report_one_result_each() {
        let (tx, events, _dir) =
            writer(MockDevice::new().with_profiles(crate::state::tests::readable()));
        let second = WriteJob { slot: Slot::new(2).unwrap(), ..upload_job() };
        tx.send(Command::Write(PORT.to_owned(), vec![upload_job(), second])).unwrap();
        let Event::Written { results, .. } = next(&events) else { panic!("expected a write") };
        assert_eq!(
            results.iter().map(|r| (r.success, r.slot)).collect::<Vec<_>>(),
            [(true, 1), (true, 2)]
        );
    }

    #[test]
    fn a_failed_write_is_rolled_back_and_says_so() {
        let dev = MockDevice::new().with_profiles(crate::state::tests::readable()).fail_nth(
            MockOp::WriteFullProfile,
            0,
            Error::write("chunk refused"),
        );
        let (tx, events, _dir) = writer(dev);
        tx.send(Command::Write(PORT.to_owned(), vec![upload_job()])).unwrap();
        match next(&events) {
            Event::Written { results, .. } => {
                let result = &results[0];
                assert!(!result.success && result.rollback_succeeded, "{}", result.message);
                assert!(result.message.contains("Rollback succeeded"));
                assert_eq!(result.backup_file_path, None);
            }
            other => panic!("expected a write, got {other:?}"),
        }
    }

    #[test]
    fn a_rollback_that_fails_too_saves_the_old_profile_in_the_backup_folder() {
        let dev = MockDevice::new()
            .with_profiles(crate::state::tests::readable())
            .fail_nth(MockOp::WriteFullProfile, 0, Error::write("chunk refused"))
            .fail_nth(MockOp::WriteFullProfile, 1, Error::write("still refused"));
        let (tx, events, dir) = writer(dev);
        tx.send(Command::Write(PORT.to_owned(), vec![upload_job()])).unwrap();
        let Event::Written { results, .. } = next(&events) else { panic!("expected a write") };
        let result = &results[0];
        let saved = result.backup_file_path.clone().expect("a backup file");
        assert!(Path::new(&saved).starts_with(dir.path().join("backups")), "{saved}");
        assert_eq!(fs::read(&saved).unwrap().len(), 2348);
    }

    #[test]
    fn an_unplug_mid_write_fails_the_write_and_the_worker_goes_on() {
        let dev = MockDevice::new().with_profiles(crate::state::tests::readable()).fail_nth(
            MockOp::WriteFullProfile,
            0,
            Error::Disconnected,
        );
        let (tx, events, _dir) = writer(dev);
        tx.send(Command::Write(PORT.to_owned(), vec![upload_job()])).unwrap();
        assert!(matches!(next(&events), Event::Written { results, .. } if !results[0].success));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
    }

    /// The fixture macro of slot 1 of `XInput`, as a read's summary gives it.
    fn fixture_macro() -> MacroAt {
        let blob = fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
        let slot = Slot::new(1).unwrap();
        let def = Pro3.decode_macro_metadata(&blob, slot).unwrap().remove(0);
        ((XINPUT, 1), def)
    }

    #[test]
    fn macro_steps_are_read_and_decoded_by_their_descriptor() {
        let stream = fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();
        let (at, summary) = fixture_macro();
        let slot = MacroSlot::new(summary.macro_slot.unwrap()).unwrap();
        let dev = MockDevice::new().with_macro_stream(
            XINPUT,
            Slot::new(1).unwrap(),
            slot,
            stream.clone(),
        );
        let (tx, events, _dir) = writer(dev);
        tx.send(Command::ReadMacroSteps(PORT.to_owned(), vec![(at, summary.clone())])).unwrap();
        let Event::MacroSteps { result: Ok(read), .. } = next(&events) else {
            panic!("expected the steps")
        };
        let steps = Pro3.decode_macro_steps(&stream, summary.steps.len(), XINPUT).unwrap();
        assert_eq!(read, [(at, MacroDefinition { steps, ..summary })]);
    }

    #[test]
    fn a_failed_step_read_says_why() {
        let (tx, events, _dir) = writer(MockDevice::new());
        tx.send(Command::ReadMacroSteps(PORT.to_owned(), vec![fixture_macro()])).unwrap();
        assert!(matches!(
            next(&events),
            Event::MacroSteps { result: Err(Failure { error: Error::NoDevice, .. }), .. }
        ));
    }

    /// Needs a locked Ultimate attached. Unlocks it through the worker, then relocks it.
    #[test]
    #[ignore = "needs a locked Ultimate"]
    fn hardware_unlock_then_relock_through_the_worker() {
        let (etx, events) = mpsc::channel();
        let tx = spawn(
            Box::new(|port: &str| controller_core::devices::open(Some(port))),
            PathBuf::from(SYSFS_USB),
            controller_core::devices::config_ports(),
            Hooks { install: installed, locks: Box::new(SystemLocks) },
            PathBuf::new(),
            false,
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        let Event::Locked(pads) = next(&events) else { panic!("no locked pad") };
        let pad = pads.first().expect("a locked pad").clone();
        tx.send(Command::Unlock(pad.port.clone())).unwrap();
        loop {
            match next(&events) {
                Event::Unlock { result, mode, .. } => {
                    assert!(matches!(result, Ok(Unlocked::Port(_))), "{result:?}");
                    assert!(mode.is_some());
                    break;
                }
                Event::Locked(_) => {}
                other => panic!("{other:?}"),
            }
        }
        tx.send(Command::Relock(pad)).unwrap();
        loop {
            match next(&events) {
                Event::Relock { result, .. } => {
                    assert!(result.is_ok(), "{result:?}");
                    break;
                }
                Event::Presence { .. } => panic!("a relock is not a replug"),
                _ => {}
            }
        }
    }
}
