//! 64-byte interrupt packet builders and response decoding (config protocol).

use crate::error::{Error, Result};
use crate::protocol::bytes::{read_u16_le, take};
use crate::protocol::crc16::crc16_modbus;

const PACKET_LEN: usize = 64;
const PROFILE_SIG: [u8; 2] = [0x2C, 0x09]; // 0x092C little-endian

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
/// `payload_offset` is 18 (XInput/Switch) or 16 (`DInput`). CRC-16/MODBUS is taken
/// over the canonical payload window `[18..18+len]`.
#[must_use]
pub fn build_upload_packet(offset: u16, chunk: &[u8], payload_offset: usize) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    let len = chunk.len().min(PACKET_LEN.saturating_sub(payload_offset));
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = 0x02;
    p[3] = 0x00;
    // chunk size as LE16 at offset 6; len <= 46 (PACKET_LEN - 18), fits in u16
    #[allow(clippy::cast_possible_truncation)]
    let len_bytes = (len as u16).to_le_bytes();
    p[6] = len_bytes[0];
    p[7] = len_bytes[1];
    // profile sig at offset 10
    p[10] = PROFILE_SIG[0];
    p[11] = PROFILE_SIG[1];
    // blob offset as LE16 at offset 14
    let off_bytes = offset.to_le_bytes();
    p[14] = off_bytes[0];
    p[15] = off_bytes[1];
    // copy payload
    if let Some(dst) = p.get_mut(payload_offset..payload_offset + len) {
        if let Some(src) = chunk.get(..len) {
            dst.copy_from_slice(src);
        }
    }
    // CRC-16/MODBUS over canonical window [18..18+len]
    let crc = p.get(18..18 + len).map_or_else(|| crc16_modbus(&[]), crc16_modbus);
    let crc_bytes = crc.to_le_bytes();
    p[8] = crc_bytes[0];
    p[9] = crc_bytes[1];
    p
}

/// One decoded `PROFILE_UPLOAD` response chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadChunk {
    /// Echoed payload size.
    pub size: u16,
    /// Echoed blob offset.
    pub offset: u16,
    /// Extracted payload bytes.
    pub payload: Vec<u8>,
}

/// Decodes a `PROFILE_UPLOAD` response (header `02 04 04`, payload at `[18..18+size]`).
///
/// # Errors
/// Returns [`Error::Decode`] if the header is wrong or the payload window is short.
pub fn decode_upload_response(resp: &[u8]) -> Result<UploadChunk> {
    let header = take(resp, 0, 3)?;
    if header != [0x02, 0x04, 0x04] {
        return Err(Error::Decode("unexpected upload response header".to_owned()));
    }
    let size = read_u16_le(resp, 6)?;
    let offset = read_u16_le(resp, 14)?;
    let payload = take(resp, 18, size as usize)?.to_vec();
    Ok(UploadChunk { size, offset, payload })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    #[test]
    fn upload_packet_has_header_size_and_crc() {
        let chunk = [0xCCu8; 45];
        let pkt = build_upload_packet(0, &chunk, 18);
        assert_eq!(&pkt[0..4], &[0x81, 0x04, 0x02, 0x00]);
        assert_eq!(u16::from_le_bytes([pkt[6], pkt[7]]), 45); // chunk size
        assert_eq!(&pkt[10..12], &[0x2C, 0x09]); // profile sig
        assert_eq!(&pkt[18..18 + 45], &chunk); // payload at offset 18
    }
    #[test]
    fn decode_response_extracts_payload() {
        let mut resp = vec![0u8; 64];
        resp[0..5].copy_from_slice(&[0x02, 0x04, 0x04, 0x00, 0x02]);
        resp[6..8].copy_from_slice(&4u16.to_le_bytes()); // echo size
        resp[14..16].copy_from_slice(&8u16.to_le_bytes()); // echo offset
        resp[18..22].copy_from_slice(&[1, 2, 3, 4]);
        let c = decode_upload_response(&resp).unwrap();
        assert_eq!((c.size, c.offset, c.payload.as_slice()), (4, 8, &[1, 2, 3, 4][..]));
    }
}
