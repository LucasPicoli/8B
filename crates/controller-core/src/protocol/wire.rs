//! 64-byte interrupt packet builders and response decoding (config protocol).

use crate::error::{Error, Result};
use crate::protocol::bytes::{read_u16_le, take};
use crate::protocol::crc16::crc16_modbus;

const PACKET_LEN: usize = 64;
const PROFILE_SIG: [u8; 2] = [0x2C, 0x09]; // 0x092C little-endian
/// Where every request carries its payload and starts its CRC window, in every mode.
/// The two bytes before it are zero. Official `DInput` writes use 18 too.
pub const PAYLOAD_OFFSET: usize = 18;

/// Builds the `START_CONFIG` packet (`81 04 00 01` + zero padding).
#[must_use]
pub const fn build_start_config() -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x00;
    p[3] = 0x01;
    p
}

/// Builds the `SLOT_SELECT` (`CMD_0x14`) packet for the given slot-select value.
#[must_use]
pub const fn build_slot_select(slot_select_value: u8) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x14;
    p[3] = 0x00;
    p[4] = slot_select_value;
    p[8] = 0xFF;
    p[9] = 0xFF;
    p
}

/// Builds a `PROFILE_UPLOAD` request for `chunk` at `offset`.
///
/// The `0xCC` filler sits at [`PAYLOAD_OFFSET`] in every mode, and the
/// CRC-16/MODBUS covers it. A `DInput` request with the filler at 16 makes the
/// device return a repeating 4-byte pattern instead of the blob.
#[must_use]
pub fn build_upload_packet(offset: u16, chunk: &[u8]) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    let len = chunk.len().min(PACKET_LEN - PAYLOAD_OFFSET);
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x02;
    p[3] = 0x00;
    // len <= 46, fits in u16
    #[allow(clippy::cast_possible_truncation)]
    let len_bytes = (len as u16).to_le_bytes();
    p[6] = len_bytes[0];
    p[7] = len_bytes[1];
    p[10] = PROFILE_SIG[0];
    p[11] = PROFILE_SIG[1];
    let off_bytes = offset.to_le_bytes();
    p[14] = off_bytes[0];
    p[15] = off_bytes[1];
    let window = PAYLOAD_OFFSET..PAYLOAD_OFFSET + len;
    if let (Some(dst), Some(src)) = (p.get_mut(window.clone()), chunk.get(..len)) {
        dst.copy_from_slice(src);
    }
    let crc = p.get(window).map_or_else(|| crc16_modbus(&[]), crc16_modbus);
    let crc_bytes = crc.to_le_bytes();
    p[8] = crc_bytes[0];
    p[9] = crc_bytes[1];
    p
}

/// Decodes a `PROFILE_UPLOAD` response and extracts the payload chunk.
///
/// Validates (matching C++ `decodeUploadChunk`): `resp.len() >= 64`; header
/// `[0..3] == [02 04 04]`; command echo `[4] == 0x02`; echoed size at `[6..8]`
/// equals `expected_chunk_size`; echoed offset at `[14..16]` equals
/// `expected_offset`. Returns `resp[18..18+echo_size]`.
///
/// # Errors
/// Returns [`Error::Decode`] on any header/echo mismatch or truncated payload.
pub fn decode_upload_response(
    resp: &[u8],
    expected_offset: u16,
    expected_chunk_size: u16,
) -> Result<Vec<u8>> {
    let header = take(resp, 0, 3)?;
    if header != [0x02, 0x04, 0x04] || take(resp, 4, 1)? != [0x02] {
        return Err(Error::Decode("unexpected upload response header".to_owned()));
    }
    let echo_size = read_u16_le(resp, 6)?;
    let echo_offset = read_u16_le(resp, 14)?;
    if echo_offset != expected_offset {
        return Err(Error::Decode(format!(
            "upload echo offset mismatch: expected 0x{expected_offset:04X}, got 0x{echo_offset:04X}"
        )));
    }
    if echo_size != expected_chunk_size {
        return Err(Error::Decode(format!(
            "upload echo size mismatch: expected {expected_chunk_size}, got {echo_size}"
        )));
    }
    Ok(take(resp, 18, usize::from(echo_size))?.to_vec())
}

/// Builds a `QUERY_STATUS` (`CMD 0x07`) packet.
///
/// **Warning:** sending this command disrupts HID/joydev reports until the device
/// is physically reconnected. Use only in the macro-read "app-style" session;
/// never send it in the profile-read path.
#[must_use]
pub const fn build_query_status() -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x07;
    p[3] = 0x00;
    p
}

