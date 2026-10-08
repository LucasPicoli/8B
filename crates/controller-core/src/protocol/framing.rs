//! Per-current-mode framing of config packets, and matching a reply to its request.
//!
//! Packet builders produce the normal layout. [`Framing::request`] turns it into the
//! bytes for the wire, and [`Framing::reply`] turns a report read back into the normal
//! layout, or drops it when it is not the reply to the request.

use crate::protocol::wire_write::PACKET_LEN;

/// Report prefix of a wrapped request (Nintendo output report `0x01`).
const WRAPPED_OUT: [u8; 3] = [0x01, 0x66, 0xAA];
/// Report prefix of a wrapped reply (Nintendo input report `0x81`).
const WRAPPED_IN: [u8; 3] = [0x81, 0x66, 0xA5];
/// First two bytes of every normal reply. The wrapped reply leaves them out.
const REPLY_HEAD: [u8; 2] = [0x02, 0x04];
/// Request byte that carries the command.
const REQ_CMD: usize = 2;
/// Request bytes that carry the chunk length, little-endian.
const REQ_CHUNK_LEN: usize = 6;
/// Bytes a length-framed request counts before its data: the `04` and the 16-byte
/// request header.
const LENGTH_HEADER: usize = 17;
/// Reply byte that echoes the request's command.
const REPLY_CMD_ECHO: usize = 4;

/// How config packets are framed on the wire in one current mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Framing {
    /// Packets go out as built: report `0x81` out, report `0x02` in.
    Plain,
    /// Nintendo-id framing: request `01 66 AA` + normal\[1..\] (normal byte k at k+2),
    /// reply `81 66 A5` + normal\[2..\] (normal byte k at k+1).
    Wrapped,
    /// Length-byte framing of older 8BitDo pads such as the Pro 2. The request is
    /// `81 <len>` followed by normal\[1..\] (normal byte k at k+1), where `len` is 17
    /// plus the chunk length. Replies are plain.
    Length,
}

impl Framing {
    /// Frames a normal-layout request for the wire.
    ///
    /// A wrapped request loses the last two normal bytes, so a 45-byte payload at 18
    /// keeps 44 bytes. The CRC stays the one taken over 45; the device accepts it.
    #[must_use]
    pub fn request(self, normal: &[u8; PACKET_LEN]) -> [u8; PACKET_LEN] {
        let prefixed = |head: &[u8]| {
            let mut wire = [0u8; PACKET_LEN];
            for (dst, b) in wire.iter_mut().zip(head.iter().chain(normal.iter().skip(1))) {
                *dst = *b;
            }
            wire
        };
        match self {
            Self::Plain => *normal,
            Self::Wrapped => prefixed(&WRAPPED_OUT),
            Self::Length => {
                let chunk = normal.get(REQ_CHUNK_LEN).copied().unwrap_or(0);
                let len = (LENGTH_HEADER + usize::from(chunk)).min(PACKET_LEN - 2);
                prefixed(&[normal[0], u8::try_from(len).unwrap_or(u8::MAX)])
            }
        }
    }

