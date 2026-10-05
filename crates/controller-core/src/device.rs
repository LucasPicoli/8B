//! Per-controller abstraction seam: static spec + pure protocol codec.

use crate::description::ControllerDescription;
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::framing::Framing;
use crate::protocol::wire_write::PACKET_LEN;

/// A supported USB (vendor, product) pair. JSON writes each as `"0x2dc8"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbId {
    /// USB vendor id.
    #[serde(deserialize_with = "crate::description::hex_u16")]
    pub vendor: u16,
    /// USB product id.
    #[serde(deserialize_with = "crate::description::hex_u16")]
    pub product: u16,
}

/// The config interface a controller presents in one current mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigPort {
    /// USB id the controller enumerates with in this current mode.
    pub usb: UsbId,
    /// The current mode this USB id stands for.
    pub mode: Mode,
    /// USB interface number whose hidraw node carries the config reports.
    pub interface: u8,
    /// Framing of config packets on that node.
    pub framing: Framing,
    /// The mode to flip the controller to before a write, when this current mode takes
    /// no writes in place. `None` means writes go out in this mode.
    #[serde(default)]
    pub write_via: Option<Mode>,
}

/// Protocol bytes of a controller model, plus a pointer to its description.
pub trait ControllerSpec {
    /// The model's embedded controller description, parsed once.
    ///
    /// # Errors
    /// Returns [`crate::Error::Decode`] if the embedded file is malformed. A unit test
    /// loads every embedded description, so a shipped build never returns it.
    fn description(&self) -> Result<&'static ControllerDescription>;
    /// Profile blob size in bytes.
    fn blob_size(&self) -> usize;
    /// Substring used to match the joydev device name.
    fn joydev_name_match(&self) -> &'static str;
    /// Value sent in the slot-select command (`0x14`) to target `mode`'s slots.
    fn slot_select_value(&self, mode: Mode) -> u8;
    /// Gamepad-mode byte carried by the macro commands for `mode`.
    fn macro_gamepad_mode(&self, mode: Mode) -> u8;
    /// Normal-layout packet that makes the controller re-enumerate in `target` mode
    /// until it is closed or replugged. `None` if the model cannot flip to `target`.
    fn mode_flip_command(&self, target: Mode) -> Option<[u8; PACKET_LEN]>;
    /// Normal-layout packet that sends a flipped controller back to the mode its
    /// slide switch shows.
    fn mode_close_command(&self) -> [u8; PACKET_LEN];
}

/// Pure byte-level codec for a controller model (no device I/O).
pub trait ProtocolCodec {
    /// Decodes a raw blob into a canonical profile summary.
    ///
    /// # Errors
    /// Returns [`crate::Error::Decode`] on malformed input.
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary>;

    /// The profile a new slot of `mode` starts from, with an empty name.
    fn default_profile(&self, mode: Mode) -> CanonicalProfile;

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

    /// Encodes macro steps into the padded wire step stream.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] on an unknown step-button name.
    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>>;

    /// Encodes a macro's 52-byte Section-4 metadata descriptor.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] on an unknown trigger or step-count overflow.
    fn encode_macro_metadata(
        &self,
        def: &MacroDefinition,
        macro_slot: MacroSlot,
    ) -> Result<Vec<u8>>;

    /// Compiles a canonical profile into the device-native 2348-byte blob.
    ///
    /// `base_blob` (when 2348 bytes) is the read-modify-write baseline whose
    /// non-target slots are preserved; otherwise a fresh zeroed blob is used.
    /// `macros` are already-resolved Section-4 descriptors for the target slot.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] on an unknown control/trigger name, an
    /// out-of-range slot, or an encoding overflow.
    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>>;

    /// Like [`Self::compile_profile`], but keeps the macro descriptors `base_blob` already
    /// holds for `target_slot`, byte for byte, instead of clearing them.
    ///
    /// # Errors
    /// Same as [`Self::compile_profile`].
    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>>;

    /// Returns `blob` with the macros of `slot` that `triggers` start removed. The other
    /// macros stay byte for byte.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] if `blob` is not a full profile blob.
    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>>;

    /// Returns `base_blob` with `slot` deactivated. Macros and other slots stay as read.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] if `base_blob` is not a full profile blob.
    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>>;

    /// Checks a button remap request for `mode`.
    ///
    /// # Errors
    /// Returns [`crate::Error::Validation`] with a message fit to show to the user.
    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()>;
}
