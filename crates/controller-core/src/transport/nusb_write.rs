//! Write operations of the real USB transport.
//!
//! Each function opens its own session (as the C++ `ProfileWriteService` does), builds
//! packets with [`crate::protocol::wire_write`], and maps failures to the categories
//! the C++ code used: transfer failure on a simple command is [`Error::Timeout`], a
//! rejected response is [`Error::Write`].

use crate::device::ControllerSpec as _;
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{MacroSlot, Mode, Slot};
use crate::protocol::bytes::take;
use crate::protocol::wire::{build_query_status, build_slot_select};
use crate::protocol::wire_write::{
    build_apply, build_erase_macro, build_write_macro, build_write_packet,
    validate_command_response, validate_erase_response, validate_write_macro_response,
    validate_write_response, MACRO_CHUNK_LEN, PACKET_LEN,
};
use crate::transport::session::{Session, WRITE_TIMEOUT};
use crate::transport::write_input::{
    check_macro_stream, check_patch, check_profile_blob, macro_flash_base, macro_total_len,
    plan_profile_chunks, PATCH_PACKETS,
};

/// Byte 3 of the macro commands, always 1 in the captures.
const MACRO_CMD_MODE: u8 = 0x01;
const CMD_SLOT_SELECT: u8 = 0x14;
const CMD_APPLY: u8 = 0x06;
const CMD_QUERY_STATUS: u8 = 0x07;

fn open(spec: Pro3, mode: Mode) -> Result<Session> {
    Session::for_mode(spec, mode, WRITE_TIMEOUT)
}

fn rejected(what: &str, cause: &Error) -> Error {
    Error::write(format!("{what} rejected: {cause}"))
}

/// Ties a failure of packet `index` out of `total` to that packet.
fn chunk_failure(index: usize, total: usize, what: &str, cause: &Error) -> Error {
    Error::Write {
        message: format!("{what} {}/{total}: {cause}", index + 1),
        failed_chunk: Some(index),
        total_chunks: total,
    }
}

fn len_u16(len: usize) -> Result<u16> {
    u16::try_from(len).map_err(|_| Error::write("chunk length overflows 16 bits"))
}

/// Sends `packet`, then validates the response with `validate`.
fn exchange(
    session: &mut Session,
    packet: &[u8; PACKET_LEN],
    validate: impl FnOnce(&[u8]) -> Result<()>,
) -> Result<()> {
    let resp = session.send_recv(packet)?;
    validate(&resp)
}

pub(super) fn write_full_profile(spec: Pro3, mode: Mode, blob: &[u8]) -> Result<()> {
    check_profile_blob(blob)?;
    let chunks = plan_profile_chunks();
    let total = chunks.len();
    let mut session = open(spec, mode)?;
    let payload_offset = session.params.payload_offset;
    for (i, &(offset, size)) in chunks.iter().enumerate() {
        let send = |session: &mut Session| -> Result<()> {
            let data = take(blob, usize::from(offset), size)?;
            let packet = build_write_packet(offset, data, payload_offset);
            let size16 = len_u16(size)?;
            exchange(session, &packet, |r| validate_write_response(r, offset, size16))
        };
        send(&mut session).map_err(|e| chunk_failure(i, total, "profile chunk", &e))?;
    }
    Ok(())
}

pub(super) fn write_patch(spec: Pro3, mode: Mode, offset: u16, data: &[u8]) -> Result<()> {
    check_patch(data)?;
    let size = len_u16(data.len())?;
    let mut session = open(spec, mode)?;
    let packet = build_write_packet(offset, data, session.params.payload_offset);
    for i in 0..PATCH_PACKETS {
        exchange(&mut session, &packet, |r| validate_write_response(r, offset, size))
            .map_err(|e| chunk_failure(i, PATCH_PACKETS, "patch packet", &e))?;
    }
    Ok(())
}

/// Opens a session, sends one simple command and checks its echo.
fn command(spec: Pro3, mode: Mode, packet: &[u8; PACKET_LEN], echo: u8, what: &str) -> Result<()> {
    let mut session = open(spec, mode)?;
    exchange(&mut session, packet, |r| validate_command_response(r, echo)).map_err(|e| {
        if matches!(e, Error::Decode(_)) {
            rejected(what, &e)
        } else {
            e
        }
    })
}

pub(super) fn send_slot_select(spec: Pro3, mode: Mode) -> Result<()> {
    let packet = build_slot_select(spec.slot_select_value(mode));
    command(spec, mode, &packet, CMD_SLOT_SELECT, "slot select")
}

pub(super) fn send_apply(spec: Pro3, mode: Mode) -> Result<()> {
    command(spec, mode, &build_apply(), CMD_APPLY, "apply")
}

pub(super) fn query_status(spec: Pro3, mode: Mode) -> Result<()> {
    command(spec, mode, &build_query_status(), CMD_QUERY_STATUS, "query status")
}

/// Wire profile slot: 0-based.
const fn profile_slot0(profile_slot: Slot) -> u8 {
    profile_slot.get().saturating_sub(1)
}

fn erase_in_session(
    session: &mut Session,
    spec: Pro3,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
) -> Result<()> {
    let packet = build_erase_macro(
        MACRO_CMD_MODE,
        profile_slot0(profile_slot),
        spec.macro_gamepad_mode(mode),
        macro_slot.get(),
    );
    exchange(session, &packet, validate_erase_response).map_err(|e| {
        if matches!(e, Error::Decode(_)) {
            rejected("macro erase", &e)
        } else {
            e
        }
    })
}

pub(super) fn erase_macro(
    spec: Pro3,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
) -> Result<()> {
    let mut session = open(spec, mode)?;
    erase_in_session(&mut session, spec, mode, profile_slot, macro_slot)
}

pub(super) fn write_macro_stream(
    spec: Pro3,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
    stream: &[u8],
) -> Result<()> {
    check_macro_stream(stream, macro_slot)?;
    let total_len = macro_total_len(stream.len(), macro_slot)?;
    let base = macro_flash_base(macro_slot)?;
    let gamepad_mode = spec.macro_gamepad_mode(mode);
    let mut session = open(spec, mode)?;
    erase_in_session(&mut session, spec, mode, profile_slot, macro_slot)?;

    let total = stream.len() / MACRO_CHUNK_LEN;
    for (i, data) in stream.chunks(MACRO_CHUNK_LEN).enumerate() {
        let send = |session: &mut Session| -> Result<()> {
            let offset = len_u16(i * MACRO_CHUNK_LEN)?
                .checked_add(base)
                .ok_or_else(|| Error::write("macro chunk offset overflows 16 bits"))?;
            let packet = build_write_macro(
                MACRO_CMD_MODE,
                profile_slot0(profile_slot),
                gamepad_mode,
                data,
                offset,
                total_len,
            );
            exchange(session, &packet, validate_write_macro_response)
        };
        send(&mut session).map_err(|e| chunk_failure(i, total, "macro chunk", &e))?;
    }
    Ok(())
}
