//! UTF-16BE name decoding for device blobs.

/// Decodes a UTF-16BE name field, stopping at the first NUL unit.
///
/// Does **not** trim surrounding whitespace — matches C++ `decodeName`
/// (`profile_mapper.cpp`), which appends code points verbatim until NUL.
#[must_use]
pub fn decode_utf16be_name(bytes: &[u8]) -> String {
    let mut units: Vec<u16> = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        // Convert &[u8] chunk to [u8; 2] to avoid indexing_slicing lint.
        let arr: [u8; 2] = match <[u8; 2]>::try_from(pair) {
            Ok(a) => a,
            Err(_) => break,
        };
        let unit = u16::from_be_bytes(arr);
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    String::from_utf16_lossy(&units)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    #[test]
    fn decodes_utf16be_until_nul() {
        // "Hi" in UTF-16BE, then a NUL terminator and padding.
        let bytes = [0x00, b'H', 0x00, b'i', 0x00, 0x00, 0x00, 0x00];
        assert_eq!(decode_utf16be_name(&bytes), "Hi");
    }

    #[test]
    fn preserves_surrounding_whitespace() {
        // C++ `decodeName` does not trim — a leading/trailing space must survive.
        let bytes = [0x00, b' ', 0x00, b'A', 0x00, b' ', 0x00, 0x00];
        assert_eq!(decode_utf16be_name(&bytes), " A ");
    }
}
