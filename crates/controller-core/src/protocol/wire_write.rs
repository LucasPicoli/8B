//! 64-byte packet builders and response validators for the write half of the
//! config protocol (profile write, apply, macro erase, macro write).
//!
//! Ports the anonymous-namespace helpers of C++ `profile_write_service.cpp`.
//! Read-side packets live in [`super::wire`].

use crate::error::{Error, Result};
use crate::protocol::bytes::{read_u16_le, take};
use crate::protocol::crc16::crc16_modbus;
use crate::protocol::wire::PAYLOAD_OFFSET;

/// Length of every config packet.
pub const PACKET_LEN: usize = 64;
/// Bytes per macro write chunk.
pub const MACRO_CHUNK_LEN: usize = 32;
/// Bytes erased per macro slot, and the flash stride between macro slots.
pub const MACRO_PAGE_LEN: u16 = 0x1000;

const CMD_PROFILE_WRITE: u8 = 0x01;
const CMD_WRITE_MACRO: u8 = 0x03;
const CMD_ERASE_MACRO: u8 = 0x04;
const CMD_APPLY: u8 = 0x06;
const APPLY_CONST: [u8; 2] = [0x23, 0x01];

const OFF_CHUNK_SIZE: usize = 6;
const OFF_CRC: usize = 8;
const OFF_SIG_OR_TOTAL: usize = 10;
const OFF_FLASH_OFFSET: usize = 14;

/// Returns a zeroed packet with the `81 04 <cmd> <sub>` header set.
const fn header(cmd: u8, sub: u8) -> [u8; PACKET_LEN] {
    let mut p = [0u8; PACKET_LEN];
    p[0] = 0x81;
    p[1] = 0x04;
    p[2] = cmd;
    p[3] = sub;
    p
}

fn put16(p: &mut [u8; PACKET_LEN], off: usize, value: u16) {
    if let Some(dst) = p.get_mut(off..off + 2) {
        dst.copy_from_slice(&value.to_le_bytes());
    }
}

/// Copies up to `max` bytes of `chunk` into the packet at `at`.
fn put_payload(p: &mut [u8; PACKET_LEN], at: usize, chunk: &[u8], max: usize) {
    let len = chunk.len().min(max).min(PACKET_LEN.saturating_sub(at));
    if let (Some(dst), Some(src)) = (p.get_mut(at..at + len), chunk.get(..len)) {
        dst.copy_from_slice(src);
    }
}

/// Stores the CRC-16/MODBUS of `wire[18..18+len]` at bytes 8-9.
fn stamp_crc(p: &mut [u8; PACKET_LEN], len: usize) {
    let crc =
        p.get(PAYLOAD_OFFSET..PAYLOAD_OFFSET + len).map_or_else(|| crc16_modbus(&[]), crc16_modbus);
    put16(p, OFF_CRC, crc);
}

fn len_u16(len: usize) -> u16 {
    u16::try_from(len).unwrap_or(u16::MAX)
}

/// Builds a `PROFILE_WRITE` packet for `chunk` at `offset` of a `blob_size`-byte blob.
///
/// The payload and its CRC window start at [`PAYLOAD_OFFSET`] in every mode. The C++
/// oracle put `DInput` payloads at 16; the official app's `DInput` writes use 18.
#[must_use]
pub fn build_write_packet(offset: u16, chunk: &[u8], blob_size: u16) -> [u8; PACKET_LEN] {
    let mut p = header(CMD_PROFILE_WRITE, 0x00);
    put16(&mut p, OFF_CHUNK_SIZE, len_u16(chunk.len()));
    put16(&mut p, OFF_SIG_OR_TOTAL, blob_size);
    put16(&mut p, OFF_FLASH_OFFSET, offset);
    put_payload(&mut p, PAYLOAD_OFFSET, chunk, PACKET_LEN);
    stamp_crc(&mut p, chunk.len());
    p
}

/// Builds the `PROFILE_APPLY` packet (`81 04 06 00 23 01`).
#[must_use]
pub const fn build_apply() -> [u8; PACKET_LEN] {
    let mut p = header(CMD_APPLY, 0x00);
    p[4] = APPLY_CONST[0];
    p[5] = APPLY_CONST[1];
    p
}

/// Builds a macro erase packet (`CMD_0x04`). The flash offset is `macro_slot * 4096`.
#[must_use]
pub fn build_erase_macro(
    cmd_mode: u8,
    profile_slot0: u8,
    gamepad_mode: u8,
    macro_slot: u8,
) -> [u8; PACKET_LEN] {
    let mut p = header(CMD_ERASE_MACRO, cmd_mode);
    p[4] = profile_slot0;
    p[5] = gamepad_mode;
    put16(&mut p, OFF_CHUNK_SIZE, MACRO_PAGE_LEN);
    put16(&mut p, OFF_FLASH_OFFSET, u16::from(macro_slot).wrapping_mul(MACRO_PAGE_LEN));
    p
}

