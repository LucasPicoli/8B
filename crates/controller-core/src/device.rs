//! Per-controller abstraction seam: static spec + pure protocol codec.

use crate::error::Result;
use crate::model::{
    CanonicalProfileSummary, MacroDefinition, MacroStep, Mode, RawProfilePayload, Slot,
};

/// A supported USB (vendor, product) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UsbId {
    /// USB vendor id.
    pub vendor: u16,
    /// USB product id.
    pub product: u16,
}

/// Per-mode USB transport parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportParams {
    /// Interface number to claim.
    pub interface: u8,
    /// Interrupt OUT endpoint address.
    pub ep_out: u8,
    /// Interrupt IN endpoint address.
    pub ep_in: u8,
    /// Offset into the 64-byte packet where the payload begins.
    pub payload_offset: usize,
}

/// Static description of a controller model.
pub trait ControllerSpec {
    /// Supported (vendor, product) pairs.
    fn usb_ids(&self) -> &[UsbId];
    /// Supported operating modes.
    fn modes(&self) -> &[Mode];
    /// Transport parameters for `mode`.
    fn transport_params(&self, mode: Mode) -> TransportParams;
    /// Number of profile slots.
    fn slot_count(&self) -> u8;
    /// Number of macro slots per profile slot.
    fn macro_slot_count(&self) -> u8;
    /// Profile blob size in bytes.
    fn blob_size(&self) -> usize;
    /// Substring used to match the joydev device name.
    fn joydev_name_match(&self) -> &'static str;
    /// USB product id used for `mode`.
    fn product_id_for_mode(&self, mode: Mode) -> u16;
}

/// Pure byte-level codec for a controller model (no device I/O).
pub trait ProtocolCodec {
    /// Decodes a raw blob into a canonical profile summary.
    ///
    /// # Errors
    /// Returns [`crate::Error::Decode`] on malformed input.
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary>;

    /// Decodes Section-4 macro metadata for `profile_slot` (steps left empty).
    ///
    /// # Errors
    /// Returns [`crate::Error::Decode`] on malformed input.
    fn decode_macro_metadata(
        &self,
        blob: &[u8],
        profile_slot: Slot,
    ) -> Result<Vec<MacroDefinition>>;

    /// Decodes a raw macro step stream into steps.
    ///
    /// # Errors
    /// Returns [`crate::Error::Decode`] on malformed input.
    fn decode_macro_steps(
        &self,
        stream: &[u8],
        step_count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>>;
}
