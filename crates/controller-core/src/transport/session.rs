//! One open hidraw node of the config interface, with a blocking send/receive primitive.
//!
//! A session pauses the gamepad input stream when it opens and resumes it when it is
//! dropped, error paths included. While paused, config replies do not race the input
//! reports on the shared IN endpoint (`DInput` and Switch).

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use log::{debug, info, trace, warn};
use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

use crate::detect::{config_hidraw, scan_sysfs_all, DetectedUsb};
use crate::device::Model;
use crate::devices::{config_ports, find_model, models};
use crate::error::{Error, Result};
use crate::model::Mode;
use crate::protocol::bytes::{hex, read_u16_le, read_u8};
use crate::protocol::framing::Framing;
use crate::protocol::wire::{build_input_stream, build_start_config};
use crate::protocol::wire_write::PACKET_LEN;

const SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";
/// Reply budget on the read path: the vendor app reads up to 10 times, 200 ms each.
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Reply budget on the write path.
pub(super) const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Wait after the pause command so the input stream stops before the drain.
const PAUSE_SETTLE: Duration = Duration::from_millis(500);
/// Upper bound on reports discarded by one drain, in case the stream never stops.
const DRAIN_MAX_REPORTS: usize = 512;
/// Poll interval while waiting for the controller to come back on USB.
const REENUMERATE_POLL: Duration = Duration::from_millis(50);
/// Bytes of a frame that the failure log shows: the command header, which ends where
/// the profile payload starts (byte 18).
const LOG_HEADER_LEN: usize = 18;
/// Request byte that carries the command, for the log.
const REQUEST_CMD_OFFSET: usize = 2;
/// `START_CONFIG` reply bytes that carry the model id (little-endian).
const MODEL_ID_OFFSET: usize = 22;
/// `START_CONFIG` reply bytes that carry the firmware version times 100
/// (little-endian), as the vendor app reads it (`HIDManage.readGamePadNewVersion`).
const FIRMWARE_VERSION_OFFSET: usize = 18;
/// `START_CONFIG` reply byte that carries the firmware beta number, `0` for a release.
const FIRMWARE_BETA_OFFSET: usize = 20;

/// A controller to talk to: the USB port path it sits on, such as `8-5`. With no port,
/// the first supported controller found. The session finds out the model.
#[derive(Debug, Clone, Copy)]
pub(super) struct Target<'a> {
    /// The USB port path, or `None` for the first controller found.
    pub(super) port: Option<&'a str>,
}

impl Target<'_> {
    /// The target's USB device, if present.
    fn find(self) -> Option<DetectedUsb> {
        scan_sysfs_all(Path::new(SYSFS_USB_DEVICES), &config_ports())
            .into_iter()
            .find(|usb| self.port.is_none_or(|p| usb.port_path() == p))
    }
}

/// An open config node. Dropping it resumes the input stream.
pub(super) struct Session {
    file: File,
    timeout: Duration,
    /// Set once the pause went out, so `Drop` sends the resume.
    paused: bool,
    /// Framing of the current mode.
    pub(super) framing: Framing,
    /// The current mode, from the USB id the controller enumerated with.
    pub(super) current_mode: Mode,
    /// The mode to flip to before a write, from the matched config port.
    pub(super) write_via: Option<Mode>,
    /// The firmware version from the `START_CONFIG` reply, such as `1.04`.
    pub(super) firmware_version: String,
    /// The model the `START_CONFIG` reply named. [`Self::open`] always sets it.
    model: Option<&'static dyn Model>,
    /// The last frame written, as sent on the wire. For the failure log.
    last_sent: [u8; PACKET_LEN],
    /// The last report read, as it came off the wire. For the failure log.
    last_received: Option<Vec<u8>>,
}

