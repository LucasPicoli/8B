//! One open hidraw node of the config interface, with a blocking send/receive primitive.
//!
//! A session pauses the gamepad input stream when it opens and resumes it when it is
//! dropped, error paths included. While paused, config replies do not race the input
//! reports on the shared IN endpoint (`DInput` and Switch).

use std::fs::{File, OpenOptions};
use std::io::{Read as _, Write as _};
use std::path::Path;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

use crate::detect::{config_hidraw, scan_sysfs};
use crate::device::ControllerSpec as _;
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::Mode;
use crate::protocol::bytes::read_u16_le;
use crate::protocol::framing::Framing;
use crate::protocol::wire::{build_input_stream, build_start_config};
use crate::protocol::wire_write::PACKET_LEN;

const SYSFS_USB_DEVICES: &str = "/sys/bus/usb/devices";
/// Reply budget on the read path: the vendor app reads up to 10 times, 200 ms each.
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(2);
/// Reply budget on the write path (the C++ default).
pub(super) const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
/// Wait after the pause command so the input stream stops before the drain.
const PAUSE_SETTLE: Duration = Duration::from_millis(500);
/// Upper bound on reports discarded by one drain, in case the stream never stops.
const DRAIN_MAX_REPORTS: usize = 512;
/// `START_CONFIG` reply bytes that carry the model id (little-endian).
const MODEL_ID_OFFSET: usize = 22;

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
}

impl Session {
    /// Finds the attached controller, opens its config node, pauses its input and
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
    /// Returns [`Error::UnsupportedModel`] when the pad answers with a model id
    /// `spec` does not list. Returns [`Error::PermissionDenied`] when the node may
    /// not be opened, [`Error::Usb`] when it is missing or fails to open otherwise,
    /// and the errors of [`Self::send_recv`].
    pub(super) fn open(spec: Pro3, timeout: Duration) -> Result<Self> {
        let found = scan_sysfs(Path::new(SYSFS_USB_DEVICES), &spec.description()?.config_ports)
            .ok_or(Error::NoDevice)?;
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
        };
        session.pause()?;
        let reply = match session.send_recv(&build_start_config()) {
            Ok(reply) => reply,
            Err(e) => {
                // A wrapped id that stays silent may be a Nintendo pad: send nothing more.
                session.paused = session.framing == Framing::Plain;
                return Err(e);
            }
        };
        identify(&reply, &spec.description()?.model_ids)?;
        Ok(session)
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
        self.file.write_all(&self.framing.request(packet)).map_err(|e| io_error(&e))
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
    /// until the session's reply budget runs out.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] when no reply arrives in time,
    /// [`Error::Disconnected`] when the device goes away, and [`Error::Usb`] on
    /// any other I/O failure.
    pub(super) fn send_recv(&mut self, packet: &[u8; PACKET_LEN]) -> Result<Vec<u8>> {
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
            if let Some(reply) = self.framing.reply(buf.get(..n).unwrap_or(&buf), packet) {
                return Ok(reply);
            }
        }
    }
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
fn errno_error(e: Errno) -> Error {
    if e == Errno::IO || e == Errno::NODEV {
        Error::Disconnected
    } else {
        Error::Usb(e.to_string())
    }
}

fn io_error(e: &std::io::Error) -> Error {
    Errno::from_io_error(e).map_or_else(|| Error::Usb(e.to_string()), errno_error)
}

/// Splits a denied open of the hidraw node from other open failures.
fn open_error(node: &Path, e: &std::io::Error) -> Error {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        Error::PermissionDenied(node.display().to_string())
    } else {
        Error::Usb(format!("cannot open {}: {e}", node.display()))
    }
}

/// Checks the model id in a `START_CONFIG` reply against `supported`.
fn identify(reply: &[u8], supported: &[u16]) -> Result<()> {
    let model = read_u16_le(reply, MODEL_ID_OFFSET)?;
    if supported.contains(&model) {
        Ok(())
    } else {
        Err(Error::UnsupportedModel(model))
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn reply_with(model: u16) -> Vec<u8> {
        let mut reply = vec![0u8; PACKET_LEN];
        reply[MODEL_ID_OFFSET..MODEL_ID_OFFSET + 2].copy_from_slice(&model.to_le_bytes());
        reply
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
        let pro3 = &Pro3.description()?.model_ids;
        assert!(identify(&reply_with(0x6009), pro3).is_ok());
        assert!(identify(&reply_with(0x600A), pro3).is_ok());
        // Ultimate 2: same USB id 2dc8:310b in XInput, different model id.
        assert!(matches!(
            identify(&reply_with(0x6012), pro3),
            Err(Error::UnsupportedModel(0x6012))
        ));
        assert!(matches!(identify(&[0u8; 10], pro3), Err(Error::Decode(_))));
        Ok(())
    }
}
