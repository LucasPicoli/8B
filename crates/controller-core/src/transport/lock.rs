//! Unlocking and relocking a controller whose description declares a lock.
//!
//! [`unlock`] sends the declared unlock sequence to the locked controller over usbfs
//! and waits for its config port on the same USB port path. [`relock`] sends the
//! declared relock sequence to the config port and waits for the locked controller
//! to return.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use log::info;

use super::session::{io_error, open_error};
use super::usbfs::{UsbIo, UsbNode};
use crate::description::{Lock, TimedPacket, Unlock, MAX_PACKET_LEN};
use crate::detect::{config_hidraw, usb_id};
use crate::device::{Model, UsbId};
use crate::devices::{config_ports, LockedController, SYSFS_USB};
use crate::error::{Error, Result};

/// How often the port path is checked while the controller re-enumerates.
const POLL: Duration = Duration::from_millis(50);
/// The pause between claiming the interface and the first unlock packet. The driver
/// that let the interface go may have just sent its own packet. With no pause the
/// Ultimate stayed locked in 1 unlock of 13; with it, it unlocked 30 times of 30.
const SETTLE_AFTER_CLAIM: Duration = Duration::from_millis(500);

/// What a sent unlock brought up on the controller's USB port path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unlocked {
    /// A config port: its hidraw node, which this user may read and write.
    Port(PathBuf),
    /// A USB id no description lists as a config port. Nothing more is sent to it;
    /// a replug brings the controller back locked.
    Unsupported(UsbId),
}

/// The lock declaration of `model`.
///
/// # Errors
/// Returns [`Error::Validation`] for a model that declares none.
fn lock_of(model: &dyn Model) -> Result<&Lock> {
    let description = model.description()?;
    description
        .lock
        .as_ref()
        .ok_or_else(|| Error::Validation(format!("the {} has no lock", description.short_name)))
}

/// A wait that sleeps until `at_ms` after the clock was made.
fn clock() -> impl FnMut(u32) {
    let start = Instant::now();
    move |at_ms| {
        let at = start + Duration::from_millis(u64::from(at_ms));
        std::thread::sleep(at.saturating_duration_since(Instant::now()));
    }
}

/// Sends `unlock` through `io`, and runs `then` before giving the interface back.
///
/// Takes the interface from the kernel driver, claims it, writes each packet once
/// `wait` returns for its time, runs `then`, then releases the interface and gives it
/// back. The claim holds while `then` waits for the controller to leave. A driver given the
/// interface back at once sends its own start-up packets in the second before the pad
/// acts on the unlock, and the pad then often stays locked. The release and the
/// reattach run on every path, errors included. A device that goes away after the
/// first packet has re-enumerated, which is the point, so that is no error, and
/// nothing more is sent.
///
/// # Errors
/// The first error of a step, from [`UsbIo`], or of `then`.
pub fn send_unlock<T>(
    io: &mut dyn UsbIo,
    unlock: &Unlock,
    wait: &mut dyn FnMut(u32),
    then: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let interface = unlock.interface;
    let gone_is_fine = |r: Result<()>| match r {
        Err(Error::Disconnected) => Ok(()),
        other => other,
    };
    let sent = io.disconnect(interface).and_then(|()| io.claim(interface)).and_then(|()| {
        for (i, p) in unlock.packets.iter().enumerate() {
            wait(p.at_ms);
            match io.interrupt_out(unlock.endpoint, &p.data) {
                Ok(()) => {}
                Err(Error::Disconnected) if i > 0 => return Ok(()),
                Err(e) => return Err(e),
            }
        }
        Ok(())
    });
    let done = sent.and_then(|()| then());
    let released = gone_is_fine(io.release(interface));
    let connected = gone_is_fine(io.connect(interface));
    let done = done?;
    released.and(connected).map(|()| done)
}

/// Unlocks `locked` and returns what came up on its USB port path.
///
/// Sends its model's unlock sequence, then waits up to the declared `timeout_ms` for a
/// config port on the same port path, and until its hidraw node opens to read and
/// write. udev grants the node a moment after it appears.
///
/// # Errors
/// Returns [`Error::PermissionDenied`] when the usbfs node, or the config port's node
/// until the time runs out, may not be opened; [`Error::Timeout`] when no config port
/// shows in time; and the errors of [`send_unlock`].
pub fn unlock(locked: &LockedController) -> Result<Unlocked> {
    let mut node = UsbNode::open(&locked.usb_node)?;
    // The first wait comes after the claim: it starts the clock once the settle is over.
    let mut times = None;
    let mut wait = |at_ms| {
        let times = times.get_or_insert_with(|| {
            std::thread::sleep(SETTLE_AFTER_CLAIM);
            clock()
        });
        times(at_ms);
    };
    unlock_in(Path::new(SYSFS_USB), locked, &mut node, &mut wait)
}

