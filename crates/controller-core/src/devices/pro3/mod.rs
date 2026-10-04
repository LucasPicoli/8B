//! 8BitDo Pro 3 controller backend.

pub mod edit;
pub mod macros;
pub mod profile;
pub mod tables;

use crate::device::{ConfigPort, ControllerSpec, ProtocolCodec, UsbId};
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::framing::Framing;

/// The 8BitDo Pro 3 controller backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pro3;

const CONFIG_PORTS: [ConfigPort; 3] = [
    ConfigPort {
        usb: UsbId { vendor: 0x2DC8, product: 0x310B },
        mode: Mode::XInput,
        interface: 2,
        framing: Framing::Plain,
    },
    // Nintendo's id: a genuine Pro Controller enumerates the same, so the transport
    // checks the model id before it sends anything else.
    ConfigPort {
        usb: UsbId { vendor: 0x057E, product: 0x2009 },
        mode: Mode::Switch,
        interface: 0,
        framing: Framing::Wrapped,
    },
    ConfigPort {
        usb: UsbId { vendor: 0x2DC8, product: 0x6009 },
        mode: Mode::DInput,
        interface: 0,
        framing: Framing::Plain,
    },
];
/// `START_CONFIG` model ids of the Pro 3 (the vendor app accepts both).
const MODEL_IDS: [u16; 2] = [0x6009, 0x600A];
const MODES: [Mode; 3] = [Mode::XInput, Mode::Switch, Mode::DInput];

impl ControllerSpec for Pro3 {
    fn config_ports(&self) -> &[ConfigPort] {
        &CONFIG_PORTS
    }
    fn model_ids(&self) -> &[u16] {
        &MODEL_IDS
    }
    fn modes(&self) -> &[Mode] {
        &MODES
    }
    fn write_payload_offset(&self, mode: Mode) -> usize {
        // The C++ oracle writes `DInput` payloads at 16; unverified on hardware.
        match mode {
            Mode::XInput | Mode::Switch => 18,
            Mode::DInput => 16,
        }
    }
    fn slot_count(&self) -> u8 {
        3
    }
    fn macro_slot_count(&self) -> u8 {
        4
    }
    fn blob_size(&self) -> usize {
        0x092C
    }
    fn joydev_name_match(&self) -> &'static str {
        "8BitDo"
    }
    fn slot_select_value(&self, mode: Mode) -> u8 {
        match mode {
            Mode::Switch => 0,
            Mode::DInput => 1,
            Mode::XInput => 3,
        }
    }
    fn macro_gamepad_mode(&self, mode: Mode) -> u8 {
        match mode {
            Mode::Switch => 0,
            Mode::DInput => 1,
            Mode::XInput => 3,
        }
    }
}

impl ProtocolCodec for Pro3 {
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        profile::map_profile(self, raw)
    }

    fn decode_macro_metadata(
        &self,
        blob: &[u8],
        profile_slot: Slot,
    ) -> Result<Vec<MacroDefinition>> {
        macros::decode_macro_metadata(blob, profile_slot)
    }

    fn decode_macro_steps(
        &self,
        stream: &[u8],
        step_count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>> {
        macros::decode_macro_steps(stream, step_count, mode)
    }

    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
        macros::encode_macro_steps(steps, mode)
    }

    fn encode_macro_metadata(
        &self,
        def: &MacroDefinition,
        macro_slot: MacroSlot,
    ) -> Result<Vec<u8>> {
        macros::encode_macro_metadata(def, macro_slot)
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        profile::compile_profile(profile, target_slot, base_blob, macros)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        edit::compile_profile_keep_macros(profile, target_slot, base_blob)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        edit::deactivate_profile(base_blob, slot)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        edit::validate_remap(mode, source, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::ControllerSpec;
    use crate::model::Mode;

    #[test]
    fn pro3_config_ports_cover_every_mode() {
        let modes: Vec<Mode> = Pro3.config_ports().iter().map(|p| p.mode).collect();
        assert_eq!(modes, Pro3.modes());
        assert_eq!(Pro3.write_payload_offset(Mode::XInput), 18);
        assert_eq!(Pro3.write_payload_offset(Mode::DInput), 16);
        assert_eq!(Pro3.blob_size(), 0x092C);
    }

    #[test]
    fn pro3_slot_select_and_macro_mode_values() {
        assert_eq!(Pro3.slot_select_value(Mode::Switch), 0);
        assert_eq!(Pro3.slot_select_value(Mode::DInput), 1);
        assert_eq!(Pro3.slot_select_value(Mode::XInput), 3);
        assert_eq!(Pro3.macro_gamepad_mode(Mode::XInput), 3);
        assert_eq!(Pro3.macro_gamepad_mode(Mode::Switch), 0);
        assert_eq!(Pro3.macro_gamepad_mode(Mode::DInput), 1);
    }
}
