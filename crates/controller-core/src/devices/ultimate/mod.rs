//! 8BitDo Ultimate controller backend: the variant with Nintendo labels and a 2.4G and
//! Bluetooth switch, over its USB cable.
//!
//! The pad keeps one 1652-byte image per mode, `XInput` and Switch. Each image holds
//! three profile slots with their macros; [`tables`] lays the bytes out. Its config
//! port `2dc8:6007` names it, since no reply carries a model id, and shows only after
//! the unlock its description declares.

pub mod edit;
pub mod macros;
pub mod profile;
pub mod settings;
pub mod tables;

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, ProtocolCodec, RawRequest};
use crate::error::{Error, Result};
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::bytes::{read_u16_le, read_u32_le};
use crate::protocol::wire_write::PACKET_LEN;

use tables::{BLOB_SIZE, FLAG_LEN, GAMEPAD_MODE_OFFSET, IN_USE, SLOT_COUNT, SLOT_FLAG_OFFSET};

/// The Ultimate's `XInput` mode.
pub const XINPUT: Mode = Mode::from_static("xinput");
/// The Ultimate's Nintendo Switch mode.
pub const SWITCH: Mode = Mode::from_static("switch");
/// The Ultimate's modes.
pub const MODES: [Mode; 2] = [XINPUT, SWITCH];

/// The firmware request, `81 05 00 21 01`, as the pad takes it in a USB capture.
const FIRMWARE_REQUEST: [u8; 5] = [0x81, 0x05, 0x00, 0x21, 0x01];
/// The head of the firmware reply, `02 22`.
const FIRMWARE_REPLY: [u8; 2] = [0x02, 0x22];
/// Where the firmware reply holds the version times 100, little-endian.
const FIRMWARE_VERSION_AT: usize = 2;
/// The argument of the apply command.
const APPLY_ARGUMENT: u16 = 0x0015;

/// The image-mode value of `mode`, also the value that selects its image.
///
/// # Errors
/// Returns [`Error::Validation`] for a mode the Ultimate does not have.
pub fn mode_byte(description: &ControllerDescription, mode: Mode) -> Result<u8> {
    description
        .mode(mode)
        .and_then(|m| m.gamepad_mode)
        .ok_or_else(|| Error::Validation(format!("the Ultimate has no {mode} mode")))
}

/// The mode of the image `blob`, from the mode it records.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is too short or records another mode. A mode byte 0 on a
/// model with no mode of byte 0 gets a message that names the modes it does list.
pub fn image_mode(description: &ControllerDescription, blob: &[u8]) -> Result<Mode> {
    let value = read_u16_le(blob, GAMEPAD_MODE_OFFSET)?;
    description
        .modes
        .iter()
        .find(|m| m.gamepad_mode.is_some_and(|b| u16::from(b) == value))
        .map(|m| m.id)
        .ok_or_else(|| {
            if value != 0 {
                return Error::Decode(format!("the image records an unknown mode {value}"));
            }
            // Byte 0 is Switch mode, and this model lists no Switch mode.
            let listed: Vec<&str> = description.modes.iter().map(|m| m.label.as_str()).collect();
            Error::Decode(format!(
                "the pad is in a mode 8B does not support: switch it to {}",
                listed.join(" or ")
            ))
        })
}

/// Refuses a blob that is not a full image.
fn check_blob(blob: &[u8]) -> Result<()> {
    if blob.len() == BLOB_SIZE {
        Ok(())
    } else {
        Err(Error::Decode(format!("an Ultimate image is {BLOB_SIZE} bytes, not {}", blob.len())))
    }
}

/// The 0-based index of `slot`.
///
/// # Errors
/// Returns [`Error::Validation`] for a slot past the third.
fn slot_index(slot: Slot) -> Result<usize> {
    slot.check(SLOT_COUNT).map(|s| usize::from(s.get()) - 1)
}

/// The flag at `at`.
fn read_flag(blob: &[u8], at: usize) -> Result<u32> {
    read_u32_le(blob, at)
}

/// Whether `slot` of `blob` holds a profile: its flag is exactly the in-use value.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is too short, and [`Error::Validation`] for a
/// slot past the third.
pub fn slot_in_use(blob: &[u8], slot: Slot) -> Result<bool> {
    Ok(read_flag(blob, SLOT_FLAG_OFFSET + slot_index(slot)? * FLAG_LEN)? == IN_USE)
}

