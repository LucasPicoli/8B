//! The `DeviceIo` trait: device operations the services/orchestrators depend on.

use crate::device::Model;
use crate::error::Result;
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};

/// Where one macro's step stream sits, and how many steps to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacroStream {
    /// The bank the macro belongs to.
    pub mode: Mode,
    /// The profile slot that holds the macro.
    pub profile_slot: Slot,
    /// The macro slot, which picks the flash page.
    pub macro_slot: MacroSlot,
    /// The steps to read, from the macro's descriptor.
    pub step_count: usize,
}

/// Device read and write operations.
///
/// Callers sequence the protocol steps (slot select, write, apply). Between
/// [`Self::begin_write`] and [`Self::end_write`] every call runs in the one session the
/// write job holds open; outside a write job each call opens its own.
pub trait DeviceIo {
    /// The model of the attached controller, from the model id its `START_CONFIG`
    /// reply carries. Opens a session unless an earlier one already identified it.
    ///
    /// # Errors
    /// Returns a connection error if no supported device is present, and
    /// [`crate::Error::UnsupportedModel`] for a model id no supported model lists.
    fn model(&self) -> Result<&'static dyn Model>;

    /// The mode the controller says it is in, for a pad whose modes do not change its
    /// USB id. `None`, the default, means the USB id of its config port tells the mode.
    ///
    /// The app asks on every presence poll, about once a second, so answer from what
    /// the last session saw or from a cheap query.
    fn current_mode(&self) -> Option<Mode> {
        None
    }

    /// Reads the profiles of every mode's bank, in the order of the model's
    /// description, from any current mode. `raw_blobs` holds one blob per bank in the
    /// same order.
    ///
    /// # Errors
    /// Returns a connection/timeout/decode error on failure.
    fn read_all_profiles(&self) -> Result<ProfileReadResult>;

    /// Reads the raw step streams of several macros from flash, in one session, in the
    /// order asked. The macros may sit in any bank. The read leaves gamepad input on.
    ///
    /// # Errors
    /// Returns a connection/timeout error on failure.
    fn read_macro_streams(&self, streams: &[MacroStream]) -> Result<Vec<Vec<u8>>>;

    /// Reads the raw step stream of one macro from flash.
    ///
    /// # Errors
    /// Same as [`Self::read_macro_streams`].
    fn read_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        step_count: usize,
    ) -> Result<Vec<u8>> {
        let stream = MacroStream { mode, profile_slot, macro_slot, step_count };
        Ok(self.read_macro_streams(&[stream])?.into_iter().next().unwrap_or_default())
    }

    /// Probes device readiness (presence + active slot marker).
    ///
    /// # Errors
    /// Returns a connection error if no supported device is present.
    fn detect_readiness(&self) -> Result<DeviceReadiness>;

    /// Readies the controller for one write job and opens the session the job runs in.
    /// When its current mode takes no writes in place (the config port's `write_via`),
    /// flips it to that mode and waits for it to come back on USB. Returns the mode to
    /// pass to [`Self::end_write`], or `None` when nothing was flipped. Every call that
    /// opened a write job ends it with [`Self::end_write`], on every path.
    ///
    /// # Errors
    /// Returns a connection error if the device cannot be opened, and
    /// [`crate::Error::Write`] if the controller does not come back after the flip.
    fn begin_write(&self) -> Result<Option<Mode>>;

    /// Ends the write job that [`Self::begin_write`] opened. With `Some(mode)`, sends
    /// the flipped controller back to `mode`, the mode its slide switch shows, and
    /// waits for it; a replug does the same. With `None`, closes the session, which
    /// resumes gamepad input.
    ///
    /// # Errors
    /// Returns a connection error if the device cannot be opened, and
    /// [`crate::Error::Timeout`] if it does not come back in time. `None` never fails.
    fn end_write(&self, back_to: Option<Mode>) -> Result<()>;

    /// Writes a complete profile blob of the model's blob size in 45-byte chunks, each
    /// ACK-checked. A Pro 3 blob is 2348 bytes in 53 chunks.
    ///
    /// The target slot is encoded in the blob itself, so there is no slot argument.
    /// [`Self::send_slot_select`] must come first or the device ACKs but ignores the write.
    ///
    /// # Errors
    /// Returns [`crate::Error::Write`] (with the failed chunk index) on a wrong blob size,
    /// a transfer failure or a rejected response. Returns a connection error if the
    /// device cannot be opened.
    fn write_full_profile(&self, mode: Mode, blob: &[u8]) -> Result<()>;

    /// Sends the slot-select command that must precede any profile write.
    ///
    /// # Errors
    /// Returns [`crate::Error::Timeout`] on a transfer failure, [`crate::Error::Write`] on a
    /// rejected response, or a connection error if the device cannot be opened.
    fn send_slot_select(&self, mode: Mode) -> Result<()>;

    /// Sends `PROFILE_APPLY` to activate what was written.
    ///
    /// # Errors
    /// Same as [`Self::send_slot_select`].
    fn send_apply(&self, mode: Mode) -> Result<()>;

    /// Erases the flash page of one macro slot, then writes `stream`, the padded step
    /// data, in 32-byte chunks, each ACK-checked. The erase comes first, so a shorter
    /// macro leaves nothing of a longer one.
    ///
    /// # Errors
    /// Returns [`crate::Error::Write`] if `stream` is empty, not a multiple of 32 bytes or
    /// longer than one page, or (with the failed chunk index) on a transfer failure or a
    /// rejected response. Returns a connection error if the device cannot be opened.
    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()>;
}
