//! Blob edits the write orchestrators need on top of `compile_profile`: compile that
//! keeps the target slot's macros, slot deactivation, and remap name checks.
//!
//! `compile_profile` zeroes the target slot's macro descriptors and re-encodes only the
//! ones passed in. Re-encoding what was decoded from the device is lossy (the gamepad
//! mode byte, unknown trigger key maps and the unmodelled descriptor bytes do not survive),
//! so [`compile_profile_keep_macros`] copies the descriptor bytes verbatim instead.

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
fn descriptor_region_offset(slot: Slot) -> usize {
    tables::SECTION4_BASE_OFFSET
        + usize::from(slot.get() - 1) * tables::SECTION4_SLOT_STRIDE
        + tables::SECTION4_RECORD_HEADER_SIZE
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
        let at = descriptor_region_offset(target_slot);
        put_slice(&mut blob, at, take(base_blob, at, DESCRIPTOR_REGION_SIZE)?)?;
        seal_crc(&mut blob)?;
    }
    Ok(blob)
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
    put_slice(&mut blob, usize::from(slot.get() - 1) * tables::FLAG_STRIDE, &[0; FLAG_SIZE])?;
    seal_crc(&mut blob)?;
    Ok(blob)
}

/// Checks a remap request: `source` must be a physical control other than `home/guide`,
/// `target` a control other than the back paddles, `disabled`, or `screenshot` (Switch only).
///
/// # Errors
/// Returns [`Error::Validation`] with a message fit to show to the user.
pub fn validate_remap(mode: Mode, source: &str, target: &str) -> Result<()> {
    let index_of = |name: &str| tables::XINPUT_ENCODINGS.iter().position(|e| e.source == name);

    let Some(source_index) = index_of(source) else {
        let names: Vec<&str> = tables::XINPUT_ENCODINGS.iter().map(|e| e.source).collect();
        return Err(Error::Validation(format!(
            "Invalid source control '{source}'. Valid names: {}.",
            names.join(", ")
        )));
    };
    if source_index == tables::HOME_GUIDE_INDEX {
        return Err(Error::Validation(
            "Cannot remap 'home/guide': that button cannot be remapped.".to_owned(),
        ));
    }

    match target {
        "disabled" => Ok(()),
        "screenshot" if mode == Mode::Switch => Ok(()),
        "screenshot" => {
            Err(Error::Validation("Target 'screenshot' is only valid for switch mode.".to_owned()))
        }
        _ if index_of(target).is_some_and(|i| i < tables::NULL_DEFAULT_FIRST_INDEX) => Ok(()),
        _ => Err(Error::Validation(format!(
            "Invalid remap target '{target}'. Valid targets: any button except rp, lp, l4 and r4, or 'disabled'."
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_rules() {
        assert!(validate_remap(Mode::XInput, "l1", "r1").is_ok());
        assert!(validate_remap(Mode::XInput, "rp", "l1").is_ok(), "paddles are valid sources");
        assert!(validate_remap(Mode::XInput, "l1", "disabled").is_ok());
        assert!(validate_remap(Mode::Switch, "turbo", "screenshot").is_ok());
        assert!(validate_remap(Mode::XInput, "turbo", "screenshot").is_err());
        assert!(validate_remap(Mode::XInput, "l1", "rp").is_err(), "paddles are not targets");
        assert!(validate_remap(Mode::XInput, "home/guide", "l1").is_err());
        assert!(validate_remap(Mode::XInput, "nope", "l1").is_err());
        assert!(validate_remap(Mode::XInput, "l1", "nope").is_err());
    }
}
