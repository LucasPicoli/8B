//! Write operations of the real hidraw transport.
//!
//! Each function opens its own session, builds packets with
//! [`crate::protocol::wire_write`], and maps failures to categories: transfer failure on
//! a simple command is [`Error::Timeout`], a rejected response is [`Error::Write`].

use std::time::Duration;

use crate::error::{Error, Result};
use crate::model::{MacroSlot, Mode, Slot};
use crate::protocol::bytes::take;
use crate::protocol::wire::{build_query_status, build_slot_select};
use crate::protocol::wire_write::{
    build_apply, build_erase_macro, build_write_macro, build_write_packet,
    validate_command_response, validate_erase_response, validate_write_macro_response,
    validate_write_response, MACRO_CHUNK_LEN, PACKET_LEN,
};
use crate::transport::session::{wait_for_mode, Session, Target, WRITE_TIMEOUT};
use crate::transport::write_input::{
    check_macro_stream, check_patch, check_profile_blob, macro_flash_base, macro_total_len,
    plan_profile_chunks, PATCH_PACKETS,
};

/// Byte 3 of the macro commands, always 1 in the captures.
const MACRO_CMD_MODE: u8 = 0x01;
const CMD_SLOT_SELECT: u8 = 0x14;
const CMD_APPLY: u8 = 0x06;
const CMD_QUERY_STATUS: u8 = 0x07;

/// Budget for the controller to come back on USB after a flip or close. The Pro 3
/// took 1.2 s and 1.4 s.
const REENUMERATE_BUDGET: Duration = Duration::from_secs(5);
/// Wait after the controller is back in its slide-switch mode, before the next session.
/// The kernel driver may probe the returning id first: hid-nintendo finished with
/// `057e:2009` 0.55 s after its node appeared, and a session sent 0.2 s after the
/// node appeared got no reply.
const RETURN_SETTLE: Duration = Duration::from_secs(2);

/// Opens a write session. Refuses in a current mode that takes no writes in place:
/// [`begin_write`] must flip the controller first.
fn open(to: Target<'_>) -> Result<Session> {
    let session = Session::open(to, WRITE_TIMEOUT)?;
    if let Some(via) = session.write_via {
        return Err(Error::write(format!(
            "the controller takes no writes in {} mode; flip it to {via} first",
            session.current_mode
        )));
    }
    Ok(session)
}

/// Flips the controller to the mode its config port names in `write_via`, if any,
/// and waits for it. Returns the mode to send it back to.
pub(super) fn begin_write(to: Target<'_>) -> Result<Option<Mode>> {
    let session = Session::open(to, WRITE_TIMEOUT)?;
    let Some(via) = session.write_via else {
        return Ok(None);
    };
    let back_to = session.current_mode;
    let flip = session.model()?.mode_flip_command(via).ok_or_else(|| {
        Error::write(format!("this controller cannot flip from {back_to} to {via} mode"))
    })?;
    session.send_last(&flip)?;
    wait_for_mode(to, via, REENUMERATE_BUDGET).map_err(|e| {
        Error::write(format!(
            "the controller did not come back in {via} mode ({e}); unplug it and plug it back in"
        ))
    })?;
    Ok(Some(back_to))
}

/// Sends the close command, then waits for the controller to come back in `back_to`.
pub(super) fn end_write(to: Target<'_>, back_to: Mode) -> Result<()> {
    let session = Session::open(to, WRITE_TIMEOUT)?;
    let close = session.model()?.mode_close_command().ok_or_else(|| {
        Error::write(format!(
            "this controller cannot go back to {back_to} mode; unplug it and plug it back in"
        ))
    })?;
    session.send_last(&close)?;
    wait_for_mode(to, back_to, REENUMERATE_BUDGET)?;
    std::thread::sleep(RETURN_SETTLE);
    Ok(())
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
    validate(&resp).inspect_err(|e| session.log_failure(packet, e))
}