/// Builds a macro write packet (`CMD_0x03`) carrying up to 32 bytes of step data.
///
/// `offset` is the absolute flash offset of this chunk; `total_len` is the padded
/// stream size plus `macro_slot * 4096`.
#[must_use]
pub fn build_write_macro(
    cmd_mode: u8,
    profile_slot0: u8,
    gamepad_mode: u8,
    chunk: &[u8],
    offset: u16,
    total_len: u16,
) -> [u8; PACKET_LEN] {
    let mut p = header(CMD_WRITE_MACRO, cmd_mode);
    p[4] = profile_slot0;
    p[5] = gamepad_mode;
    put16(&mut p, OFF_CHUNK_SIZE, len_u16(MACRO_CHUNK_LEN));
    put16(&mut p, OFF_SIG_OR_TOTAL, total_len);
    put16(&mut p, OFF_FLASH_OFFSET, offset);
    put_payload(&mut p, PAYLOAD_OFFSET, chunk, MACRO_CHUNK_LEN);
    stamp_crc(&mut p, MACRO_CHUNK_LEN);
    p
}

fn check_len(resp: &[u8], what: &str) -> Result<()> {
    if resp.len() < PACKET_LEN {
        return Err(Error::Decode(format!("{what} response too short ({} bytes)", resp.len())));
    }
    Ok(())
}

/// Validates a `PROFILE_WRITE` response: header `02 04 04 _ 01`, echoed size and offset.
///
/// # Errors
/// Returns [`Error::Decode`] on a short response, wrong header, or echo mismatch.
pub fn validate_write_response(resp: &[u8], offset: u16, size: u16) -> Result<()> {
    check_len(resp, "PROFILE_WRITE")?;
    if take(resp, 0, 3)? != [0x02, 0x04, 0x04] || take(resp, 4, 1)? != [CMD_PROFILE_WRITE] {
        return Err(Error::Decode("unexpected PROFILE_WRITE response header".to_owned()));
    }
    let echo_size = read_u16_le(resp, OFF_CHUNK_SIZE)?;
    let echo_offset = read_u16_le(resp, OFF_FLASH_OFFSET)?;
    if echo_size != size {
        return Err(Error::Decode(format!("echo size mismatch: expected {size}, got {echo_size}")));
    }
    if echo_offset != offset {
        return Err(Error::Decode(format!(
            "echo offset mismatch: expected {offset}, got {echo_offset}"
        )));
    }
    Ok(())
}

/// Validates a simple command response (apply, query status, slot select):
/// header `02 04 04`, byte 4 echoes `command`.
///
/// # Errors
/// Returns [`Error::Decode`] on a short response, wrong header, or wrong echo.
pub fn validate_command_response(resp: &[u8], command: u8) -> Result<()> {
    check_len(resp, "command")?;
    if take(resp, 0, 3)? != [0x02, 0x04, 0x04] {
        return Err(Error::Decode("unexpected response header".to_owned()));
    }
    let echo = take(resp, 4, 1)?;
    if echo != [command] {
        return Err(Error::Decode(format!(
            "response echoed command 0x{:02X}, expected 0x{command:02X}",
            echo.first().copied().unwrap_or(0)
        )));
    }
    Ok(())
}

/// Macro responses carry `02 04` then byte 2 of `04` or `05` (the C++ hardware notes).
fn check_macro_header(resp: &[u8], command: u8, name: &str) -> Result<()> {
    check_len(resp, name)?;
    let head = take(resp, 0, 3)?;
    if head != [0x02, 0x04, 0x04] && head != [0x02, 0x04, 0x05] {
        return Err(Error::Decode(format!("unexpected {name} response header")));
    }
    if take(resp, 4, 1)? != [command] {
        return Err(Error::Decode(format!(
            "{name} response: expected command echo 0x{command:02X}"
        )));
    }
    Ok(())
}

/// Validates a macro erase response: echo `0x04` and echoed length `0x1000`.
///
/// # Errors
/// Returns [`Error::Decode`] on a short response, wrong header, echo, or length.
pub fn validate_erase_response(resp: &[u8]) -> Result<()> {
    check_macro_header(resp, CMD_ERASE_MACRO, "EraseMacro")?;
    let echo_len = read_u16_le(resp, OFF_CHUNK_SIZE)?;
    if echo_len != MACRO_PAGE_LEN {
        return Err(Error::Decode(format!(
            "EraseMacro echo length mismatch: expected {MACRO_PAGE_LEN}, got {echo_len}"
        )));
    }
    Ok(())
}