impl Session {
    /// Finds the target controller, opens its config node, pauses its input and
    /// identifies the model from the `START_CONFIG` reply.
    ///
    /// Every current mode is identified: several 8BitDo pads share one USB id, so
    /// the id alone does not name the model. The pause and `START_CONFIG` are the
    /// only packets sent before the model id is confirmed. The Pro 3 answers nothing
    /// in Switch position until its input is paused. On a wrapped id (shared with a
    /// genuine Nintendo Pro Controller) both carry `0x00` at wire byte 10, the no-op
    /// subcommand of a Nintendo pad, and if `START_CONFIG` gets no reply nothing
    /// more is sent, not even the resume.
    ///
    /// # Errors
    /// Returns [`Error::NoDevice`] when no supported controller is attached.
    /// Returns [`Error::UnsupportedModel`] when the pad answers with a model id no
    /// supported model lists, or with one whose model does not use this config port. Returns [`Error::PermissionDenied`] when the node may
    /// not be opened, [`Error::Usb`] when it is missing or fails to open otherwise,
    /// and the errors of [`Self::send_recv`].
    pub(super) fn open(to: Target<'_>, timeout: Duration) -> Result<Self> {
        let found = to.find().ok_or(Error::NoDevice)?;
        let node =
            config_hidraw(Path::new(&found.sysfs_path), found.port.interface).ok_or_else(|| {
                Error::Usb(format!(
                    "no hidraw node on interface {} of {}:{}",
                    found.port.interface, found.vendor_id, found.product_id
                ))
            })?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&node)
            .map_err(|e| open_error(&node, &e))?;
        let mut session = Self {
            file,
            timeout,
            paused: false,
            framing: found.port.framing,
            current_mode: found.port.mode,
            write_via: found.port.write_via,
            firmware_version: String::new(),
            model: None,
            last_sent: [0; PACKET_LEN],
            last_received: None,
        };
        session.pause().inspect_err(|e| warn!("cannot pause the input stream: {e}"))?;
        let reply = match session.send_recv(&build_start_config()) {
            Ok(reply) => reply,
            Err(e) => {
                // A wrapped id that stays silent may be a Nintendo pad: send nothing more.
                session.paused = session.framing != Framing::Wrapped;
                return Err(e);
            }
        };
        session.model = Some(
            identify(&reply, models(), &found)
                .inspect_err(|e| warn!("cannot identify the controller: {e}"))?,
        );
        let model = read_u16_le(&reply, MODEL_ID_OFFSET)?;
        session.firmware_version = firmware_version(&reply)
            .inspect_err(|e| warn!("cannot read the firmware version: {e}"))?;
        info!(
            "controller opened: model 0x{model:04x}, mode {:?}, firmware {}",
            session.current_mode, session.firmware_version
        );
        Ok(session)
    }

    /// The model the controller named when the session opened.
    ///
    /// # Errors
    /// Never in practice: [`Self::open`] fails rather than return an unidentified
    /// session. Returns [`Error::Decode`] if that ever changes.
    pub(super) fn model(&self) -> Result<&'static dyn Model> {
        self.model.ok_or_else(|| Error::Decode("the session has not identified the model".into()))
    }

    /// Pauses input, waits for the stream to stop, then drops what is queued.
    /// The pause reply is not awaited: it may never come.
    fn pause(&mut self) -> Result<()> {
        self.paused = true;
        self.write(&build_input_stream(false))?;
        std::thread::sleep(PAUSE_SETTLE);
        let mut buf = [0u8; PACKET_LEN];
        for _ in 0..DRAIN_MAX_REPORTS {
            if !self.readable(Duration::ZERO)? {
                break;
            }
            let _ = self.file.read(&mut buf).map_err(|e| io_error(&e))?;
        }
        Ok(())
    }

    fn write(&mut self, packet: &[u8; PACKET_LEN]) -> Result<()> {
        self.last_sent = self.framing.request(packet);
        trace!("out {}", hex(&self.last_sent));
        self.file.write_all(&self.last_sent).map_err(|e| io_error(&e))
    }

    /// Logs a failed command at `warn` with the header of the last frame sent and of
    /// the last report read, as hex. The payload is withheld: a profile chunk carries
    /// the profile name. Only `trace` shows whole frames.
    pub(super) fn log_failure(&self, packet: &[u8; PACKET_LEN], error: &Error) {
        let cmd = read_u8(packet, REQUEST_CMD_OFFSET).unwrap_or_default();
        warn!(
            "command 0x{cmd:02x} failed: {error}; sent {}; received {}",
            header_hex(&self.last_sent),
            self.last_received.as_deref().map_or_else(|| "nothing".to_owned(), header_hex)
        );
    }

    /// Waits up to `timeout` for a report. `Ok(false)` on timeout or a signal.
    fn readable(&self, timeout: Duration) -> Result<bool> {
        let ts = Timespec::try_from(timeout).map_err(|_| Error::Timeout)?;
        let mut fds = [PollFd::new(&self.file, PollFlags::IN)];
        match poll(&mut fds, Some(&ts)) {
            Ok(n) => Ok(n > 0),
            Err(Errno::INTR) => Ok(false),
            Err(e) => Err(errno_error(e)),
        }
    }

    /// Sends one normal-layout packet and returns its reply in normal layout.
    ///
    /// Reports that are not the reply (gamepad input, other echoes) are skipped
    /// until the session's reply budget runs out. The command, its duration and the
    /// skipped count go to the log, and a failure also logs the raw frames.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] when no reply arrives in time,
    /// [`Error::Disconnected`] when the device goes away, and [`Error::Usb`] on
    /// any other I/O failure.
    pub(super) fn send_recv(&mut self, packet: &[u8; PACKET_LEN]) -> Result<Vec<u8>> {
        let start = Instant::now();
        let mut skipped = 0usize;
        self.last_received = None;
        let result = self.exchange(packet, &mut skipped);
        match &result {
            Ok(_) => debug!(
                "command 0x{:02x} ok in {} ms, {skipped} other reports skipped",
                read_u8(packet, REQUEST_CMD_OFFSET).unwrap_or_default(),
                start.elapsed().as_millis()
            ),
            Err(e) => self.log_failure(packet, e),
        }
        result
    }

    /// The send and the wait of [`Self::send_recv`]; counts the reports it skips.
    fn exchange(&mut self, packet: &[u8; PACKET_LEN], skipped: &mut usize) -> Result<Vec<u8>> {
        self.write(packet)?;
        let deadline = Instant::now() + self.timeout;
        let mut buf = [0u8; PACKET_LEN];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            if !self.readable(left)? {
                continue;
            }
            let n = self.file.read(&mut buf).map_err(|e| io_error(&e))?;
            let raw = buf.get(..n).unwrap_or(&buf);
            trace!("in {}", hex(raw));
            self.last_received = Some(raw.to_vec());
            if let Some(reply) = self.framing.reply(raw, packet) {
                return Ok(reply);
            }
            *skipped += 1;
        }
    }
}

