//! 8BitDo Ultimate 2 Wireless controller backend.
//!
//! The Ultimate 2 speaks the Pro 3 protocol and keeps the Pro 3 image layout, less the
//! Pro 3's 1176-byte legacy macro section and plus 420 bytes of motion and light
//! settings at the end. The codec widens an Ultimate 2 image to the Pro 3 layout, runs
//! the Pro 3 codec on it, and narrows the result back.

use std::sync::LazyLock;
use std::time::Duration;

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, ProtocolCodec};
use crate::devices::pro3::{self, Pro3};
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::bytes::{put_slice, read_u16_le, take};

/// Ultimate 2 image size in bytes (`0x0638`).
pub const BLOB_SIZE: usize = 0x0638;
/// End of the head both layouts share: flags, names, settings and the button map.
const SHARED_HEAD_END: usize = 0x01F4;
/// Length of the Pro 3's legacy macro section, which the Ultimate 2 lacks.
const LEGACY_LEN: usize = 0x0498;
/// Start of the macro records and motor ranges in a Pro 3 image.
const PRO3_TAIL_START: usize = SHARED_HEAD_END + LEGACY_LEN;
/// End of the macro records and motor ranges in an Ultimate 2 image. The motion and
/// light settings follow, up to [`BLOB_SIZE`].
const TAIL_END: usize = SHARED_HEAD_END + (pro3::tables::EXPECTED_PROFILE_SIZE - PRO3_TAIL_START);

/// The face-button map entries that trade places with the entry after them between
/// the two layouts. A swap is its own inverse, so it serves both directions.
const FACE_SWAPS: [usize; 2] = [0, 2];

/// The vendor app waits this long between the last chunk and the apply, on the
/// Ultimate 2 only.
const APPLY_DELAY: Duration = Duration::from_millis(100);

/// Offset of the macro count in a slot's macro record: the number of macros with
/// steps, a byte. The Pro 3 code writes its macro slot count there instead.
const MACRO_COUNT_OFFSET: usize = 4;

/// The `device` every Ultimate 2 profile carries.
const DEVICE: &str = "8bitdo-ultimate-2";
/// The `kind` every Ultimate 2 profile carries.
const KIND: &str = "8bitdo.ultimate-2.profile";

/// The 8BitDo Ultimate 2 Wireless controller backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ultimate2;

/// The Ultimate 2 controller description, parsed on first use.
static DESCRIPTION: LazyLock<Result<ControllerDescription>> = LazyLock::new(|| {
    ControllerDescription::parse(
        include_str!("../../../controllers/ultimate-2/description.json"),
        &[
            ("front.svg", include_str!("../../../controllers/ultimate-2/front.svg")),
            ("back.svg", include_str!("../../../controllers/ultimate-2/back.svg")),
        ],
    )
});

/// `blob` in the Pro 3 layout. A blob of another size, such as the empty base of a new
/// write, passes through, and the Pro 3 codec treats it as it would its own.
fn widen(blob: &[u8]) -> Vec<u8> {
    if blob.len() != BLOB_SIZE {
        return blob.to_vec();
    }
    let (head, rest) = blob.split_at(SHARED_HEAD_END);
    let tail = rest.get(..TAIL_END - SHARED_HEAD_END).unwrap_or_default();
    let mut wide = [head, &[0; LEGACY_LEN], tail].concat();
    swap_faces(&mut wide);
    wide
}

/// Swaps the [`FACE_SWAPS`] entries of every slot's button map in `blob`. The map
/// lies in the shared head, so the same offsets serve both layouts.
fn swap_faces(blob: &mut [u8]) {
    use pro3::tables::{BUTTON_ENTRY_BYTES, BUTTON_MAP_BASE_OFFSET, BUTTON_MAP_SLOT_STRIDE};
    for slot in 0..usize::from(pro3::tables::PROFILE_SLOT_COUNT) {
        let map = BUTTON_MAP_BASE_OFFSET + slot * BUTTON_MAP_SLOT_STRIDE;
        for index in FACE_SWAPS {
            let start = map + index * BUTTON_ENTRY_BYTES;
            let Some(pair) = blob.get_mut(start..start + 2 * BUTTON_ENTRY_BYTES) else {
                return;
            };
            let (first, second) = pair.split_at_mut(BUTTON_ENTRY_BYTES);
            first.swap_with_slice(second);
        }
    }
}