pub(super) fn write_full_profile(to: Target<'_>, blob: &[u8]) -> Result<()> {
    let mut session = open(to)?;
    let blob_size = session.model()?.blob_size();
    check_profile_blob(blob, blob_size)?;
    let total_len = len_u16(blob_size)?;
    let chunks = plan_profile_chunks(blob_size);
    let total = chunks.len();
    for (i, &(offset, size)) in chunks.iter().enumerate() {
        let send = |session: &mut Session| -> Result<()> {
            let data = take(blob, usize::from(offset), size)?;
            let packet = build_write_packet(offset, data, total_len);
            let size16 = len_u16(size)?;
            exchange(session, &packet, |r| validate_write_response(r, offset, size16))
        };
        send(&mut session).map_err(|e| chunk_failure(i, total, "profile chunk", &e))?;
    }
    Ok(())
}

pub(super) fn write_patch(to: Target<'_>, offset: u16, data: &[u8]) -> Result<()> {
    check_patch(data)?;
    let size = len_u16(data.len())?;
    let mut session = open(to)?;
    let packet = build_write_packet(offset, data, len_u16(session.model()?.blob_size())?);
    for i in 0..PATCH_PACKETS {
        exchange(&mut session, &packet, |r| validate_write_response(r, offset, size))
            .map_err(|e| chunk_failure(i, PATCH_PACKETS, "patch packet", &e))?;
    }
    Ok(())
}

/// Opens a session, sends one simple command and checks its echo.
fn command(to: Target<'_>, packet: &[u8; PACKET_LEN], echo: u8, what: &str) -> Result<()> {
    command_with(to, |_| Ok(*packet), echo, what)
}

/// [`command`] with a packet built from the open session, for a command whose bytes
/// depend on the model.
fn command_with(
    to: Target<'_>,
    build: impl FnOnce(&Session) -> Result<[u8; PACKET_LEN]>,
    echo: u8,
    what: &str,
) -> Result<()> {
    let mut session = open(to)?;
    let packet = build(&session)?;
    exchange(&mut session, &packet, |r| validate_command_response(r, echo)).map_err(|e| {
        if matches!(e, Error::Decode(_)) {
            rejected(what, &e)
        } else {
            e
        }
    })
}

pub(super) fn send_slot_select(to: Target<'_>, mode: Mode) -> Result<()> {
    let build = |s: &Session| Ok(build_slot_select(s.model()?.slot_select_value(mode)?));
    command_with(to, build, CMD_SLOT_SELECT, "slot select")
}

pub(super) fn send_apply(to: Target<'_>) -> Result<()> {
    command(to, &build_apply(), CMD_APPLY, "apply")
}

pub(super) fn query_status(to: Target<'_>) -> Result<()> {
    command(to, &build_query_status(), CMD_QUERY_STATUS, "query status")
}

/// Wire profile slot: 0-based.
const fn profile_slot0(profile_slot: Slot) -> u8 {
    profile_slot.get().saturating_sub(1)
}

fn erase_in_session(
    session: &mut Session,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
) -> Result<()> {
    let packet = build_erase_macro(
        MACRO_CMD_MODE,
        profile_slot0(profile_slot),
        session.model()?.macro_gamepad_mode(mode)?,
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
    to: Target<'_>,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
) -> Result<()> {
    let mut session = open(to)?;
    erase_in_session(&mut session, mode, profile_slot, macro_slot)
}

pub(super) fn write_macro_stream(
    to: Target<'_>,
    mode: Mode,
    profile_slot: Slot,
    macro_slot: MacroSlot,
    stream: &[u8],
) -> Result<()> {
    check_macro_stream(stream, macro_slot)?;
    let total_len = macro_total_len(stream.len(), macro_slot)?;
    let base = macro_flash_base(macro_slot)?;
    let mut session = open(to)?;
    let gamepad_mode = session.model()?.macro_gamepad_mode(mode)?;
    erase_in_session(&mut session, mode, profile_slot, macro_slot)?;

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
