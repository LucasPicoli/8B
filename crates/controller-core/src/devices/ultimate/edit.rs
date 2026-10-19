//! Writing one slot of an Ultimate image: a profile, its macros, or both.
//!
//! A slot not in use changes only on its first edit. That edit starts every section
//! from the mode's default and sets all 7 flags. In a slot in use, a section whose own
//! flag is not set starts from the default too. The name, the other slots and every
//! byte outside the slots go back as read.

use crate::description::ControllerDescription;
use crate::error::{Error, Result};
use crate::model::{CanonicalProfile, MacroDefinition, MacroSlot, Mode, Slot};
use crate::protocol::bytes::{put_u16_le, put_u32_le};

use super::macros::{decode_macros, write_macros};
use super::profile::{default_profile, Section};
use super::tables::{
    BLOB_SIZE, FLAG_LEN, GAMEPAD_MODE_OFFSET, IN_USE, MACROS_LEN, MACROS_OFFSET, NO_SELECTION,
    SELECTION_OFFSET, SLOT_FLAG_OFFSET,
};
use super::{check_blob, image_mode, mode_byte, read_flag, slot_in_use, slot_index};

/// Where the section flags of slot `s` sit: the 5 settings sections, then the macros.
fn section_starts(s: usize) -> impl Iterator<Item = usize> {
    let settings = Section::ALL.into_iter().map(move |sec| {
        let (base, len) = sec.place();
        base + s * len
    });
    settings.chain([MACROS_OFFSET + s * MACROS_LEN])
}

/// Returns `blob` with `slot` holding `profile`'s settings and button map, and `macros`.
/// `None` keeps what the slot holds, or the mode's default where it holds nothing.
fn write_slot(
    d: &ControllerDescription,
    blob: &[u8],
    slot: Slot,
    profile: Option<&CanonicalProfile>,
    macros: Option<&[MacroDefinition]>,
) -> Result<Vec<u8>> {
    check_blob(blob)?;
    let s = slot_index(slot)?;
    let mode = image_mode(d, blob)?;
    if let Some(p) = profile {
        if p.mode != mode {
            return Err(Error::Validation(format!(
                "a {} profile cannot go into the {mode} image",
                p.mode
            )));
        }
    }
    let first = !slot_in_use(blob, slot)?;
    let default = default_profile(d, mode);
    let mut out = blob.to_vec();
    for sec in Section::ALL {
        let (base, len) = sec.place();
        let at = base + s * len;
        let fresh = first || read_flag(blob, at)? != IN_USE;
        let bytes = out.get_mut(at..at + len).ok_or_else(|| Error::Decode("short image".into()))?;
        if fresh {
            bytes.fill(0);
        }
        match (profile, fresh) {
            (Some(p), _) => sec.write(d, bytes, p)?,
            (None, true) => sec.write(d, bytes, &default)?,
            (None, false) => {}
        }
    }
    let at = MACROS_OFFSET + s * MACROS_LEN;
    let fresh = first || read_flag(blob, at)? != IN_USE;
    let bytes =
        out.get_mut(at..at + MACROS_LEN).ok_or_else(|| Error::Decode("short image".into()))?;
    if fresh {
        bytes.fill(0);
    }
    match (macros, fresh) {
        (Some(defs), _) => write_macros(d, bytes, defs)?,
        (None, true) => write_macros(d, bytes, &[])?,
        (None, false) => {}
    }
    for at in section_starts(s) {
        put_u32_le(&mut out, at, IN_USE)?;
    }
    put_u32_le(&mut out, SLOT_FLAG_OFFSET + s * FLAG_LEN, IN_USE)?;
    Ok(out)
}

/// An image of `mode` with nothing in it, as the pad holds one before its first save:
/// every byte `ff` except the image's mode and the selection, which is none.
///
/// # Errors
/// Returns [`Error::Validation`] for a mode the Ultimate does not have.
pub fn erased_image(d: &ControllerDescription, mode: Mode) -> Result<Vec<u8>> {
    let mut image = vec![0xFF; BLOB_SIZE];
    put_u16_le(&mut image, GAMEPAD_MODE_OFFSET, u16::from(mode_byte(d, mode)?))?;
    put_u16_le(&mut image, SELECTION_OFFSET, NO_SELECTION)?;
    Ok(image)
}

/// Returns `base_blob` with `target_slot` holding `profile` and `macros`. A `base_blob`
/// that is not a full image is taken as an erased image of the profile's mode.
///
/// # Errors
/// Returns [`Error::Validation`] for a slot past the third, a profile of another mode
/// than the image, an unknown output, or a macro the pad cannot hold.
pub fn compile_profile(
    d: &ControllerDescription,
    profile: &CanonicalProfile,
    target_slot: Slot,
    base_blob: &[u8],
    macros: &[MacroDefinition],
) -> Result<Vec<u8>> {
    let erased;
    let base = if base_blob.len() == BLOB_SIZE {
        base_blob
    } else {
        erased = erased_image(d, profile.mode)?;
        &erased
    };
    write_slot(d, base, target_slot, Some(profile), Some(macros))
}

/// Like [`compile_profile`], but keeps the macros `base_blob` holds for `target_slot`
/// byte for byte.
///
/// # Errors
/// Same as [`compile_profile`], and [`Error::Decode`] if `base_blob` is not a full image.
pub fn compile_profile_keep_macros(
    d: &ControllerDescription,
    profile: &CanonicalProfile,
    target_slot: Slot,
    base_blob: &[u8],
) -> Result<Vec<u8>> {
    write_slot(d, base_blob, target_slot, Some(profile), None)
}

/// Returns `blob` with macro slot `macro_slot` of `slot` holding `def`, or empty when
/// `def` is `None`.
///
/// The pad plays the first `count` records, so the macros stay packed in macro-slot
/// order: a macro put past the last one lands right after it.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is not a full image, and [`Error::Validation`]
/// if `def` cannot be encoded.
pub fn set_macro(
    d: &ControllerDescription,
    blob: &[u8],
    slot: Slot,
    macro_slot: MacroSlot,
    def: Option<&MacroDefinition>,
) -> Result<Vec<u8>> {
    let m = macro_slot.get();
    let mut defs: Vec<MacroDefinition> =
        decode_macros(d, blob, slot)?.into_iter().filter(|d| d.macro_slot != Some(m)).collect();
    if let Some(def) = def {
        defs.push(MacroDefinition { macro_slot: Some(m), ..def.clone() });
    }
    write_slot(d, blob, slot, None, Some(&defs))
}

/// Returns `blob` with each button of `triggers` taken off the macros of `slot`. Every
/// macro stays, one left with no button included.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is not a full image.
pub fn drop_macros(
    d: &ControllerDescription,
    blob: &[u8],
    slot: Slot,
    triggers: &[String],
) -> Result<Vec<u8>> {
    let defs: Vec<MacroDefinition> = decode_macros(d, blob, slot)?
        .into_iter()
        .map(|mut d| {
            d.trigger_buttons.retain(|b| !triggers.contains(b));
            d
        })
        .collect();
    write_slot(d, blob, slot, None, Some(&defs))
}

/// Returns `blob` with `slot` empty: its 7 flags cleared, as the vendor app's Delete
/// does. Every other byte stays as read, so the pad skips the slot and its next edit
/// starts from the default.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is not a full image.
pub fn deactivate_profile(blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
    check_blob(blob)?;
    let s = slot_index(slot)?;
    let mut out = blob.to_vec();
    for at in section_starts(s).chain([SLOT_FLAG_OFFSET + s * FLAG_LEN]) {
        put_u32_le(&mut out, at, 0)?;
    }
    Ok(out)
}