/// The refusal of a step-page hook: the Ultimate keeps each macro's steps in its record.
fn steps_in_blob() -> Error {
    Error::Validation("the Ultimate keeps macro steps inside the profile image".to_owned())
}

/// The description of one Ultimate model, parsed on first use.
pub type DescriptionFn = fn() -> Result<&'static ControllerDescription>;

/// The 8BitDo Ultimate controller backend, serving one model.
///
/// It carries the description of the model's folder, and every code, index and default
/// the codec uses comes from it. Build one with [`model!`](crate::devices::ultimate::model).
#[derive(Debug, Clone, Copy)]
pub struct Ultimate {
    /// The description of the model this value serves.
    description: DescriptionFn,
}

impl Ultimate {
    /// An Ultimate that serves the model `description` returns.
    #[must_use]
    pub const fn new(description: DescriptionFn) -> Self {
        Self { description }
    }
}

/// An [`Ultimate`] for the model in `controllers/<folder>/`, which holds `description.json`,
/// `front.svg` and `back.svg`. A model is one folder and one line in the registry:
/// `&ultimate::model!("folder")`.
#[macro_export]
macro_rules! ultimate_model {
    ($folder:literal) => {
        $crate::devices::ultimate::Ultimate::new({
            fn description() -> $crate::Result<&'static $crate::description::ControllerDescription>
            {
                static DESCRIPTION: ::std::sync::LazyLock<
                    $crate::Result<$crate::description::ControllerDescription>,
                > = ::std::sync::LazyLock::new(|| {
                    macro_rules! file {
                        ($name:literal) => {
                            include_str!(concat!(
                                env!("CARGO_MANIFEST_DIR"),
                                "/controllers/",
                                $folder,
                                "/",
                                $name
                            ))
                        };
                    }
                    $crate::description::ControllerDescription::parse(
                        file!("description.json"),
                        &[("front.svg", file!("front.svg")), ("back.svg", file!("back.svg"))],
                    )
                });
                DESCRIPTION.as_ref().map_err(::std::clone::Clone::clone)
            }
            description
        })
    };
}
pub use crate::ultimate_model as model;

impl ControllerSpec for Ultimate {
    fn description(&self) -> Result<&'static ControllerDescription> {
        (self.description)()
    }
    fn blob_size(&self) -> usize {
        BLOB_SIZE
    }
    fn apply_argument(&self) -> u16 {
        APPLY_ARGUMENT
    }
    fn firmware_request(&self) -> Option<RawRequest> {
        let mut wire = [0u8; PACKET_LEN];
        for (dst, b) in wire.iter_mut().zip(FIRMWARE_REQUEST) {
            *dst = b;
        }
        Some(RawRequest { wire, reply_head: FIRMWARE_REPLY })
    }
    /// The version times 100 sits at reply bytes 2 and 3: `6d 00` is 1.09.
    fn firmware_version(&self, reply: &[u8]) -> Result<String> {
        let hundredths = read_u16_le(reply, FIRMWARE_VERSION_AT)?;
        Ok(format!("{}.{:02}", hundredths / 100, hundredths % 100))
    }
    /// The pad acknowledges a chunk with a wrong CRC and stores it, so only the
    /// read-back finds a corrupted chunk.
    fn read_back_writes(&self) -> bool {
        true
    }
    fn slot_select_value(&self, mode: Mode) -> Result<u8> {
        mode_byte((self.description)()?, mode)
    }
}

impl ProtocolCodec for Ultimate {
    fn slot_active(&self, blob: &[u8], slot: Slot) -> Result<bool> {
        slot_in_use(blob, slot)
    }

