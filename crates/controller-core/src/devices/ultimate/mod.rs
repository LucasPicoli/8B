//! 8BitDo Ultimate controller backend: the codec of its profile images.
//!
//! The pad keeps one 1652-byte image per mode, `XInput` and Switch. Each image holds
//! three profile slots with their macros; [`tables`] lays the bytes out.

pub mod edit;
pub mod macros;
pub mod profile;
pub mod settings;
pub mod tables;

use crate::error::{Error, Result};
use crate::model::{Mode, Slot};
use crate::protocol::bytes::{read_u16_le, read_u32_le};

use tables::{
    BLOB_SIZE, FLAG_LEN, GAMEPAD_MODE_OFFSET, GAMEPAD_SWITCH, GAMEPAD_XINPUT, IN_USE, SLOT_COUNT,
    SLOT_FLAG_OFFSET,
};

/// The Ultimate's `XInput` mode.
pub const XINPUT: Mode = Mode::from_static("xinput");
/// The Ultimate's Nintendo Switch mode.
pub const SWITCH: Mode = Mode::from_static("switch");
/// The Ultimate's modes.
pub const MODES: [Mode; 2] = [XINPUT, SWITCH];

/// The image-mode value of `mode`, also the value that selects its image.
///
/// # Errors
/// Returns [`Error::Validation`] for a mode the Ultimate does not have.
pub fn mode_byte(mode: Mode) -> Result<u8> {
    match mode {
        XINPUT => Ok(GAMEPAD_XINPUT),
        SWITCH => Ok(GAMEPAD_SWITCH),
        other => Err(Error::Validation(format!("the Ultimate has no {other} mode"))),
    }
}

/// The mode of the image `blob`, from the mode it records.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is too short or records another mode.
pub fn image_mode(blob: &[u8]) -> Result<Mode> {
    let value = read_u16_le(blob, GAMEPAD_MODE_OFFSET)?;
    MODES
        .into_iter()
        .find(|&m| mode_byte(m).is_ok_and(|b| u16::from(b) == value))
        .ok_or_else(|| Error::Decode(format!("the image records an unknown mode {value}")))
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
