//! Bounds-checked little/big-endian readers so codec code stays panic-free.

use crate::error::{Error, Result};

fn slice<const N: usize>(buf: &[u8], off: usize) -> Result<[u8; N]> {
    let end = off
        .checked_add(N)
        .ok_or_else(|| Error::Decode(format!("read of {N}B at 0x{off:04X} overflows")))?;
    buf.get(off..end)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| Error::Decode(format!("read of {N}B at 0x{off:04X} out of range")))
}

/// Reads one byte at `off`.
///
/// # Errors
/// Returns [`Error::Decode`] if `off` is out of range.
pub fn read_u8(buf: &[u8], off: usize) -> Result<u8> {
    let [b] = slice::<1>(buf, off)?;
    Ok(b)
}

/// Reads a little-endian `u16` at `off`.
///
/// # Errors
/// Returns [`Error::Decode`] if the 2-byte read is out of range.
pub fn read_u16_le(buf: &[u8], off: usize) -> Result<u16> {
    Ok(u16::from_le_bytes(slice::<2>(buf, off)?))
}

/// Reads a big-endian `u16` at `off`.
///
/// # Errors
/// Returns [`Error::Decode`] if the 2-byte read is out of range.
pub fn read_u16_be(buf: &[u8], off: usize) -> Result<u16> {
    Ok(u16::from_be_bytes(slice::<2>(buf, off)?))
}

/// Reads a little-endian `u32` at `off`.
///
/// # Errors
/// Returns [`Error::Decode`] if the 4-byte read is out of range.
pub fn read_u32_le(buf: &[u8], off: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(slice::<4>(buf, off)?))
}

/// Returns the `len`-byte subslice at `off`.
///
/// # Errors
/// Returns [`Error::Decode`] if the range is out of bounds.
pub fn take(buf: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    let end = off
        .checked_add(len)
        .ok_or_else(|| Error::Decode(format!("read of {len}B at 0x{off:04X} overflows")))?;
    buf.get(off..end)
        .ok_or_else(|| Error::Decode(format!("read of {len}B at 0x{off:04X} out of range")))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn reads_little_endian_within_bounds() {
        let buf = [0x2C, 0x09, 0xAB, 0xCD];
        assert_eq!(read_u16_le(&buf, 0).unwrap(), 0x092C);
        // Non-zero offset: bytes at [2..4] = [0xAB, 0xCD] → LE u16 = 0xCDAB
        assert_eq!(read_u16_le(&buf, 2).unwrap(), 0xCDAB);
    }
    #[test]
    fn out_of_range_is_decode_error() {
        let buf = [0x00, 0x01];
        assert!(matches!(read_u32_le(&buf, 0), Err(Error::Decode(_))));
    }
}