impl Session {
    /// Sends a packet after which the controller drops off USB (a mode flip or close),
    /// then ends the session without the resume. Takes the reply if one comes first.
    /// A disconnect is the expected outcome, so it is not an error.
    ///
    /// # Errors
    /// Returns [`Error::Usb`] on any other I/O failure. A missing reply is not one.
    pub(super) fn send_last(mut self, packet: &[u8; PACKET_LEN]) -> Result<()> {
        self.paused = false;
        match self.send_recv(packet) {
            Ok(_) | Err(Error::Disconnected | Error::Timeout) => Ok(()),
            Err(e) => Err(e),
        }
    }
}

/// Waits until the target controller is back on USB in `mode` and its config node
/// opens.
///
/// The node can refuse an open for a moment after it appears, until udev tags it,
/// so every failure is retried until `budget` runs out.
///
/// # Errors
/// Returns [`Error::Timeout`] when the controller is not back in time, or the last
/// [`Error::PermissionDenied`] if only the open kept failing.
pub(super) fn wait_for_mode(to: Target<'_>, mode: Mode, budget: Duration) -> Result<()> {
    let deadline = Instant::now() + budget;
    let mut last = Error::Timeout;
    while Instant::now() < deadline {
        let node = to
            .find()
            .filter(|found| found.port.mode == mode)
            .and_then(|found| config_hidraw(Path::new(&found.sysfs_path), found.port.interface));
        if let Some(node) = node {
            match OpenOptions::new().read(true).write(true).open(&node) {
                Ok(_) => return Ok(()),
                Err(e) => last = open_error(&node, &e),
            }
        }
        std::thread::sleep(REENUMERATE_POLL);
    }
    Err(match last {
        e @ Error::PermissionDenied(_) => e,
        _ => Error::Timeout,
    })
}

