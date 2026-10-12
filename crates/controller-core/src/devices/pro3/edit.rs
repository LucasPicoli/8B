//! Blob edits the write orchestrators need on top of `compile_profile`: compile that
//! keeps the target slot's macros, macro removal, slot deactivation, and remap name checks.
//!
//! `compile_profile` zeroes the target slot's macro descriptors and re-encodes only the
//! ones passed in. Re-encoding what was decoded from the device is lossy (the gamepad
//! mode byte, unknown trigger key maps and the unmodelled descriptor bytes do not survive),
//! so [`compile_profile_keep_macros`] copies the descriptor bytes verbatim instead.

use super::macros::{encode_macro_metadata, trigger_key_map};
use super::profile::{compile_profile, seal_crc};
use super::tables;
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, MacroDefinition, MacroSlot, Mode, Slot};
use crate::protocol::bytes::{put_slice, put_u32_le, read_u32_le, take};

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

/// Returns `blob` with each button of `buttons` taken off the macros of `slot` it starts.
///
/// Only the `key_map` field of a descriptor changes. A macro that loses its last button
/// stays, with no button; its descriptor is never zeroed. The other bytes, the other
/// slots and the record header stay as read, and the CRC is resealed. A name that is no
/// macro trigger button is skipped.
///
/// # Errors
/// Returns [`Error::Validation`] if `blob` is not 2348 bytes.
pub fn drop_macros(blob: &[u8], slot: Slot, buttons: &[String]) -> Result<Vec<u8>> {
    if blob.len() != tables::EXPECTED_PROFILE_SIZE {
        return Err(Error::Validation(format!(
            "blob must be {} bytes to remove a macro, got {}",
            tables::EXPECTED_PROFILE_SIZE,
            blob.len()
        )));
    }
    let known: Vec<String> =
        buttons.iter().filter(|b| tables::TRIGGERS.iter().any(|t| t.name == *b)).cloned().collect();
    let dropped = trigger_key_map(&known)?;
    let mut out = blob.to_vec();
    for index in 0..tables::MACRO_SLOTS_PER_PROFILE {
        let at = descriptor_region_offset(slot)?
            + index * tables::MACRO_DESCRIPTOR_SIZE
            + tables::MACRO_KEY_MAP_OFFSET;
        let key_map = read_u32_le(&out, at)?;
        put_u32_le(&mut out, at, key_map & !dropped)?;
    }
    seal_crc(&mut out)?;
    Ok(out)
}