/// [`unlock`] in sysfs at `root`, through `io`, with `wait` keeping the packet times.
fn unlock_in(
    root: &Path,
    locked: &LockedController,
    io: &mut dyn UsbIo,
    wait: &mut dyn FnMut(u32),
) -> Result<Unlocked> {
    let lock = lock_of(locked.model)?;
    send_unlock(io, &lock.unlock, wait, || wait_for_port(root, locked, lock))
}

/// Waits up to the unlock's `timeout_ms` for what comes up on the port path of `locked`.
///
/// # Errors
/// As [`unlock`].
fn wait_for_port(root: &Path, locked: &LockedController, lock: &Lock) -> Result<Unlocked> {
    let deadline = Instant::now() + Duration::from_millis(u64::from(lock.unlock.timeout_ms));
    let dir = root.join(&locked.port_path);
    let mut last = Error::Timeout;
    while Instant::now() < deadline {
        match usb_id(&dir) {
            None => {}
            Some(usb) if usb == lock.locked.usb => {}
            Some(usb) => {
                let Some(port) = config_ports().into_iter().find(|p| p.usb == usb) else {
                    info!(
                        "unlock on {} brought up {:04x}:{:04x}, which no description lists",
                        locked.port_path, usb.vendor, usb.product
                    );
                    return Ok(Unlocked::Unsupported(usb));
                };
                if let Some(node) = config_hidraw(&dir, port.interface) {
                    match OpenOptions::new().read(true).write(true).open(&node) {
                        Ok(_) => {
                            info!("unlocked {} at {}", locked.port_path, node.display());
                            return Ok(Unlocked::Port(node));
                        }
                        Err(e) => last = open_error(&node, &e),
                    }
                }
            }
        }
        std::thread::sleep(POLL);
    }
    Err(match last {
        e @ Error::PermissionDenied(_) => e,
        _ => Error::Timeout,
    })
}