/// Builds the input-stream toggle `81 04 07 00 xx`: `resume = false` pauses the
/// gamepad input stream, `resume = true` resumes it.
///
/// The pause is the same packet as [`build_query_status`]. While paused, the
/// gamepad sends no input, and config replies no longer race the input reports.
#[must_use]
pub const fn build_input_stream(resume: bool) -> [u8; PACKET_LEN] {
    let mut p = build_query_status();
    p[4] = resume as u8;
    p
}

/// Builds a macro-read request packet (`CMD 0x02/0x01` subspace).
///
/// Byte layout: `[0..4] = 81 04 02 01`; `[4]=profile_slot`; `[5]=gamepad_mode`;
/// `[6..8]=chunk_size LE16`; `[8..10]=CRC-16/MODBUS LE16`; `[10..12]=total_len LE16`;
/// `[14..16]=offset LE16`; `[18..18+chunk_size]=0xCC placeholder bytes`.
/// `chunk_size` must be ≤ 32.
#[must_use]
pub fn build_read_macro_packet(
    profile_slot: u8,
    gamepad_mode: u8,
    offset: u16,
    total_len: u16,
    chunk_size: u16,
) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x02;
    p[3] = 0x01;
    p[4] = profile_slot;
    p[5] = gamepad_mode;
    let cs = chunk_size.to_le_bytes();
    p[6] = cs[0];
    p[7] = cs[1];
    let tl = total_len.to_le_bytes();
    p[10] = tl[0];
    p[11] = tl[1];
    let off = offset.to_le_bytes();
    p[14] = off[0];
    p[15] = off[1];
    // fill [18..18+chunk_size] with 0xCC (chunk_size <= 32, end <= 50, within bounds)
    let fill_end = 18usize.saturating_add(chunk_size as usize);
    if let Some(region) = p.get_mut(18..fill_end) {
        for b in region {
            *b = 0xCC;
        }
    }
    // CRC-16/MODBUS over [18..18+chunk_size]
    let crc = p.get(18..fill_end).map_or_else(|| crc16_modbus(&[]), crc16_modbus);
    let crc_bytes = crc.to_le_bytes();
    p[8] = crc_bytes[0];
    p[9] = crc_bytes[1];
    p
}

