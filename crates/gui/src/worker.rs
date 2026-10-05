//! The worker thread: the one owner of the controllers. It runs commands one at a
//! time, and between them it watches sysfs for controllers coming and going, each
//! keyed by its USB port path.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use controller_core::detect::{config_hidraw, scan_sysfs_all};
use controller_core::device::ConfigPort;
use controller_core::model::{Mode, ProfileReadResult, WriteResult};
use controller_core::transport::DeviceIo;
use controller_core::Error;

use crate::holders::{holders, PROC};
use crate::writes::{self, WriteJob};

/// Where the kernel lists USB devices.
pub const SYSFS_USB: &str = "/sys/bus/usb/devices";

/// How often the worker looks for controllers while idle.
const POLL: Duration = Duration::from_millis(500);

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
    /// Write one slot of the controller on this USB port path.
    Write(String, WriteJob),
    /// Install the udev rule that grants access to the controller.
    InstallUdevRule,
}

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
        /// The read, or why it failed.
        result: Result<ProfileReadResult, Failure>,
    },
    /// The result of [`Command::Write`] for `port`.
    Written {
        /// The USB port path the write went to.
        port: String,
        /// How the write went, rollback included.
        result: WriteResult,
        /// When it failed: the other programs that have the controller's config node
        /// open.
        holders: Vec<String>,
    },
    /// The result of [`Command::InstallUdevRule`]: why it failed, as a sentence.
    Installed(Result<(), String>),
}

/// Installs the udev rule; [`crate::udev::install_rule`] outside tests.
pub type Installer = fn() -> Result<(), String>;

