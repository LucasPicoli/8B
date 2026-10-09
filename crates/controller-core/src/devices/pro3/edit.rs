//! Blob edits the write orchestrators need on top of `compile_profile`: compile that
//! keeps the target slot's macros, macro removal, slot deactivation, and remap name checks.
//!
//! `compile_profile` zeroes the target slot's macro descriptors and re-encodes only the
//! ones passed in. Re-encoding what was decoded from the device is lossy (the gamepad
//! mode byte, unknown trigger key maps and the unmodelled descriptor bytes do not survive),
//! so [`compile_profile_keep_macros`] copies the descriptor bytes verbatim instead.

use super::macros::decode_macro_metadata;
use super::profile::{compile_profile, seal_crc};
use super::tables;
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, Mode, Slot};
use crate::protocol::bytes::{put_slice, take};

/// Size of the descriptor region of one Section-4 slot record (4 descriptors of 52 bytes).
const DESCRIPTOR_REGION_SIZE: usize =
    tables::MACRO_SLOTS_PER_PROFILE * tables::MACRO_DESCRIPTOR_SIZE;

/// Size of the active-slot flag at the start of the blob (one per profile slot).
const FLAG_SIZE: usize = 4;

/// Offset of the first macro descriptor of `slot`.
fn descriptor_region_offset(slot: Slot) -> Result<usize> {
    Ok(tables::SECTION4_BASE_OFFSET
        + super::slot_index(slot)? * tables::SECTION4_SLOT_STRIDE
        + tables::SECTION4_RECORD_HEADER_SIZE)
}

/// Compiles `profile` into `target_slot` of `base_blob` and keeps the macro descriptors
/// that `base_blob` already holds for that slot, byte for byte.
///
/// A `base_blob` that is not 2348 bytes has no macros to keep, so none are written.
///
/// # Errors
/// Same as [`compile_profile`].
pub fn compile_profile_keep_macros(
    profile: &CanonicalProfile,
    target_slot: Slot,
    base_blob: &[u8],
) -> Result<Vec<u8>> {
    let mut blob = compile_profile(profile, target_slot, base_blob, &[])?;
    if base_blob.len() == tables::EXPECTED_PROFILE_SIZE {
        let at = descriptor_region_offset(target_slot)?;
        put_slice(&mut blob, at, take(base_blob, at, DESCRIPTOR_REGION_SIZE)?)?;
        seal_crc(&mut blob)?;
    }
    Ok(blob)
}

/// Returns `blob` with every macro of `slot` whose trigger is in `triggers` removed.
///
/// A removed macro's 52-byte descriptor becomes zeros, which the controller reads as an
/// empty macro slot. Its step page stays as it is. The other macros, the other slots and
/// the record header stay as read. A trigger that names no macro is skipped.
///
/// # Errors
/// Returns [`Error::Validation`] if `blob` is not 2348 bytes.
pub fn drop_macros(blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
    if blob.len() != tables::EXPECTED_PROFILE_SIZE {
        return Err(Error::Validation(format!(
            "blob must be {} bytes to remove a macro, got {}",
            tables::EXPECTED_PROFILE_SIZE,
            blob.len()
        )));
    }
    let mut out = blob.to_vec();
    for def in decode_macro_metadata(blob, slot)? {
        if !triggers.contains(&def.trigger) {
            continue;
        }
        let index = usize::from(def.macro_slot.unwrap_or(0));
        let at = descriptor_region_offset(slot)? + index * tables::MACRO_DESCRIPTOR_SIZE;
        put_slice(&mut out, at, &[0; tables::MACRO_DESCRIPTOR_SIZE])?;
    }
    seal_crc(&mut out)?;
    Ok(out)
}

/// Clears the active-slot flag of `slot` in `base_blob` and reseals the CRC.
///
/// Everything else stays as read, macros included.
///
/// # Errors
/// Returns [`Error::Validation`] if `base_blob` is not 2348 bytes.
pub fn deactivate_profile(base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
    if base_blob.len() != tables::EXPECTED_PROFILE_SIZE {
        return Err(Error::Validation(format!(
            "blob must be {} bytes to deactivate a slot, got {}",
            tables::EXPECTED_PROFILE_SIZE,
            base_blob.len()
        )));
    }
    let mut blob = base_blob.to_vec();
    put_slice(&mut blob, super::slot_index(slot)? * tables::FLAG_STRIDE, &[0; FLAG_SIZE])?;
    seal_crc(&mut blob)?;
    Ok(blob)
}

/// Checks a remap request against the Pro 3 description.
///
/// `source` must have `can_be_remapped`. `target` must have `can_be_output`, be
/// `disabled`, or be one of the mode's extra outputs (`screenshot` in Switch). It is
/// never `unrecognised`: that target is only ever read from the pad.
///
/// # Errors
/// Returns [`Error::Validation`] with a message fit to show to the user, or
/// [`Error::Decode`] if the embedded description is malformed.
pub fn validate_remap(mode: Mode, source: &str, target: &str) -> Result<()> {
    super::description()?.validate_remap(mode, source, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::pro3::{DINPUT, SWITCH, XINPUT};

    #[test]
    fn remap_rules() {
        assert!(validate_remap(XINPUT, "l1", "r1").is_ok());
        assert!(validate_remap(XINPUT, "rp", "l1").is_ok(), "paddles are valid sources");
        assert!(validate_remap(XINPUT, "l1", "disabled").is_ok());
        assert!(validate_remap(SWITCH, "turbo", "screenshot").is_ok());
        assert!(validate_remap(XINPUT, "turbo", "screenshot").is_err());
        assert!(validate_remap(DINPUT, "l1", "rp output").is_ok());
        assert!(validate_remap(XINPUT, "l1", "rp output").is_err());
        assert!(validate_remap(SWITCH, "l1", "r4 output").is_err());
        // A button mapped to turbo fires nothing on the real pad; Turbo itself still works.
        assert!(validate_remap(DINPUT, "l4", "turbo").is_err());
        assert!(validate_remap(DINPUT, "turbo", "turbo").is_ok());
        assert!(validate_remap(XINPUT, "l1", "rp").is_err(), "paddles are not targets");
        assert!(validate_remap(XINPUT, "home/guide", "l1").is_err());
        assert!(validate_remap(XINPUT, "nope", "l1").is_err());
        assert!(validate_remap(XINPUT, "l1", "nope").is_err());
    }
}