impl Drop for Session {
    fn drop(&mut self) {
        if self.paused {
            // Best effort: after a disconnect there is nothing left to resume.
            let _ = self.write(&build_input_stream(true));
        }
    }
}

/// `EIO` then `ENODEV` is what a hidraw fd returns once the device is gone.
pub(super) fn errno_error(e: Errno) -> Error {
    if e == Errno::IO || e == Errno::NODEV {
        Error::Disconnected
    } else {
        Error::Usb(e.to_string())
    }
}

pub(super) fn io_error(e: &std::io::Error) -> Error {
    Errno::from_io_error(e).map_or_else(|| Error::Usb(e.to_string()), errno_error)
}

/// Splits a denied open of the hidraw node from other open failures.
pub(super) fn open_error(node: &Path, e: &std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        Error::PermissionDenied(node.display().to_string())
    } else {
        Error::Usb(format!("cannot open {}: {e}", node.display()))
    }
}

/// The model among `models` that a `START_CONFIG` reply names, checked against the
/// config port the controller was found on.
fn identify(
    reply: &[u8],
    models: &[&'static dyn Model],
    found: &DetectedUsb,
) -> Result<&'static dyn Model> {
    let id = read_u16_le(reply, MODEL_ID_OFFSET)?;
    let model = find_model(models, id).ok_or(Error::UnsupportedModel(id))?;
    if model.description()?.config_ports.contains(&found.port) {
        Ok(model)
    } else {
        Err(Error::UnsupportedModel(id))
    }
}

/// The first [`LOG_HEADER_LEN`] bytes of a frame as hex, with a note of what was held
/// back.
fn header_hex(frame: &[u8]) -> String {
    let (head, payload) = frame.split_at(frame.len().min(LOG_HEADER_LEN));
    if payload.is_empty() {
        hex(head)
    } else {
        format!("{} (+{} payload bytes withheld)", hex(head), payload.len())
    }
}

/// Reads the firmware version from a `START_CONFIG` reply: `1.04`, or
/// `1.04 beta 12` for a beta build.
fn firmware_version(reply: &[u8]) -> Result<String> {
    let raw = read_u16_le(reply, FIRMWARE_VERSION_OFFSET)?;
    let version = format!("{}.{:02}", raw / 100, raw % 100);
    Ok(match read_u8(reply, FIRMWARE_BETA_OFFSET)? {
        0 => version,
        beta => format!("{version} beta {beta}"),
    })
}

#[cfg(test)]
#[allow(clippy::indexing_slicing, clippy::unwrap_used, clippy::significant_drop_tightening)]
mod tests {
    use std::os::unix::net::UnixStream;
    use std::sync::Mutex;

    use super::*;
    use crate::device::{ConfigPort, ControllerSpec as _};
    use crate::devices::pro3::Pro3;
    use crate::protocol::wire_write::build_write_packet;

    /// Keeps every record, so a test can read back what the session logged.
    struct Capture(Mutex<Vec<(log::Level, String)>>);

    impl log::Log for Capture {
        fn enabled(&self, _: &log::Metadata<'_>) -> bool {
            true
        }
        fn log(&self, record: &log::Record<'_>) {
            self.0.lock().unwrap().push((record.level(), record.args().to_string()));
        }
        fn flush(&self) {}
    }

    static CAPTURE: Capture = Capture(Mutex::new(Vec::new()));