/// The device that talks to the controller on a USB port path.
pub type Opener = Box<dyn Fn(&str) -> Box<dyn DeviceIo + Send> + Send>;

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
    install: Installer,
    backup_dir: PathBuf,
    emit: impl Fn(Event) + Send + 'static,
) -> io::Result<Sender<Command>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new().name("8b-worker".to_owned()).spawn(move || {
        run(
            &open,
            &rx,
            &Site { sysfs: &sysfs, ports: &ports, backup_dir: &backup_dir },
            install,
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
}

/// The other programs that have the config node of the controller on `port` open.
fn holders_of(site: &Site<'_>, port: &str) -> Vec<String> {
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
    install: Installer,
    emit: &dyn Fn(Event),
) {
    let (sysfs, ports) = (site.sysfs, site.ports);
    let mut devices: BTreeMap<String, Box<dyn DeviceIo + Send>> = BTreeMap::new();
    let mut presence: BTreeMap<String, Debounce> = BTreeMap::new();
    loop {
        match rx.recv_timeout(POLL) {
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
                    Ok(read) => emit(Event::Read { port, result: Ok(read) }),
                    Err(error) => {
                        let holders = holders_of(site, &port);
                        emit(Event::Read { port, result: Err(Failure { error, holders }) });
                    }
                }
            }
            Ok(Command::Write(port, job)) => {
                let dev = devices.entry(port.clone()).or_insert_with(|| open(&port));
                let result = writes::run(dev.as_ref(), &job, site.backup_dir);
                // The write may have flipped the controller and back, which gave it a
                // new device number. This job caused that: do not report it as a
                // replug, or the window reads the controller a second time.
                let debounce = presence.entry(port.clone()).or_default();
                debounce.reported = scan_sysfs_all(sysfs, ports)
                    .iter()
                    .find(|usb| usb.port_path() == port)
                    .map(|usb| (usb.port.mode, devnum(&usb.sysfs_path)));
                let holders = if result.success { Vec::new() } else { holders_of(site, &port) };
                emit(Event::Written { port, result, holders });
            }
            Ok(Command::InstallUdevRule) => emit(Event::Installed(install())),
            Err(RecvTimeoutError::Timeout) => {
                let seen: BTreeMap<String, Sighting> = scan_sysfs_all(sysfs, ports)
                    .iter()
                    .map(|usb| {
                        (usb.port_path().to_owned(), (usb.port.mode, devnum(&usb.sysfs_path)))
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

/// Reads every bank. A failure that may pass, such as a timeout, is tried once more
/// after [`RETRY_AFTER`].
fn read_with_retry(dev: &dyn DeviceIo) -> Result<ProfileReadResult, Error> {
    match dev.read_all_profiles() {
        Err(e @ (Error::Timeout | Error::Usb(_) | Error::Io(_) | Error::Decode(_))) => {
            eprintln!("8b: read failed: {e}; trying again");
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
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::sync::Mutex;
    use std::time::Instant;

    use controller_core::device::ControllerSpec as _;
    use controller_core::devices::pro3::Pro3;
    use controller_core::model::Slot;
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
        let x = Some((Mode::XInput, Some(5)));
        for _ in 1..SETTLE_POLLS {
            assert!(!d.observe(x));
        }
        assert!(!d.observe(None), "a gap restarts the count");
        assert_eq!(settle(&mut d, x), SETTLE_POLLS);
        assert!(!d.observe(x));
        // A slide-switch move: a gap, then the new mode.
        assert!(!d.observe(None));
        assert_eq!(settle(&mut d, Some((Mode::DInput, Some(6)))), SETTLE_POLLS);
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
            installed,
            PathBuf::new(),
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(Mode::XInput), .. }));
        // Unplugged and back between two polls: the kernel gave it a new number.
        fs::write(sysfs.path().join("3-1/devnum"), "45\n").unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(Mode::XInput), .. }));
    }

    /// An installer that succeeds without touching the system.
    #[allow(clippy::unnecessary_wraps)]
    const fn installed() -> Result<(), String> {
        Ok(())
    }

    fn next(events: &Receiver<Event>) -> Event {
        events.recv_timeout(Duration::from_secs(5)).expect("an event")
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
            installed,
            PathBuf::new(),
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();

        let start = Instant::now();
        plug(sysfs.path(), "2dc8", "6009");
        assert!(matches!(next(&events), Event::Presence { mode: Some(Mode::DInput), .. }));
        assert!(start.elapsed() >= POLL, "one sighting is not enough");

        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        match next(&events) {
            Event::Read { result: Ok(read), .. } => assert_eq!(read.profiles.len(), 9),
            other => panic!("expected a read, got {other:?}"),
        }

        fs::remove_dir_all(sysfs.path().join("3-1")).unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: None, .. }));
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
            installed,
            PathBuf::new(),
            move |e| {
                let _ = etx.send(e);
            },
        )
        .unwrap();
        assert!(matches!(next(&events), Event::Presence { mode: Some(Mode::XInput), .. }));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        // No failed read comes back: the controller goes, comes back, and rereads.
        assert!(matches!(next(&events), Event::Presence { mode: None, .. }));
        assert!(matches!(next(&events), Event::Presence { mode: Some(Mode::XInput), .. }));
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
            installed,
            PathBuf::new(),
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
            installed,
            PathBuf::new(),
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
            installed,
            PathBuf::new(),
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
        let refused = || Err("The password prompt was closed.".to_owned());
        let tx = spawn(
            given(MockDevice::new()),
            sysfs.path().to_owned(),
            vec![],
            refused,
            PathBuf::new(),
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
        let tx = spawn(open, sysfs.path().to_owned(), ports, installed, PathBuf::new(), move |e| {
            let _ = etx.send(e);
        })
        .unwrap();
        let seen: BTreeSet<_> = (0..2)
            .map(|_| match next(&events) {
                Event::Presence { port, mode: Some(mode) } => (port, mode),
                other => panic!("expected a presence, got {other:?}"),
            })
            .collect();
        assert_eq!(
            seen,
            BTreeSet::from([("3-1".to_owned(), Mode::XInput), ("3-2".to_owned(), Mode::DInput)])
        );
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
            mode: Mode::XInput,
            slot: Slot::new(1).unwrap(),
            op: WriteOp::Upload {
                profile: serde_json::from_str(&text).unwrap(),
                drop_macros: vec![],
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
            installed,
            dir.path().join("backups"),
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
        tx.send(Command::Write(PORT.to_owned(), upload_job())).unwrap();
        match next(&events) {
            Event::Written { port, result, holders } => {
                assert_eq!(port, PORT);
                assert!(result.success, "{}", result.message);
                assert!(holders.is_empty());
            }
            other => panic!("expected a write, got {other:?}"),
        }
        let clear = WriteJob { op: WriteOp::Clear, ..upload_job() };
        tx.send(Command::Write(PORT.to_owned(), clear)).unwrap();
        assert!(matches!(next(&events), Event::Written { result, .. } if result.success));
    }

    #[test]
    fn a_failed_write_is_rolled_back_and_says_so() {
        let dev = MockDevice::new().with_profiles(crate::state::tests::readable()).fail_nth(
            MockOp::WriteFullProfile,
            0,
            Error::write("chunk refused"),
        );
        let (tx, events, _dir) = writer(dev);
        tx.send(Command::Write(PORT.to_owned(), upload_job())).unwrap();
        match next(&events) {
            Event::Written { result, .. } => {
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
        tx.send(Command::Write(PORT.to_owned(), upload_job())).unwrap();
        let Event::Written { result, .. } = next(&events) else { panic!("expected a write") };
        let saved = result.backup_file_path.expect("a backup file");
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
        tx.send(Command::Write(PORT.to_owned(), upload_job())).unwrap();
        assert!(matches!(next(&events), Event::Written { result, .. } if !result.success));
        tx.send(Command::ReadAll(PORT.to_owned())).unwrap();
        assert!(matches!(next(&events), Event::Read { result: Ok(_), .. }));
    }
}
