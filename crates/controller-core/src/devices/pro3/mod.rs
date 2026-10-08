//! 8BitDo Pro 3 controller backend.

pub mod edit;
pub mod macros;
pub mod profile;
pub mod settings;
pub mod tables;

use std::sync::LazyLock;

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, ProtocolCodec};
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::wire_write::PACKET_LEN;
use crate::service::validation::{profile_validator, schema_errors, ValidationError};

/// Command bytes of the mode flip (`81 00 51 <target>`). Seen in the vendor app.
const MODE_FLIP: [u8; 3] = [0x81, 0x00, 0x51];
/// Flip target byte for `DInput`.
const FLIP_TO_DINPUT: u8 = 0x01;
/// Flip target byte for `XInput`.
const FLIP_TO_XINPUT: u8 = 0x02;
/// The close command (`81 05 07`): back to the slide-switch mode.
const MODE_CLOSE: [u8; 3] = [0x81, 0x05, 0x07];

/// A zero-padded normal-layout packet that starts with `head`.
fn packet(head: &[u8]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    for (dst, b) in p.iter_mut().zip(head) {
        *dst = *b;
    }
    p
}

/// The 8BitDo Pro 3 controller backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Pro3;

/// The Pro 3 controller description, parsed on first use.
static DESCRIPTION: LazyLock<Result<ControllerDescription>> = LazyLock::new(|| {
    ControllerDescription::parse(
        include_str!("../../../controllers/pro3/description.json"),
        &[
            ("front.svg", include_str!("../../../controllers/pro3/front.svg")),
            ("back.svg", include_str!("../../../controllers/pro3/back.svg")),
        ],
    )
});

/// Returns the Pro 3 controller description.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if the embedded file is malformed.
pub(crate) fn description() -> Result<&'static ControllerDescription> {
    DESCRIPTION.as_ref().map_err(Clone::clone)
}

impl ControllerSpec for Pro3 {
    fn description(&self) -> Result<&'static ControllerDescription> {
        description()
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
    fn mode_flip_command(&self, target: Mode) -> Option<[u8; PACKET_LEN]> {
        let target_byte = match target {
            Mode::DInput => FLIP_TO_DINPUT,
            Mode::XInput => FLIP_TO_XINPUT,
            Mode::Switch => return None,
        };
        let mut p = packet(&MODE_FLIP);
        if let Some(b) = p.get_mut(MODE_FLIP.len()) {
            *b = target_byte;
        }
        Some(p)
    }
    fn mode_close_command(&self) -> [u8; PACKET_LEN] {
        packet(&MODE_CLOSE)
    }
}