    /// Returns `raw` in normal layout (64 bytes) if it is the reply to `request`.
    ///
    /// Returns `None` for any other report: gamepad input, or a config reply whose
    /// command echo does not match.
    #[must_use]
    pub fn reply(self, raw: &[u8], request: &[u8; PACKET_LEN]) -> Option<Vec<u8>> {
        let mut normal = match self {
            Self::Plain | Self::Length => raw.starts_with(&REPLY_HEAD).then(|| raw.to_vec())?,
            Self::Wrapped => {
                let rest = raw.strip_prefix(&WRAPPED_IN)?;
                REPLY_HEAD.iter().chain(rest).copied().collect()
            }
        };
        if normal.get(REPLY_CMD_ECHO) != request.get(REQ_CMD) {
            return None;
        }
        normal.resize(PACKET_LEN, 0);
        Some(normal)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::protocol::wire::{build_start_config, build_upload_packet};
    use crate::protocol::wire_write::{build_apply, build_write_packet};

    #[test]
    fn wrapped_request_shifts_normal_bytes_by_two() {
        let normal = build_upload_packet(0, &[0xCC; 45], 0x092C);
        let wire = Framing::Wrapped.request(&normal);
        assert_eq!(&wire[..6], &[0x01, 0x66, 0xAA, 0x04, 0x02, 0x00]);
        // CRC at normal 8..10 lands at 10..12; payload at 18 lands at 20, cut to 44.
        assert_eq!(&wire[10..12], &normal[8..10]);
        assert_eq!(&wire[20..], &normal[18..62]);
    }

    /// The Pro 2 read and apply packets of TheJayMann/8bitdo-spec, which reads and
    /// writes a real Pro 2 (`Pro2/diReadPro2.sh`, `Pro2/diWritePro2.sh`).
    #[test]
    fn length_request_inserts_the_length_byte() {
        // Read 45 bytes at offset 45 of a 1652-byte blob: `81 3e 04 02 00 00 00 2d 00`,
        // then the CRC, then `74 06 00 00 2d 00`.
        let normal = build_upload_packet(45, &[0xCC; 45], 1652);
        let wire = Framing::Length.request(&normal);
        assert_eq!(&wire[..9], &[0x81, 0x3E, 0x04, 0x02, 0x00, 0x00, 0x00, 0x2D, 0x00]);
        assert_eq!(&wire[11..17], &[0x74, 0x06, 0x00, 0x00, 0x2D, 0x00]);
        assert_eq!(&wire[19..], &normal[18..63], "the payload moves one byte");
        // Apply has no chunk, so its length is 17: `81 11 04 06`.
        let apply = Framing::Length.request(&build_apply());
        assert_eq!(&apply[..4], &[0x81, 0x11, 0x04, 0x06]);
    }

    #[test]
    fn length_replies_are_plain() {
        // The write ack the Pro 2 script waits for: `02 04 04 00 01 00`.
        let request = build_write_packet(0, &[0; 45], 1652);
        let ack = [0x02, 0x04, 0x04, 0x00, 0x01, 0x00, 0x2D, 0x00];
        assert!(Framing::Length.reply(&ack, &request).is_some());
        assert!(Framing::Length.reply(&[0x81, 0x3E, 0x04, 0x01], &request).is_none());
    }

    #[test]
    fn plain_request_is_unchanged() {
        let normal = build_start_config();
        assert_eq!(Framing::Plain.request(&normal), normal);
    }

    #[test]
    fn wrapped_reply_is_unwrapped_to_normal_layout() {
        // START_CONFIG reply captured wrapped from 057e:2009: model id at 23..24.
        let mut raw = vec![0u8; 64];
        raw[..8].copy_from_slice(&[0x81, 0x66, 0xA5, 0x04, 0x00, 0x00, 0x01, 0x06]);
        raw[23..25].copy_from_slice(&[0x09, 0x60]);
        let normal = Framing::Wrapped.reply(&raw, &build_start_config()).unwrap();
        assert_eq!(normal.len(), PACKET_LEN);
        assert_eq!(&normal[..6], &[0x02, 0x04, 0x04, 0x00, 0x00, 0x01]);
        assert_eq!(&normal[22..24], &[0x09, 0x60]);
    }

    #[test]
    fn gamepad_reports_and_other_commands_are_skipped() {
        let start = build_start_config();
        // Switch input report 0x30, DInput input report 0x04.
        assert!(Framing::Wrapped.reply(&[0x30, 0x01, 0x02], &start).is_none());
        assert!(Framing::Plain.reply(&[0x04, 0x00, 0x7F, 0x80], &start).is_none());
        // A slot-select ack (echo 0x14) is not the START_CONFIG reply.
        let ack = [0x02, 0x04, 0x04, 0x00, 0x14, 0x00];
        assert!(Framing::Plain.reply(&ack, &start).is_none());
        let mut ok = ack;
        ok[4] = 0x00;
        assert!(Framing::Plain.reply(&ok, &start).is_some());
    }
}