/// Validates a macro write response: echo `0x03`.
///
/// # Errors
/// Returns [`Error::Decode`] on a short response, wrong header, or wrong echo.
pub fn validate_write_macro_response(resp: &[u8]) -> Result<()> {
    check_macro_header(resp, CMD_WRITE_MACRO, "WriteMacro")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn response(b2: u8, cmd: u8) -> Vec<u8> {
        let mut r = vec![0u8; PACKET_LEN];
        r[0..5].copy_from_slice(&[0x02, 0x04, b2, 0x00, cmd]);
        r
    }

    #[test]
    fn write_packet_layout() {
        let chunk = [0xAB; 45];
        let p = build_write_packet(0x0087, &chunk, 0x092C);
        assert_eq!(&p[0..4], &[0x81, 0x04, 0x01, 0x00]);
        assert_eq!(u16::from_le_bytes([p[6], p[7]]), 45);
        assert_eq!(&p[10..12], &[0x2C, 0x09]);
        assert_eq!(u16::from_le_bytes([p[14], p[15]]), 0x0087);
        assert_eq!(&p[16..18], &[0, 0]);
        assert_eq!(&p[18..63], &chunk);
        assert_eq!(u16::from_le_bytes([p[8], p[9]]), crc16_modbus(&chunk));
    }

    #[test]
    fn apply_bytes() {
        let p = build_apply();
        assert_eq!(&p[0..6], &[0x81, 0x04, 0x06, 0x00, 0x23, 0x01]);
        assert!(p[6..].iter().all(|&b| b == 0));
    }

    #[test]
    fn erase_macro_bytes() {
        let p = build_erase_macro(0x01, 2, 0x03, 3);
        assert_eq!(&p[0..6], &[0x81, 0x04, 0x04, 0x01, 2, 0x03]);
        assert_eq!(u16::from_le_bytes([p[6], p[7]]), 0x1000);
        assert_eq!(u16::from_le_bytes([p[14], p[15]]), 3 * 0x1000);
    }

    #[test]
    fn write_macro_bytes() {
        let chunk = [0x5A; 32];
        let p = build_write_macro(0x01, 1, 0x00, &chunk, 0x2020, 0x2040);
        assert_eq!(&p[0..6], &[0x81, 0x04, 0x03, 0x01, 1, 0x00]);
        assert_eq!(u16::from_le_bytes([p[6], p[7]]), 32);
        assert_eq!(u16::from_le_bytes([p[10], p[11]]), 0x2040);
        assert_eq!(u16::from_le_bytes([p[14], p[15]]), 0x2020);
        assert_eq!(&p[18..50], &chunk);
        assert_eq!(u16::from_le_bytes([p[8], p[9]]), crc16_modbus(&chunk));
    }

    #[test]
    fn write_response_accepts_good_and_rejects_bad() {
        let mut r = response(0x04, 0x01);
        r[6..8].copy_from_slice(&45u16.to_le_bytes());
        r[14..16].copy_from_slice(&90u16.to_le_bytes());
        validate_write_response(&r, 90, 45).unwrap();
        assert!(validate_write_response(&r, 91, 45).is_err());
        assert!(validate_write_response(&r, 90, 8).is_err());
        assert!(validate_write_response(&response(0x04, 0x02), 90, 45).is_err());
        assert!(validate_write_response(&r[..10], 90, 45).is_err());
    }

    #[test]
    fn command_response_checks_echo() {
        validate_command_response(&response(0x04, 0x06), 0x06).unwrap();
        assert!(validate_command_response(&response(0x04, 0x07), 0x06).is_err());
        assert!(validate_command_response(&response(0x05, 0x06), 0x06).is_err());
        assert!(validate_command_response(&[0u8; 10], 0x06).is_err());
    }

    #[test]
    fn erase_response_accepts_04_and_05() {
        for b2 in [0x04, 0x05] {
            let mut r = response(b2, 0x04);
            r[6..8].copy_from_slice(&0x1000u16.to_le_bytes());
            validate_erase_response(&r).unwrap();
        }
        let mut bad_len = response(0x04, 0x04);
        bad_len[6..8].copy_from_slice(&0x0800u16.to_le_bytes());
        assert!(validate_erase_response(&bad_len).is_err());
        assert!(validate_erase_response(&response(0x04, 0x03)).is_err());
        assert!(validate_erase_response(&response(0x06, 0x04)).is_err());
    }

    #[test]
    fn write_macro_response_accepts_04_and_05() {
        validate_write_macro_response(&response(0x04, 0x03)).unwrap();
        validate_write_macro_response(&response(0x05, 0x03)).unwrap();
        assert!(validate_write_macro_response(&response(0x04, 0x04)).is_err());
        assert!(validate_write_macro_response(&[0u8; 4]).is_err());
    }
}