impl ProtocolCodec for Pro3 {
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        profile::map_profile(self, raw)
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        profile::default_profile(mode)
    }

    fn profile_schema_errors(
        &self,
        profile_json: &serde_json::Value,
    ) -> Result<Vec<ValidationError>> {
        Ok(schema_errors(profile_validator()?, profile_json))
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

    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        edit::drop_macros(blob, slot, triggers)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        edit::deactivate_profile(base_blob, slot)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        edit::validate_remap(mode, source, target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::ControllerSpec;
    use crate::model::Mode;
    use crate::protocol::framing::Framing;
    use serde_json::Value;

    #[test]
    fn pro3_config_ports_cover_every_mode() {
        let d = Pro3.description().unwrap();
        let ports: Vec<Mode> = d.config_ports.iter().map(|p| p.mode).collect();
        let modes: Vec<Mode> = d.modes.iter().map(|m| m.id).collect();
        assert_eq!(ports, modes);
        assert_eq!(modes, [Mode::XInput, Mode::Switch, Mode::DInput]);
        assert_eq!(d.config_ports[1].framing, Framing::Wrapped);
        assert_eq!(d.model_ids, [0x6009, 0x600A]);
        assert_eq!((d.slot_count, d.macro_slot_count), (3, 4));
        assert_eq!(Pro3.blob_size(), 0x092C);
        let via: Vec<Option<Mode>> = d.config_ports.iter().map(|p| p.write_via).collect();
        assert_eq!(via, [None, Some(Mode::DInput), None]);
    }

    #[test]
    fn pro3_flip_and_close_bytes() {
        let flip = Pro3.mode_flip_command(Mode::DInput).unwrap();
        assert_eq!(flip[..5], [0x81, 0x00, 0x51, 0x01, 0x00]);
        assert_eq!(Pro3.mode_flip_command(Mode::XInput).unwrap()[3], 0x02);
        assert!(Pro3.mode_flip_command(Mode::Switch).is_none());
        // Wrapped on the Switch id, as sent in the hardware run.
        assert_eq!(Framing::Wrapped.request(&flip)[..6], [0x01, 0x66, 0xAA, 0x00, 0x51, 0x01]);
        assert_eq!(Pro3.mode_close_command()[..4], [0x81, 0x05, 0x07, 0x00]);
    }

    #[test]
    fn pro3_description_buttons_match_the_codec_table() {
        let ids: Vec<&str> =
            Pro3.description().unwrap().buttons.iter().map(|b| b.id.as_str()).collect();
        let table: Vec<&str> = tables::XINPUT_ENCODINGS.iter().map(|e| e.source).collect();
        assert_eq!(ids, table);
    }

    #[test]
    fn pro3_hotspots_sit_on_their_buttons() {
        let views = &Pro3.description().unwrap().views;
        let hit = |view: usize, x: f64, y: f64| -> Vec<&str> {
            let hotspots = views[view].hotspots.iter();
            hotspots.filter(|h| h.contains(x, y)).map(|h| h.button.as_str()).collect()
        };
        assert_eq!(hit(0, 382.0, 146.0), ["bottom face"]);
        assert_eq!(hit(0, 118.0, 88.0), ["d-pad up"]);
        assert_eq!(hit(0, 118.0, 114.0), Vec::<&str>::new(), "d-pad centre");
        assert_eq!(hit(0, 195.0, 40.0), ["l4"]);
        assert_eq!(hit(0, 60.0, 70.0), Vec::<&str>::new(), "body below L1");
        assert_eq!(hit(1, 179.0, 24.0), ["r4"], "seen from behind");
        assert_eq!(hit(1, 117.0, 50.0), ["r2"]);
        assert_eq!(hit(1, 365.0, 165.0), ["lp"]);
    }

    #[test]
    fn pro3_description_matches_its_schema() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../../schemas/controller-description-v1.schema.json"
        ))
        .unwrap();
        let file: Value =
            serde_json::from_str(include_str!("../../../controllers/pro3/description.json"))
                .unwrap();
        let validator = jsonschema::draft202012::new(&schema).unwrap();
        let errors: Vec<String> = validator.iter_errors(&file).map(|e| e.to_string()).collect();
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// The profile and macro schemas stay the validation oracle; the limits must agree.
    #[test]
    fn pro3_limits_equal_the_profile_and_macro_schemas() {
        let profile: Value =
            serde_json::from_str(include_str!("../../../../../schemas/profile-v1.schema.json"))
                .unwrap();
        let macro_: Value =
            serde_json::from_str(include_str!("../../../../../schemas/macro-v1.schema.json"))
                .unwrap();
        let range = |schema: &Value, at: &str, lo: &str, hi: &str| {
            let n = |k: &str| {
                i32::try_from(schema.pointer(&format!("{at}/{k}")).unwrap().as_i64().unwrap())
                    .unwrap()
            };
            (n(lo), n(hi))
        };
        let pair = |r: crate::description::LimitRange| (r.min, r.max);
        let l = Pro3.description().unwrap().limits;
        let props = "/properties";
        let defs = "/$defs";
        assert_eq!(
            pair(l.profile_name_length),
            range(&profile, &format!("{props}/name"), "minLength", "maxLength")
        );
        assert_eq!(
            pair(l.macro_name_length),
            range(&macro_, &format!("{props}/name"), "minLength", "maxLength")
        );
        assert_eq!(
            pair(l.macro_steps),
            range(&macro_, &format!("{props}/steps"), "minItems", "maxItems")
        );
        let d = Pro3.description().unwrap();
        let field = |mode, pointer: &str| pair(d.number(mode, pointer).unwrap().range());
        for side in ["left", "right"] {
            let sticks = format!("{defs}/Sticks/properties/{side}");
            for mode in Mode::ALL {
                let min = field(mode, &format!("/sticks/{side}_min_pct"));
                assert_eq!(
                    min,
                    range(&profile, &format!("{sticks}_min_pct"), "minimum", "maximum")
                );
                let max = field(mode, &format!("/sticks/{side}_max_pct"));
                assert_eq!(
                    max,
                    range(&profile, &format!("{sticks}_max_pct"), "minimum", "maximum")
                );
                let at = format!("{defs}/Vibration/properties/{side}_level");
                let level = field(mode, &format!("/vibration/{side}_level"));
                assert_eq!(level, range(&profile, &at, "minimum", "maximum"));
            }
            for end in ["min", "max"] {
                let at = format!("{defs}/TriggersAnalog/properties/{side}_{end}_pct");
                let pct = field(Mode::XInput, &format!("/triggers/{side}_{end}_pct"));
                assert_eq!(pct, range(&profile, &at, "minimum", "maximum"));
            }
            let at = format!("{defs}/TriggersSwitch/properties/{side}_threshold_pct");
            let point = field(Mode::Switch, &format!("/triggers/{side}_threshold_pct"));
            assert_eq!(point, range(&profile, &at, "minimum", "maximum"));
        }
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