/// `wide`, a Pro 3 layout image, back in the Ultimate 2 layout, with the motion and
/// light settings of `base` (zero when `base` is not a full image), CRC resealed.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `wide` is not a full Pro 3 layout image.
fn narrow(wide: &[u8], base: &[u8]) -> Result<Vec<u8>> {
    let short = || crate::Error::Decode(format!("expected a {}-byte image", wide.len()));
    let head = wide.get(..SHARED_HEAD_END).ok_or_else(short)?;
    let tail = wide.get(PRO3_TAIL_START..pro3::tables::EXPECTED_PROFILE_SIZE).ok_or_else(short)?;
    let extra = match base.get(TAIL_END..) {
        Some(extra) if base.len() == BLOB_SIZE => extra.to_vec(),
        _ => vec![0; BLOB_SIZE - TAIL_END],
    };
    let mut out = [head, tail, &extra].concat();
    swap_faces(&mut out);
    count_macros(&mut out)?;
    pro3::profile::seal_crc(&mut out)?;
    Ok(out)
}

/// Writes the macro count of each slot whose macro record is in use, the number of
/// its macros with steps, as the vendor app counts it before a write. An erased
/// record keeps its `ff`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `blob` is shorter than its macro records.
fn count_macros(blob: &mut [u8]) -> Result<()> {
    use pro3::tables::{
        MACRO_DESCRIPTOR_SIZE, MACRO_MAX_STEPS_OFFSET, MACRO_SLOTS_PER_PROFILE,
        SECTION4_RECORD_HEADER_SIZE, SECTION4_SLOT_STRIDE,
    };
    for slot in 0..usize::from(pro3::tables::PROFILE_SLOT_COUNT) {
        let record = SHARED_HEAD_END + slot * SECTION4_SLOT_STRIDE;
        if take(blob, record, pro3::tables::SLOT_MARKER.len())? != pro3::tables::SLOT_MARKER {
            continue;
        }
        let mut count = 0u8;
        for i in 0..MACRO_SLOTS_PER_PROFILE {
            let at = record + SECTION4_RECORD_HEADER_SIZE + i * MACRO_DESCRIPTOR_SIZE;
            if read_u16_le(blob, at + MACRO_MAX_STEPS_OFFSET)? > 0 {
                count += 1;
            }
        }
        put_slice(blob, record + MACRO_COUNT_OFFSET, &[count])?;
    }
    Ok(())
}

/// `profile` with this model's `device` and `kind`.
fn own(mut profile: CanonicalProfile) -> CanonicalProfile {
    DEVICE.clone_into(&mut profile.device);
    KIND.clone_into(&mut profile.kind);
    profile
}

/// `mode`, if the Ultimate 2 has it. The Pro 3 bytes serve only the modes both share.
///
/// # Errors
/// Returns [`crate::Error::Validation`] for a mode the Ultimate 2 does not have.
fn has(mode: Mode) -> Result<Mode> {
    match Ultimate2.description()?.mode(mode) {
        Some(_) => Ok(mode),
        None => Err(crate::Error::Validation(format!("the Ultimate 2 has no {mode} mode"))),
    }
}

impl ControllerSpec for Ultimate2 {
    fn description(&self) -> Result<&'static ControllerDescription> {
        DESCRIPTION.as_ref().map_err(Clone::clone)
    }
    fn blob_size(&self) -> usize {
        BLOB_SIZE
    }
    fn apply_argument(&self) -> u16 {
        Pro3.apply_argument()
    }
    fn apply_delay(&self) -> Duration {
        APPLY_DELAY
    }
    fn slot_select_value(&self, mode: Mode) -> Result<u8> {
        Pro3.slot_select_value(has(mode)?)
    }
    fn macro_gamepad_mode(&self, mode: Mode) -> Result<u8> {
        Pro3.macro_gamepad_mode(has(mode)?)
    }
}