/// Decodes a macro-read response and extracts the payload chunk.
///
/// Validates: header `[0..3] == [02 04 04]`; `[4]==0x02 && [5]==0x01`;
/// echo `total_len` at `[10..12]` matches `expected_total_len`;
/// echo `offset` at `[14..16]` matches `expected_offset`.
/// Returns `resp[18..18+echo_size]`.
///
/// # Errors
/// Returns [`Error::Decode`] on any validation failure or truncated response.
pub fn decode_read_macro_response(
    resp: &[u8],
    expected_offset: u16,
    expected_total_len: u16,
) -> Result<Vec<u8>> {
    if resp.len() < PACKET_LEN {
        return Err(Error::Decode(format!("read-macro response too short: {} bytes", resp.len())));
    }
    let header = take(resp, 0, 3)?;
    if header != [0x02, 0x04, 0x04] {
        return Err(Error::Decode("unexpected read-macro response header".to_owned()));
    }
    let cmd_echo = take(resp, 4, 2)?;
    if cmd_echo != [0x02, 0x01] {
        return Err(Error::Decode("read-macro response: expected 0x02/0x01 echo".to_owned()));
    }
    let echo_size = read_u16_le(resp, 6)?;
    let echo_total_len = read_u16_le(resp, 10)?;
    let echo_offset = read_u16_le(resp, 14)?;
    if echo_total_len != expected_total_len {
        return Err(Error::Decode(format!(
            "read-macro total_len mismatch: expected {expected_total_len}, got {echo_total_len}"
        )));
    }
    if echo_offset != expected_offset {
        return Err(Error::Decode(format!(
            "read-macro offset mismatch: expected 0x{expected_offset:04X}, got 0x{echo_offset:04X}"
        )));
    }
    Ok(take(resp, 18, usize::from(echo_size))?.to_vec())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    #[test]
    fn upload_packet_has_header_size_and_crc() {
        let chunk = [0xCCu8; 45];
        let pkt = build_upload_packet(0, &chunk);
        assert_eq!(&pkt[0..4], &[0x81, 0x04, 0x02, 0x00]);
        assert_eq!(u16::from_le_bytes([pkt[6], pkt[7]]), 45); // chunk size
        assert_eq!(&pkt[10..12], &[0x2C, 0x09]); // profile sig
        assert_eq!(&pkt[18..18 + 45], &chunk); // payload at offset 18
        assert_eq!(u16::from_le_bytes([pkt[8], pkt[9]]), crc16_modbus(&[0xCCu8; 45]));
    }
    #[test]
    fn decode_response_extracts_payload() {
        let mut resp = vec![0u8; 64];
        resp[0..5].copy_from_slice(&[0x02, 0x04, 0x04, 0x00, 0x02]);
        resp[6..8].copy_from_slice(&4u16.to_le_bytes()); // echo size
        resp[14..16].copy_from_slice(&8u16.to_le_bytes()); // echo offset
        resp[18..22].copy_from_slice(&[1, 2, 3, 4]);
        let payload = decode_upload_response(&resp, 8, 4).unwrap();
        assert_eq!(payload.as_slice(), &[1, 2, 3, 4][..]);
    }
    #[test]
    fn decode_response_rejects_wrong_command_echo() {
        let mut resp = vec![0u8; 64];
        // header ok but byte[4] != 0x02 (e.g. a slot-select ack).
        resp[0..5].copy_from_slice(&[0x02, 0x04, 0x04, 0x00, 0x14]);
        resp[6..8].copy_from_slice(&4u16.to_le_bytes());
        resp[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert!(decode_upload_response(&resp, 8, 4).is_err());
    }
    #[test]
    fn decode_response_rejects_echo_offset_or_size_mismatch() {
        let mut resp = vec![0u8; 64];
        resp[0..5].copy_from_slice(&[0x02, 0x04, 0x04, 0x00, 0x02]);
        resp[6..8].copy_from_slice(&4u16.to_le_bytes());
        resp[14..16].copy_from_slice(&8u16.to_le_bytes());
        assert!(decode_upload_response(&resp, 99, 4).is_err()); // offset mismatch
        assert!(decode_upload_response(&resp, 8, 45).is_err()); // size mismatch
    }

    #[test]
    fn build_query_status_header() {
        let p = build_query_status();
        assert_eq!(&p[0..4], &[0x81, 0x04, 0x07, 0x00]);
        assert!(p[4..].iter().all(|&b| b == 0));
    }

    #[test]
    fn build_read_macro_packet_fields_and_fill() {
        let pkt = build_read_macro_packet(1, 0x03, 0x0020, 0x00C8, 32);
        assert_eq!(&pkt[0..4], &[0x81, 0x04, 0x02, 0x01]);
        assert_eq!(pkt[4], 1); // profile_slot
        assert_eq!(pkt[5], 0x03); // gamepad_mode
        assert_eq!(u16::from_le_bytes([pkt[6], pkt[7]]), 32); // chunk_size
        assert_eq!(u16::from_le_bytes([pkt[10], pkt[11]]), 0x00C8); // total_len
        assert_eq!(u16::from_le_bytes([pkt[14], pkt[15]]), 0x0020); // offset
        assert!(pkt[18..50].iter().all(|&b| b == 0xCC)); // 0xCC fill
                                                         // CRC must be non-zero for all-0xCC (sanity: not zero)
        let crc = u16::from_le_bytes([pkt[8], pkt[9]]);
        assert_ne!(crc, 0);
    }

    #[test]
    fn decode_read_macro_response_roundtrip() {
        let mut resp = vec![0u8; 64];
        resp[0..3].copy_from_slice(&[0x02, 0x04, 0x04]);
        resp[4..6].copy_from_slice(&[0x02, 0x01]);
        resp[6..8].copy_from_slice(&8u16.to_le_bytes()); // echo_size
        resp[10..12].copy_from_slice(&100u16.to_le_bytes()); // echo_total_len
        resp[14..16].copy_from_slice(&32u16.to_le_bytes()); // echo_offset
        resp[18..26].copy_from_slice(&[0xAA; 8]);
        let data = decode_read_macro_response(&resp, 32, 100).unwrap();
        assert_eq!(data, vec![0xAAu8; 8]);
    }

    #[test]
    fn decode_read_macro_response_offset_mismatch() {
        let mut resp = vec![0u8; 64];
        resp[0..3].copy_from_slice(&[0x02, 0x04, 0x04]);
        resp[4..6].copy_from_slice(&[0x02, 0x01]);
        resp[6..8].copy_from_slice(&8u16.to_le_bytes());
        resp[10..12].copy_from_slice(&100u16.to_le_bytes());
        resp[14..16].copy_from_slice(&99u16.to_le_bytes()); // wrong offset
        assert!(decode_read_macro_response(&resp, 32, 100).is_err());
    }

    #[test]
    fn decode_read_macro_response_bad_header() {
        let resp = vec![0u8; 64];
        assert!(decode_read_macro_response(&resp, 0, 10).is_err());
    }
}
