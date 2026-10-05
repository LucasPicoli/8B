//! The worker thread: the one owner of the controller. It runs commands one at a
//! time, and between them it watches sysfs for the controller coming and going.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use controller_core::detect::scan_sysfs;
use controller_core::device::ConfigPort;
use controller_core::model::{Mode, ProfileReadResult};
use controller_core::transport::DeviceIo;
use controller_core::Error;

/// Where the kernel lists USB devices.
pub const SYSFS_USB: &str = "/sys/bus/usb/devices";

/// How often the worker looks for the controller while idle.
const POLL: Duration = Duration::from_millis(500);

/// Polls in a row that must agree before a presence change is reported, about 2 s.
/// A slide-switch move re-enumerates the controller several times in about a
/// second, and a kernel driver may probe the new USB id first: a read sent 1 s after
/// the Switch id appeared, during the hid-nintendo probe, got no reply.
const SETTLE_POLLS: u8 = 4;

/// Work for the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Read every bank.
    ReadAll,
}

/// What the worker reports back.
#[derive(Debug)]
pub enum Event {
    /// The controller appeared in a current mode, or went away (`None`).
    Presence(Option<Mode>),
    /// The result of [`Command::ReadAll`].
    Read(Result<ProfileReadResult, Error>),
}

/// Starts the worker on its own thread. `emit` runs on that thread for each event.
/// The worker stops when the returned sender is dropped.
///
/// # Errors
/// Returns the OS error if the thread cannot be started.
pub fn spawn(
    dev: Box<dyn DeviceIo + Send>,
    sysfs: PathBuf,
    ports: Vec<ConfigPort>,
    emit: impl Fn(Event) + Send + 'static,
) -> io::Result<Sender<Command>> {
    let (tx, rx) = mpsc::channel();
    thread::Builder::new()
        .name("8b-worker".to_owned())
        .spawn(move || run(dev.as_ref(), &rx, &sysfs, &ports, &emit))?;
    Ok(tx)
}

fn run(
    dev: &dyn DeviceIo,
    rx: &Receiver<Command>,
    sysfs: &Path,
    ports: &[ConfigPort],
    emit: &dyn Fn(Event),
) {
    let mut presence = Debounce::default();
    loop {
        match rx.recv_timeout(POLL) {
            Ok(Command::ReadAll) => match dev.read_all_profiles() {
                // The controller went away mid-read, as a slide-switch move does while
                // it settles. Not an error: report it gone, and the next settled
                // presence starts a new read.
                Err(Error::Disconnected) => {
                    presence.forget();
                    emit(Event::Presence(None));
                }
                result => emit(Event::Read(result)),
            },
            Err(RecvTimeoutError::Timeout) => {
                let seen = scan_sysfs(sysfs, ports).map(|usb| usb.port.mode);
                if presence.observe(seen) {
                    emit(Event::Presence(seen));
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// Turns raw polls into presence changes, reported once [`SETTLE_POLLS`] polls in a
/// row agree. Starts as "absent", so a missing controller reports nothing.
#[derive(Debug, Default)]
struct Debounce {
    reported: Option<Mode>,
    candidate: Option<Mode>,
    agreeing: u8,
}

impl Debounce {
    /// Treats the controller as absent, so its next settled sighting reports again.
    const fn forget(&mut self) {
        self.reported = None;
        self.agreeing = 0;
    }

    /// Feeds one poll; returns whether `seen` is now the reported presence.
    fn observe(&mut self, seen: Option<Mode>) -> bool {
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
    use std::fs;
    use std::time::Instant;

    use controller_core::device::ControllerSpec as _;
    use controller_core::devices::pro3::Pro3;
    use controller_core::transport::MockDevice;

    use super::*;

    /// Feeds `seen` until the debounce reports it; returns the polls it took.
    fn settle(d: &mut Debounce, seen: Option<Mode>) -> u8 {
        (1..=u8::MAX).find(|_| d.observe(seen)).unwrap()
    }

    #[test]
    fn debounce_needs_settle_polls_in_a_row() {
        let mut d = Debounce::default();
        assert!(!d.observe(None));
        for _ in 1..SETTLE_POLLS {
            assert!(!d.observe(Some(Mode::XInput)));
        }
        assert!(!d.observe(None), "a gap restarts the count");
        assert_eq!(settle(&mut d, Some(Mode::XInput)), SETTLE_POLLS);
        assert!(!d.observe(Some(Mode::XInput)));
        // A slide-switch move: a gap, then the new mode.
        assert!(!d.observe(None));
        assert_eq!(settle(&mut d, Some(Mode::DInput)), SETTLE_POLLS);
        assert_eq!(settle(&mut d, None), SETTLE_POLLS);
    }

    /// A fake `/sys/bus/usb/devices` holding one USB device.
    fn plug(root: &Path, vendor: &str, product: &str) {
        let dev = root.join("3-1");
        fs::create_dir_all(&dev).unwrap();
        fs::write(dev.join("idVendor"), format!("{vendor}\n")).unwrap();
        fs::write(dev.join("idProduct"), format!("{product}\n")).unwrap();
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
        let tx = spawn(Box::new(dev), sysfs.path().to_owned(), ports, move |e| {
            let _ = etx.send(e);
        })
        .unwrap();

        let start = Instant::now();
        plug(sysfs.path(), "2dc8", "6009");
        assert!(matches!(next(&events), Event::Presence(Some(Mode::DInput))));
        assert!(start.elapsed() >= POLL, "one sighting is not enough");

        tx.send(Command::ReadAll).unwrap();
        match next(&events) {
            Event::Read(Ok(read)) => assert_eq!(read.profiles.len(), 9),
            other => panic!("expected a read, got {other:?}"),
        }

        fs::remove_dir_all(sysfs.path().join("3-1")).unwrap();
        assert!(matches!(next(&events), Event::Presence(None)));
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
        let tx = spawn(Box::new(dev), sysfs.path().to_owned(), ports, move |e| {
            let _ = etx.send(e);
        })
        .unwrap();
        assert!(matches!(next(&events), Event::Presence(Some(Mode::XInput))));
        tx.send(Command::ReadAll).unwrap();
        // No failed read comes back: the controller goes, comes back, and rereads.
        assert!(matches!(next(&events), Event::Presence(None)));
        assert!(matches!(next(&events), Event::Presence(Some(Mode::XInput))));
        tx.send(Command::ReadAll).unwrap();
        assert!(matches!(next(&events), Event::Read(Ok(_))));
    }

    #[test]
    fn read_errors_reach_the_ui() {
        let sysfs = tempfile::tempdir().unwrap();
        let (etx, events) = mpsc::channel();
        let tx = spawn(Box::new(MockDevice::new()), sysfs.path().to_owned(), vec![], move |e| {
            let _ = etx.send(e);
        })
        .unwrap();
        tx.send(Command::ReadAll).unwrap();
        assert!(matches!(next(&events), Event::Read(Err(Error::NoDevice))));
    }
}
