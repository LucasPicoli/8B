//! The `DeviceIo` trait: device operations the services/orchestrators depend on.

use crate::device::Model;
use crate::error::Result;
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};

/// Device read and write operations.
///
/// Every write method opens its own USB session and releases it before returning,
/// so callers sequence the protocol steps (slot select, write, apply).
pub trait DeviceIo {
    /// The model of the attached controller, from the model id its `START_CONFIG`
    /// reply carries. Opens a session unless an earlier one already identified it.
    ///
    /// # Errors
    /// Returns a connection error if no supported device is present, and
    /// [`crate::Error::UnsupportedModel`] for a model id no supported model lists.
    fn model(&self) -> Result<&'static dyn Model>;

    /// Reads the profiles of every mode's bank, in the order of the model's
    /// description, from any current mode. `raw_blobs` holds one blob per bank in the
    /// same order.
    ///
    /// # Errors
    /// Returns a connection/timeout/decode error on failure.
    fn read_all_profiles(&self) -> Result<ProfileReadResult>;

    /// Reads a raw macro step stream from flash.
    ///
    /// # Errors
    /// Returns a connection/timeout error on failure.
    fn read_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        step_count: usize,
    ) -> Result<Vec<u8>>;

    /// Probes device readiness (presence + active slot marker).
    ///
    /// # Errors
    /// Returns a connection error if no supported device is present.
    fn detect_readiness(&self) -> Result<DeviceReadiness>;

    /// Readies the controller for one write job. When its current mode takes no writes
    /// in place (the config port's `write_via`), flips it to that mode and waits for it
    /// to come back on USB. Returns the mode to pass to [`Self::end_write`], or `None`
    /// when nothing was flipped.
    ///
    /// # Errors
    /// Returns a connection error if the device cannot be opened, and
    /// [`crate::Error::Write`] if the controller does not come back after the flip.
    fn begin_write(&self) -> Result<Option<Mode>>;

    /// Sends a controller flipped by [`Self::begin_write`] back to `back_to`, the mode
    /// its slide switch shows, and waits for it. A replug does the same.
    ///
    /// # Errors
    /// Returns a connection error if the device cannot be opened, and
    /// [`crate::Error::Timeout`] if it does not come back in time.
    fn end_write(&self, back_to: Mode) -> Result<()>;

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

    /// Writes `data` at blob `offset` (the same packet twice, as the protocol requires).
    ///
    /// # Errors
    /// Returns [`crate::Error::Write`] on empty data, a transfer failure or a rejected
    /// response. Returns a connection error if the device cannot be opened.
    fn write_patch(&self, mode: Mode, offset: u16, data: &[u8]) -> Result<()>;

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

    /// Sends `QUERY_STATUS`. Disrupts joydev input until the device is reconnected.
    ///
    /// # Errors
    /// Same as [`Self::send_slot_select`].
    fn query_status(&self, mode: Mode) -> Result<()>;

    /// Erases the 4096-byte flash page of one macro slot.
    ///
    /// Passing a macro slot beyond the controller's range is impossible: [`MacroSlot`]
    /// validates it.
    ///
    /// # Errors
    /// Same as [`Self::send_slot_select`].
    fn erase_macro(&self, mode: Mode, profile_slot: Slot, macro_slot: MacroSlot) -> Result<()>;

    /// Erases a macro slot, then writes `stream` (padded step data) in 32-byte chunks.
    ///
    /// # Errors
    /// Returns [`crate::Error::Write`] if `stream` is empty or not a multiple of 32 bytes,
    /// plus every error of [`Self::erase_macro`].
    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()>;
}
