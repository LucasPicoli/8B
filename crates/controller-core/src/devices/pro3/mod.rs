//! 8BitDo Pro 3 controller backend.

pub mod edit;
pub mod macros;
pub mod profile;
pub mod tables;

use crate::device::{ControllerSpec, ProtocolCodec, TransportParams, UsbId};
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};

/// The 8BitDo Pro 3 controller backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pro3;

const USB_IDS: [UsbId; 2] =
    [UsbId { vendor: 0x2DC8, product: 0x310B }, UsbId { vendor: 0x2DC8, product: 0x6009 }];
const MODES: [Mode; 3] = [Mode::XInput, Mode::Switch, Mode::DInput];

impl ControllerSpec for Pro3 {
    fn usb_ids(&self) -> &[UsbId] {
        &USB_IDS
    }
    fn modes(&self) -> &[Mode] {
        &MODES
    }
    fn transport_params(&self, mode: Mode) -> TransportParams {
        match mode {
            Mode::XInput | Mode::Switch => {
                TransportParams { interface: 2, ep_out: 0x03, ep_in: 0x83, payload_offset: 18 }
            }
            Mode::DInput => {
                TransportParams { interface: 0, ep_out: 0x02, ep_in: 0x81, payload_offset: 16 }
            }
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
    fn product_id_for_mode(&self, mode: Mode) -> u16 {
        match mode {
            Mode::XInput | Mode::Switch => 0x310B,
            Mode::DInput => 0x6009,
        }
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
    fn pro3_transport_params_match_spec() {
        let p = Pro3.transport_params(Mode::XInput);
        assert_eq!((p.interface, p.ep_out, p.ep_in, p.payload_offset), (2, 0x03, 0x83, 18));
        let d = Pro3.transport_params(Mode::DInput);
        assert_eq!((d.interface, d.ep_out, d.ep_in, d.payload_offset), (0, 0x02, 0x81, 16));
        assert_eq!(Pro3.blob_size(), 0x092C);
        assert_eq!(Pro3.product_id_for_mode(Mode::DInput), 0x6009);
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