/// Returns `blob` with macro slot `macro_slot` of `slot` holding `def`, or zeroed when
/// `def` is `None`. The other three descriptors and the rest of the blob stay as read,
/// and the CRC is resealed.
///
/// # Errors
/// Returns [`Error::Validation`] if `blob` is not 2348 bytes or `def` cannot be encoded.
pub fn set_macro(
    blob: &[u8],
    slot: Slot,
    macro_slot: MacroSlot,
    def: Option<&MacroDefinition>,
) -> Result<Vec<u8>> {
    if blob.len() != tables::EXPECTED_PROFILE_SIZE {
        return Err(Error::Validation(format!(
            "blob must be {} bytes to set a macro, got {}",
            tables::EXPECTED_PROFILE_SIZE,
            blob.len()
        )));
    }
    let descriptor = match def {
        Some(def) => encode_macro_metadata(def, macro_slot)?,
        None => vec![0; tables::MACRO_DESCRIPTOR_SIZE],
    };
    let at = descriptor_region_offset(slot)?
        + usize::from(macro_slot.get()) * tables::MACRO_DESCRIPTOR_SIZE;
    let mut out = blob.to_vec();
    put_slice(&mut out, at, &descriptor)?;
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::super::macros::decode_macro_metadata;
    use super::*;
    use crate::devices::pro3::{DINPUT, SWITCH, XINPUT};
    use crate::model::MacroStep;

    /// Where the CRC sits in a blob.
    const CRC: std::ops::Range<usize> = 0x0C..0x10;

    fn macro_def(name: &str, trigger: &str, steps: usize) -> MacroDefinition {
        MacroDefinition {
            name: name.into(),
            mode: XINPUT,
            trigger_buttons: trigger.split(',').map(Into::into).collect(),
            unknown_trigger_bits: 0,
            repeat_count: 2,
            interval_ms: 5,
            steps: vec![MacroStep::default(); steps],
            macro_slot: None,
        }
    }

    fn descriptor(blob: &[u8], slot: Slot, index: u8) -> &[u8] {
        let at = descriptor_region_offset(slot).unwrap()
            + usize::from(index) * tables::MACRO_DESCRIPTOR_SIZE;
        &blob[at..at + tables::MACRO_DESCRIPTOR_SIZE]
    }

    /// Every byte index where `a` and `b` differ, the CRC left out.
    fn changed(a: &[u8], b: &[u8]) -> Vec<usize> {
        (0..a.len()).filter(|&i| a[i] != b[i] && !CRC.contains(&i)).collect()
    }

    fn sealed(blob: &[u8]) -> bool {
        let mut copy = blob.to_vec();
        seal_crc(&mut copy).unwrap();
        copy == blob
    }

    #[test]
    fn a_put_writes_one_descriptor_and_keeps_the_other_three() {
        let slot = Slot::new(2).unwrap();
        let profile = super::super::profile::default_profile(XINPUT);
        let others = [macro_def("A", "l1", 3), macro_def("B", "r1", 4), macro_def("D", "l2", 5)];
        let mut base = compile_profile(&profile, slot, &[], &others).unwrap();
        // Move the third macro to macro slot 3, so macro slot 2 is the free one.
        let third = descriptor(&base, slot, 2).to_vec();
        base = set_macro(&base, slot, MacroSlot::new(2).unwrap(), None).unwrap();
        let at = descriptor_region_offset(slot).unwrap() + 3 * tables::MACRO_DESCRIPTOR_SIZE;
        put_slice(&mut base, at, &third).unwrap();
        seal_crc(&mut base).unwrap();

        let new = macro_def("New", "r4", 7);
        let m2 = MacroSlot::new(2).unwrap();
        let out = set_macro(&base, slot, m2, Some(&new)).unwrap();
        assert_eq!(descriptor(&out, slot, 2), encode_macro_metadata(&new, m2).unwrap());
        for i in [0, 1, 3] {
            assert_eq!(descriptor(&out, slot, i), descriptor(&base, slot, i), "macro slot {i}");
        }
        let at = descriptor_region_offset(slot).unwrap() + 2 * tables::MACRO_DESCRIPTOR_SIZE;
        assert!(changed(&base, &out).iter().all(|i| (at..at + 52).contains(i)));
        assert!(sealed(&out));
    }

    #[test]
    fn a_delete_zeroes_one_descriptor_and_nothing_else() {
        let slot = Slot::new(1).unwrap();
        let profile = super::super::profile::default_profile(XINPUT);
        let base = compile_profile(&profile, slot, &[], &[macro_def("A", "l1", 3)]).unwrap();
        let out = set_macro(&base, slot, MacroSlot::new(0).unwrap(), None).unwrap();
        assert_eq!(descriptor(&out, slot, 0), [0; tables::MACRO_DESCRIPTOR_SIZE]);
        assert_eq!(
            changed(&base, &out).len(),
            descriptor(&base, slot, 0).iter().filter(|&&b| b != 0).count()
        );
        assert_eq!(decode_macro_metadata(&out, slot).unwrap(), []);
        assert!(sealed(&out));
        assert!(set_macro(&base[1..], slot, MacroSlot::new(0).unwrap(), None).is_err());
    }

    #[test]
    fn a_remap_takes_the_button_off_its_macro_and_keeps_the_macro() {
        let slot = Slot::new(3).unwrap();
        let profile = super::super::profile::default_profile(XINPUT);
        let mut defs = [macro_def("Two", "l1,r1", 3), macro_def("One", "l2", 4)];
        defs[1].macro_slot = Some(1);
        let base = compile_profile(&profile, slot, &[], &defs).unwrap();
        let out = drop_macros(&base, slot, &["l1".into(), "l2".into(), "nope".into()]).unwrap();
        let read = decode_macro_metadata(&out, slot).unwrap();
        let buttons: Vec<_> =
            read.iter().map(|d| (d.name.as_str(), d.trigger_buttons.clone())).collect();
        assert_eq!(buttons, [("Two", vec!["r1".to_owned()]), ("One", vec![])]);
        let key_maps = [0, 1].map(|i| {
            descriptor_region_offset(slot).unwrap()
                + i * tables::MACRO_DESCRIPTOR_SIZE
                + tables::MACRO_KEY_MAP_OFFSET
        });
        assert!(changed(&base, &out)
            .iter()
            .all(|i| key_maps.iter().any(|k| (k..&(k + 4)).contains(&i))));
        assert!(sealed(&out));
    }

    /// `dinput-macro.blob` holds the official app's `r4` macro in macro slot 3 of every
    /// slot. Deleting it and putting back what the decoder reads gives the same bank.
    #[test]
    fn a_delete_and_a_put_of_the_read_macro_give_back_the_golden_bank() {
        let golden = std::fs::read("../../fixtures/pro3/dinput-macro.blob").unwrap();
        let (slot, m3) = (Slot::new(1).unwrap(), MacroSlot::new(3).unwrap());
        let read = decode_macro_metadata(&golden, slot).unwrap();
        let def = read.iter().find(|d| d.macro_slot == Some(3)).unwrap();
        assert_eq!(def.mode, DINPUT);

        let deleted = set_macro(&golden, slot, m3, None).unwrap();
        assert_eq!(descriptor(&deleted, slot, 3), [0; tables::MACRO_DESCRIPTOR_SIZE]);
        for n in 2..=3 {
            let other = Slot::new(n).unwrap();
            assert_eq!(descriptor(&deleted, other, 3), descriptor(&golden, other, 3));
        }
        assert_eq!(set_macro(&deleted, slot, m3, Some(def)).unwrap(), golden);
    }

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
