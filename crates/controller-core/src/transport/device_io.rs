//! The `DeviceIo` trait: device operations the services/orchestrators depend on.

use crate::error::Result;
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};

/// Device read operations (Phase 1). Write operations are added in Plan 2.
pub trait DeviceIo {
    /// Reads all on-device profiles for the given mode's product.
    ///
    /// # Errors
    /// Returns a connection/timeout/decode error on failure.
    fn read_all_profiles(&self, mode: Mode) -> Result<ProfileReadResult>;

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
}