/// Writes `packets` to `out` as output reports, each zero padded to
/// [`MAX_PACKET_LEN`], once `wait` returns for its time. A report that finds the
/// controller gone after the first one ends the sequence: it has re-enumerated.
///
/// # Errors
/// Returns the error of the first report, or [`Error::Usb`] for a later one that fails
/// otherwise.
fn send_reports(
    out: &mut dyn std::io::Write,
    packets: &[TimedPacket],
    wait: &mut dyn FnMut(u32),
) -> Result<()> {
    for (i, packet) in packets.iter().enumerate() {
        wait(packet.at_ms);
        let mut report = [0u8; MAX_PACKET_LEN];
        for (dst, b) in report.iter_mut().zip(&packet.data) {
            *dst = *b;
        }
        match out.write_all(&report).map_err(|e| io_error(&e)) {
            Ok(()) => {}
            Err(Error::Disconnected) if i > 0 => return Ok(()),
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Relocks the controller of `model` on USB port path `port_path`, such as `8-5`.
///
/// Sends the declared relock sequence to its config port, then waits up to the
/// declared unlock `timeout_ms` for the locked USB id on the same port path. Call it
/// with no session open on the config port.
///
/// # Errors
/// Returns [`Error::Validation`] for a model without a lock, [`Error::NoDevice`] when
/// no config port of `model` is on `port_path`, [`Error::PermissionDenied`] when its
/// node may not be opened, [`Error::Timeout`] when the locked controller does not
/// return in time, and [`Error::Usb`] when a report fails.
pub fn relock(model: &dyn Model, port_path: &str) -> Result<()> {
    relock_in(Path::new(SYSFS_USB), model, port_path, &mut clock())
}

/// [`relock`] in sysfs at `root`, with `wait` keeping the packet times.
fn relock_in(
    root: &Path,
    model: &dyn Model,
    port_path: &str,
    wait: &mut dyn FnMut(u32),
) -> Result<()> {
    let lock = lock_of(model)?;
    let dir = root.join(port_path);
    let usb = usb_id(&dir).ok_or(Error::NoDevice)?;
    let port = model.description()?.config_ports.iter().find(|p| p.usb == usb);
    let node = port.and_then(|p| config_hidraw(&dir, p.interface)).ok_or(Error::NoDevice)?;
    let mut file = OpenOptions::new().write(true).open(&node).map_err(|e| open_error(&node, &e))?;
    send_reports(&mut file, &lock.relock.packets, wait)?;
    let _ = file.flush();
    drop(file);
    wait_for_usb(&dir, lock.locked.usb, Duration::from_millis(u64::from(lock.unlock.timeout_ms)))
}

/// Waits up to `budget` for the device at sysfs directory `dir` to show USB id `usb`.
///
/// # Errors
/// Returns [`Error::Timeout`] when it does not in time.
fn wait_for_usb(dir: &Path, usb: UsbId, budget: Duration) -> Result<()> {
    let deadline = Instant::now() + budget;
    loop {
        if usb_id(dir) == Some(usb) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Timeout);
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::cell::Cell;

    use super::*;
    use crate::devices::ultimate::Ultimate;

    /// One step a fake usbfs device saw.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Step {
        Disconnect(u8),
        Claim(u8),
        /// The time the wait last returned for, the endpoint, the bytes.
        Out(u32, u8, Vec<u8>),
        Release(u8),
        Connect(u8),
    }

    /// Records each step, and fails the `fail_at`th (0-based) with `error`.
    struct Fake<'a> {
        now: &'a Cell<u32>,
        steps: Vec<Step>,
        fail_at: Option<(usize, Error)>,
    }

    impl Fake<'_> {
        fn step(&mut self, step: Step) -> Result<()> {
            let n = self.steps.len();
            self.steps.push(step);
            match &self.fail_at {
                Some((at, e)) if *at == n => Err(e.clone()),
                _ => Ok(()),
            }
        }
    }

    impl UsbIo for Fake<'_> {
        fn disconnect(&mut self, interface: u8) -> Result<()> {
            self.step(Step::Disconnect(interface))
        }
        fn claim(&mut self, interface: u8) -> Result<()> {
            self.step(Step::Claim(interface))
        }
        fn interrupt_out(&mut self, endpoint: u8, data: &[u8]) -> Result<()> {
            self.step(Step::Out(self.now.get(), endpoint, data.to_vec()))
        }
        fn release(&mut self, interface: u8) -> Result<()> {
            self.step(Step::Release(interface))
        }
        fn connect(&mut self, interface: u8) -> Result<()> {
            self.step(Step::Connect(interface))
        }
    }

    fn ultimate_lock() -> &'static Lock {
        lock_of(&Ultimate).unwrap()
    }

    /// Runs [`send_unlock`] of the Ultimate on a fake that fails step `fail_at`.
    fn run(fail_at: Option<(usize, Error)>) -> (Result<()>, Vec<Step>) {
        let now = Cell::new(0);
        let mut fake = Fake { now: &now, steps: Vec::new(), fail_at };
        let result =
            send_unlock(&mut fake, &ultimate_lock().unlock, &mut |at| now.set(at), || Ok(()));
        (result, fake.steps)
    }

    /// One line of the captured sequences: its name, its time in ms and its bytes.
    fn captured() -> Vec<(String, f64, Vec<u8>)> {
        include_str!("../../../../fixtures/ultimate/unlock-relock.txt")
            .lines()
            .filter(|l| !l.starts_with('#'))
            .map(|l| {
                let fields: Vec<&str> = l.split_whitespace().collect();
                let bytes = (0..fields[2].len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&fields[2][i..i + 2], 16).unwrap())
                    .collect();
                (fields[0].to_owned(), fields[1].parse().unwrap(), bytes)
            })
            .collect()
    }

    #[test]
    fn the_declared_sequences_are_the_captured_ones() {
        let lock = ultimate_lock();
        let lines = captured();
        for (name, packets) in [("unlock", &lock.unlock.packets), ("relock", &lock.relock.packets)]
        {
            let want: Vec<_> = lines.iter().filter(|l| l.0 == name).collect();
            assert_eq!(want.len(), packets.len(), "{name}");
            for (p, (_, at, bytes)) in packets.iter().zip(want) {
                assert_eq!(&p.data, bytes, "{name} at {at}");
                assert!((f64::from(p.at_ms) - at).abs() < 1.0, "{name}: {} against {at}", p.at_ms);
            }
        }
        assert_eq!((lock.unlock.interface, lock.unlock.endpoint), (0, 0x02));
    }

    #[test]
    fn the_unlock_detaches_claims_sends_on_time_and_gives_the_interface_back() {
        let (result, steps) = run(None);
        result.unwrap();
        let unlock = &ultimate_lock().unlock;
        let outs: Vec<Step> =
            unlock.packets.iter().map(|p| Step::Out(p.at_ms, 0x02, p.data.clone())).collect();
        let want = [
            vec![Step::Disconnect(0), Step::Claim(0)],
            outs,
            vec![Step::Release(0), Step::Connect(0)],
        ]
        .concat();
        assert_eq!(steps, want);
    }

    #[test]
    fn the_interface_goes_back_on_an_error_too() {
        // The claim fails: no packet goes out, the driver is still attached again.
        let (result, steps) = run(Some((1, Error::Usb("busy".into()))));
        assert!(matches!(result, Err(Error::Usb(m)) if m == "busy"));
        assert_eq!(
            steps,
            [Step::Disconnect(0), Step::Claim(0), Step::Release(0), Step::Connect(0)]
        );
        // The third packet fails.
        let (result, steps) = run(Some((4, Error::Timeout)));
        assert!(matches!(result, Err(Error::Timeout)));
        assert_eq!(steps.len(), 7);
        assert_eq!(steps[5..], [Step::Release(0), Step::Connect(0)]);
    }

    #[test]
    fn a_device_gone_after_the_first_packet_has_unlocked() {
        // 2 steps, 15 packets, then the release finds the device gone.
        let (result, _) = run(Some((17, Error::Disconnected)));
        result.unwrap();
        // The last packet finds it gone: nothing more goes out, the interface goes back.
        let (result, steps) = run(Some((16, Error::Disconnected)));
        result.unwrap();
        assert_eq!(steps.len(), 19);
        // Gone at the first packet: the pad was unplugged, not unlocked.
        let (result, steps) = run(Some((2, Error::Disconnected)));
        assert!(matches!(result, Err(Error::Disconnected)));
        assert_eq!(steps[3..], [Step::Release(0), Step::Connect(0)]);
    }

    /// Takes one report, then reports the device gone.
    struct GoneAfterOne(usize);
    impl std::io::Write for GoneAfterOne {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0 += 1;
            if self.0 > 1 {
                return Err(std::io::Error::from_raw_os_error(19)); // ENODEV
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_relock_sends_padded_reports_and_stops_once_the_pad_is_gone() {
        let packets = &ultimate_lock().relock.packets;
        let mut out = Vec::new();
        let mut times = Vec::new();
        send_reports(&mut out, packets, &mut |at| times.push(at)).unwrap();
        assert_eq!(times, [0, 8, 16, 24]);
        assert_eq!(out.len(), 4 * MAX_PACKET_LEN);
        assert_eq!(out[..7], [0x81, 0x11, 0x04, 0x07, 0x00, 0x01, 0x00]);
        assert_eq!(out[MAX_PACKET_LEN..MAX_PACKET_LEN + 6], [0x81, 0x05, 0x00, 0x51, 0x02, 0]);

        send_reports(&mut GoneAfterOne(0), packets, &mut |_| {}).unwrap();
        let mut first_fails = GoneAfterOne(1);
        assert!(send_reports(&mut first_fails, packets, &mut |_| {}).is_err());
    }

    /// A sysfs tree with the device on `8-5` showing `product`.
    fn sysfs(product: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let at = dir.path().join("8-5");
        std::fs::create_dir_all(&at).unwrap();
        std::fs::write(at.join("idVendor"), "2dc8\n").unwrap();
        std::fs::write(at.join("idProduct"), format!("{product}\n")).unwrap();
        dir
    }

    #[test]
    fn an_unlock_that_brings_up_an_unlisted_id_sends_nothing_more() {
        let dir = sysfs("6008");
        let locked = LockedController {
            model: &Ultimate,
            port_path: "8-5".to_owned(),
            usb_node: PathBuf::from("/dev/null"),
        };
        let now = Cell::new(0);
        let mut fake = Fake { now: &now, steps: Vec::new(), fail_at: None };
        let got = unlock_in(dir.path(), &locked, &mut fake, &mut |_| {}).unwrap();
        assert_eq!(got, Unlocked::Unsupported(UsbId { vendor: 0x2DC8, product: 0x6008 }));
        assert_eq!(fake.steps.len(), 19, "the unlock only");
    }

    #[test]
    fn the_wait_for_the_locked_id_ends_when_it_shows() {
        let dir = sysfs("3106");
        let locked = ultimate_lock().locked.usb;
        wait_for_usb(&dir.path().join("8-5"), locked, Duration::ZERO).unwrap();
        let other = UsbId { vendor: 0x2DC8, product: 0x6007 };
        let missing = wait_for_usb(&dir.path().join("8-5"), other, Duration::from_millis(60));
        assert!(matches!(missing, Err(Error::Timeout)));
    }
}