    /// Refuses an image whose recorded mode is not the one asked for.
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        let asked = raw.mode_hint;
        let d = (self.description)()?;
        if image_mode(d, &raw.payload)? != asked {
            return Err(Error::Decode(format!("the image does not match the {asked} mode")));
        }
        profile::map_profile(d, raw)
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        // An unreadable description is a build defect; no model reaches the blank profile.
        (self.description)().map_or_else(
            |_| profile::blank_profile(mode, Vec::new(), 0),
            |d| profile::default_profile(d, mode),
        )
    }

    fn macro_steps_in_blob(&self) -> bool {
        true
    }

    fn decode_macro_metadata(
        &self,
        blob: &[u8],
        profile_slot: Slot,
    ) -> Result<Vec<MacroDefinition>> {
        macros::decode_macros((self.description)()?, blob, profile_slot)
    }

    fn decode_macro_steps(&self, _: &[u8], _: usize, _: Mode) -> Result<Vec<MacroStep>> {
        Err(steps_in_blob())
    }

    fn encode_macro_steps(&self, _: &[MacroStep], _: Mode) -> Result<Vec<u8>> {
        Err(steps_in_blob())
    }

    /// The macro's 96-byte record, steps included.
    fn encode_macro_metadata(&self, def: &MacroDefinition, _: MacroSlot) -> Result<Vec<u8>> {
        macros::encode_record((self.description)()?, def).map(|record| record.to_vec())
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        edit::compile_profile((self.description)()?, profile, target_slot, base_blob, macros)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        edit::compile_profile_keep_macros((self.description)()?, profile, target_slot, base_blob)
    }

    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        edit::drop_macros((self.description)()?, blob, slot, triggers)
    }

    fn set_macro(
        &self,
        blob: &[u8],
        slot: Slot,
        macro_slot: MacroSlot,
        def: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        edit::set_macro((self.description)()?, blob, slot, macro_slot, def)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        edit::deactivate_profile(base_blob, slot)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        (self.description)()?.validate_remap(mode, source, target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    /// The registered Ultimate.
    fn ultimate() -> &'static dyn crate::device::Model {
        crate::devices::model_named("Ultimate").unwrap()
    }
    use super::*;

    #[test]
    fn buttons_are_printed_as_on_the_switch_layout_in_every_mode() {
        let d = ultimate().description().unwrap();
        let name = |id: &str, mode| d.button(id).unwrap().printed(mode).to_owned();
        for mode in [Mode::from_static("xinput"), Mode::from_static("switch")] {
            let printed: Vec<_> = ["right face", "bottom face", "top face", "left face"]
                .map(|id| name(id, mode))
                .into();
            assert_eq!(printed, ["A", "B", "X", "Y"]);
            let printed: Vec<_> = ["l1", "r1", "l2", "r2", "select/back", "start/menu"]
                .map(|id| name(id, mode))
                .into();
            assert_eq!(printed, ["L", "R", "ZL", "ZR", "−", "+"]);
            let minus = d.button("select/back").unwrap();
            assert_eq!(minus.printed_spoken(mode), "Minus", "a print reads as words");
            assert_eq!(name("l3", mode), d.button("l3").unwrap().labels[&mode], "no print set");
        }
        let right = d.button("right face").unwrap();
        assert_eq!(
            right.labels[&Mode::from_static("xinput")],
            "B",
            "the output keeps its XInput name"
        );
        assert_eq!(right.printed_spoken(Mode::from_static("xinput")), "A");
    }

    #[test]
    fn ultimate_hotspots_sit_on_their_buttons() {
        let views = &ultimate().description().unwrap().views;
        let hit = |view: usize, x: f64, y: f64| -> Vec<&str> {
            let hotspots = views[view].hotspots.iter();
            hotspots.filter(|h| h.contains(x, y)).map(|h| h.button.as_str()).collect()
        };
        assert_eq!(hit(0, 385.0, 147.0), ["bottom face"]);
        assert_eq!(hit(0, 118.0, 112.0), ["l3"]);
        assert_eq!(hit(0, 185.0, 150.0), ["d-pad up"]);
        assert_eq!(hit(0, 185.0, 175.0), Vec::<&str>::new(), "d-pad centre");
        assert_eq!(hit(0, 250.0, 76.0), ["home/guide"]);
        assert_eq!(hit(0, 250.0, 110.0), ["star"], "below Home");
        assert_eq!(hit(0, 216.0, 110.0), ["select/back"]);
        assert_eq!(hit(0, 284.0, 110.0), ["start/menu"]);
        assert_eq!(hit(0, 100.0, 36.0), ["l1"], "bumper on the front");
        assert_eq!(hit(0, 400.0, 36.0), ["r1"]);
        assert_eq!(hit(0, 60.0, 120.0), Vec::<&str>::new(), "body beside the left stick");
        assert_eq!(hit(0, 60.0, 70.0), Vec::<&str>::new(), "body below the bumper");
        assert_eq!(hit(1, 117.0, 50.0), ["r2"]);
        assert_eq!(hit(1, 135.0, 165.0), ["p1"], "under the right hand, on the viewer's left");
        assert_eq!(hit(1, 365.0, 165.0), ["p2"]);
        assert_eq!(hit(1, 250.0, 100.0), Vec::<&str>::new(), "body between the triggers");
    }
}
