//! Input checks and chunk planning shared by the real and mock write paths.
//!
//! Public so the [`MockDevice`](crate::transport::MockDevice) and real transport share
//! one source of truth, and so callers can pre-check inputs.

use crate::error::{Error, Result};
use crate::model::MacroSlot;
use crate::protocol::wire_write::{MACRO_CHUNK_LEN, MACRO_PAGE_LEN};

/// Size of a full profile blob.
pub const PROFILE_SIZE: usize = 0x092C;
/// Bytes per profile read/write chunk.
pub const PROFILE_CHUNK: usize = 45;
/// A patch write sends its packet this many times.
pub const PATCH_PACKETS: usize = 2;

/// Rejects a profile blob that is not exactly [`PROFILE_SIZE`] bytes.
///
/// # Errors
/// Returns [`Error::Write`] on a wrong size.
pub fn check_profile_blob(blob: &[u8]) -> Result<()> {
    if blob.len() == PROFILE_SIZE {
        Ok(())
    } else {
        Err(Error::write(format!(
            "profile blob must be exactly {PROFILE_SIZE} bytes, got {}",
            blob.len()
        )))
    }
}

/// Rejects an empty patch.
///
/// # Errors
/// Returns [`Error::Write`] if `data` is empty.
pub fn check_patch(data: &[u8]) -> Result<()> {
    if data.is_empty() {
        Err(Error::write("patch data must not be empty"))
    } else {
        Ok(())
    }
}

/// Rejects a macro step stream the device cannot take.
///
/// # Errors
/// Returns [`Error::Write`] if the stream is empty, not a multiple of 32 bytes, longer
/// than one 4096-byte flash page (it would spill into the next macro slot), or its
/// `total_len` field would not fit in 16 bits.
pub fn check_macro_stream(stream: &[u8], macro_slot: MacroSlot) -> Result<()> {
    let len = stream.len();
    if len == 0 || len % MACRO_CHUNK_LEN != 0 {
        return Err(Error::write(format!(
            "macro step stream size ({len}) must be a non-zero multiple of {MACRO_CHUNK_LEN} bytes"
        )));
    }
    if len > usize::from(MACRO_PAGE_LEN) {
        return Err(Error::write(format!(
            "macro step stream ({len} bytes) exceeds one {MACRO_PAGE_LEN}-byte flash page"
        )));
    }
    macro_total_len(len, macro_slot).map(|_| ())
}

/// Flash base offset of a macro slot.
///
/// # Errors
/// Returns [`Error::Write`] if the offset overflows 16 bits.
pub fn macro_flash_base(macro_slot: MacroSlot) -> Result<u16> {
    u16::from(macro_slot.get())
        .checked_mul(MACRO_PAGE_LEN)
        .ok_or_else(|| Error::write("macro flash offset overflows 16 bits"))
}

/// `total_len` field of a macro write: padded size plus the slot's flash base.
///
/// # Errors
/// Returns [`Error::Write`] if the total overflows 16 bits.
pub fn macro_total_len(stream_len: usize, macro_slot: MacroSlot) -> Result<u16> {
    let total = stream_len + usize::from(macro_flash_base(macro_slot)?);
    u16::try_from(total).map_err(|_| Error::write("macro total length overflows 16 bits"))
}

/// Splits a profile blob into `(offset, size)` chunks: 52 of 45 bytes, then one of 8.
#[must_use]
pub fn plan_profile_chunks() -> Vec<(u16, usize)> {
    (0..PROFILE_SIZE)
        .step_by(PROFILE_CHUNK)
        .filter_map(|off| Some((u16::try_from(off).ok()?, PROFILE_CHUNK.min(PROFILE_SIZE - off))))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn profile_chunks_are_52_by_45_then_8() {
        let chunks = plan_profile_chunks();
        assert_eq!(chunks.len(), 53);
        assert!(chunks.iter().take(52).all(|&(_, size)| size == 45));
        assert_eq!(chunks.last().copied(), Some((2340, 8)));
        assert_eq!(chunks.iter().map(|&(_, s)| s).sum::<usize>(), PROFILE_SIZE);
        assert_eq!(chunks.get(1).copied(), Some((45, 45)));
    }

    #[test]
    fn blob_and_patch_checks() {
        assert!(check_profile_blob(&[0; PROFILE_SIZE]).is_ok());
        assert!(check_profile_blob(&[0; 10]).is_err());
        assert!(check_patch(&[1]).is_ok());
        assert!(check_patch(&[]).is_err());
    }

    #[test]
    fn macro_stream_checks_and_total_len() {
        let slot = MacroSlot::new(2).unwrap();
        assert!(check_macro_stream(&[0; 64], slot).is_ok());
        assert!(check_macro_stream(&[], slot).is_err());
        assert!(check_macro_stream(&[0; 33], slot).is_err());
        assert!(check_macro_stream(&[0; 4128], slot).is_err());
        assert_eq!(macro_total_len(64, slot).unwrap(), 64 + 2 * 4096);
        assert_eq!(macro_flash_base(MacroSlot::new(3).unwrap()).unwrap(), 3 * 4096);
    }
}