impl ProtocolCodec for Ultimate2 {
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        let wide = RawProfilePayload { payload: widen(&raw.payload), ..raw.clone() };
        let mut summary = Pro3.map_profile(&wide)?;
        summary.canonical = own(summary.canonical);
        Ok(summary)
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        own(Pro3.default_profile(mode))
    }

    fn decode_macro_metadata(
        &self,
        blob: &[u8],
        profile_slot: Slot,
    ) -> Result<Vec<MacroDefinition>> {
        Pro3.decode_macro_metadata(&widen(blob), profile_slot)
    }

    fn decode_macro_steps(
        &self,
        stream: &[u8],
        step_count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>> {
        Pro3.decode_macro_steps(stream, step_count, mode)
    }

    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
        Pro3.encode_macro_steps(steps, mode)
    }

    fn encode_macro_metadata(
        &self,
        def: &MacroDefinition,
        macro_slot: MacroSlot,
    ) -> Result<Vec<u8>> {
        Pro3.encode_macro_metadata(def, macro_slot)
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        let wide = Pro3.compile_profile(profile, target_slot, &widen(base_blob), macros)?;
        narrow(&wide, base_blob)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        let wide = Pro3.compile_profile_keep_macros(profile, target_slot, &widen(base_blob))?;
        narrow(&wide, base_blob)
    }

    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        narrow(&Pro3.drop_macros(&widen(blob), slot, triggers)?, blob)
    }

    fn set_macro(
        &self,
        blob: &[u8],
        slot: Slot,
        macro_slot: MacroSlot,
        def: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        narrow(&Pro3.set_macro(&widen(blob), slot, macro_slot, def)?, blob)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        narrow(&Pro3.deactivate_profile(&widen(base_blob), slot)?, base_blob)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        self.description()?.validate_remap(mode, source, target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::devices::pro3::XINPUT;

    /// Bank read of a real Ultimate 2 in `XInput`: slot 1 "Crimson" with two macros.
    const XINPUT_BLOB: &[u8] = include_bytes!("../../../../../fixtures/ultimate-2/xinput.blob");
    /// Where the CRC sits. The pad does not check it, and its own differs from ours.
    const CRC: std::ops::Range<usize> = 0x0C..0x10;

    /// `blob` with its CRC zeroed.
    fn sans_crc(blob: &[u8]) -> Vec<u8> {
        let mut out = blob.to_vec();
        out[CRC].fill(0);
        out
    }

    fn slot1() -> CanonicalProfile {
        let raw = RawProfilePayload {
            payload: XINPUT_BLOB.to_vec(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: XINPUT,
        };
        Ultimate2.map_profile(&raw).unwrap().canonical
    }

    #[test]
    fn layout_sizes_add_up() {
        assert_eq!(PRO3_TAIL_START, 0x068C);
        assert_eq!(TAIL_END, 0x0494);
        assert_eq!(XINPUT_BLOB.len(), BLOB_SIZE);
        assert_eq!(widen(XINPUT_BLOB).len(), pro3::tables::EXPECTED_PROFILE_SIZE);
    }

    #[test]
    fn narrow_undoes_widen() {
        let back = narrow(&widen(XINPUT_BLOB), XINPUT_BLOB).unwrap();
        assert_eq!(sans_crc(&back), sans_crc(XINPUT_BLOB));
    }

    #[test]
    fn reads_the_captured_slot() {
        let p = slot1();
        assert_eq!(
            (p.name.as_str(), p.device.as_str(), p.kind.as_str()),
            ("Crimson", DEVICE, KIND)
        );
        for face in ["right face", "bottom face", "top face", "left face"] {
            let m = p.button_mappings.iter().find(|m| m.source == face).unwrap();
            assert_eq!(m.target, face, "the capture remaps no face button");
        }
        let macros = Ultimate2.decode_macro_metadata(XINPUT_BLOB, Slot::new(1).unwrap()).unwrap();
        let found: Vec<_> =
            macros.iter().map(|m| (m.name.as_str(), m.trigger_buttons.clone())).collect();
        assert_eq!(found, [("L1L2", vec!["l4".to_owned()]), ("R1R2", vec!["r4".to_owned()])]);
    }

    #[test]
    fn writing_the_read_profile_back_changes_no_byte() {
        let out = Ultimate2
            .compile_profile_keep_macros(&slot1(), Slot::new(1).unwrap(), XINPUT_BLOB)
            .unwrap();
        let diff: Vec<usize> =
            (0..BLOB_SIZE).filter(|&i| out[i] != XINPUT_BLOB[i] && !CRC.contains(&i)).collect();
        assert!(diff.is_empty(), "bytes differ at {diff:x?}");
    }
}