    #[test]
    fn timeout_logs_one_warn_with_both_frames() {
        log::set_logger(&CAPTURE).unwrap();
        log::set_max_level(log::LevelFilter::Trace);
        let (ours, mut peer) = UnixStream::pair().unwrap();
        let mut session = Session {
            file: File::from(std::os::fd::OwnedFd::from(ours)),
            timeout: Duration::from_millis(50),
            paused: false,
            framing: Framing::Plain,
            current_mode: Mode::DInput,
            write_via: None,
            firmware_version: String::new(),
            model: None,
            last_sent: [0; PACKET_LEN],
            last_received: None,
        };
        // A gamepad input report: read, not the reply, so the exchange goes on to time out.
        peer.write_all(&[0x30, 0x01, 0x02]).unwrap();
        // A profile chunk: bytes 18 and up are the payload, which holds the name.
        let request = build_write_packet(0, &[0xA7; 45], 0x092C);
        assert!(matches!(session.send_recv(&request), Err(Error::Timeout)));

        let lines = CAPTURE.0.lock().unwrap();
        let warns: Vec<_> = lines.iter().filter(|(level, _)| *level == log::Level::Warn).collect();
        assert_eq!(warns.len(), 1);
        let line = &warns[0].1;
        assert!(line.contains(&hex(&request[..LOG_HEADER_LEN])), "sent header missing: {line}");
        assert!(line.contains("payload bytes withheld"), "{line}");
        assert!(!line.contains("a7 a7"), "payload reached the log: {line}");
        assert!(line.contains("received 30 01 02"), "received frame missing: {line}");
        // The core logs frames and numbers only: no home path reaches a line.
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_owned());
        assert!(lines.iter().all(|(_, text)| !text.contains(&home)));
    }

    fn reply_with(model: u16) -> Vec<u8> {
        let mut reply = vec![0u8; PACKET_LEN];
        reply[MODEL_ID_OFFSET..MODEL_ID_OFFSET + 2].copy_from_slice(&model.to_le_bytes());
        reply
    }

    #[test]
    fn firmware_version_reads_the_vendor_bytes() -> Result<()> {
        let mut reply = vec![0u8; PACKET_LEN];
        reply[FIRMWARE_VERSION_OFFSET..FIRMWARE_VERSION_OFFSET + 2]
            .copy_from_slice(&104u16.to_le_bytes());
        assert_eq!(firmware_version(&reply)?, "1.04");
        reply[FIRMWARE_BETA_OFFSET] = 12;
        assert_eq!(firmware_version(&reply)?, "1.04 beta 12");
        Ok(())
    }

    #[test]
    fn denied_open_is_permission_denied() {
        let node = Path::new("/dev/hidraw3");
        let denied = std::io::Error::from_raw_os_error(Errno::ACCESS.raw_os_error());
        assert!(
            matches!(open_error(node, &denied), Error::PermissionDenied(n) if n == "/dev/hidraw3")
        );
        let missing = std::io::Error::from(std::io::ErrorKind::NotFound);
        assert!(matches!(open_error(node, &missing), Error::Usb(_)));
    }

    #[test]
    fn identify_accepts_only_listed_models() -> Result<()> {
        let port = Pro3.description()?.config_ports[0];
        let found = DetectedUsb {
            vendor_id: String::new(),
            product_id: String::new(),
            sysfs_path: String::new(),
            port,
        };
        assert_eq!(identify(&reply_with(0x6009), models(), &found)?.blob_size(), 0x092C);
        assert!(identify(&reply_with(0x600A), models(), &found).is_ok());
        // Ultimate 2: same USB id 2dc8:310b in XInput, different model id.
        assert!(matches!(
            identify(&reply_with(0x6012), models(), &found),
            Err(Error::UnsupportedModel(0x6012))
        ));
        assert!(matches!(identify(&[0u8; 10], models(), &found), Err(Error::Decode(_))));
        // A listed id on a port its model does not use.
        let other = DetectedUsb { port: ConfigPort { interface: 9, ..port }, ..found };
        assert!(matches!(
            identify(&reply_with(0x6009), models(), &other),
            Err(Error::UnsupportedModel(0x6009))
        ));
        Ok(())
    }
}
