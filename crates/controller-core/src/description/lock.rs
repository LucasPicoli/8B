//! The lock declaration: a controller that hides its config port until it gets an
//! unlock sequence, and that a relock sequence returns to its locked mode.
//!
//! The declaration holds every byte and every time, so no model needs code of its own
//! to unlock. A description without one always shows its config port.

use serde::{Deserialize, Deserializer};

use crate::device::UsbId;

/// The most bytes in one packet: a full-speed interrupt transfer, and the size of the
/// output report a relock packet is zero padded to.
pub const MAX_PACKET_LEN: usize = 64;

/// Bit 7 of a USB endpoint address: set on an IN endpoint.
const ENDPOINT_IN: u8 = 0x80;

/// How a locked controller shows, how to unlock it and how to relock it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lock {
    /// The controller while it is locked.
    pub locked: LockedPort,
    /// The sequence that shows the config port.
    pub unlock: Unlock,
    /// The sequence that returns the controller to its locked mode.
    pub relock: Relock,
}

/// A locked controller as the USB bus lists it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LockedPort {
    /// The USB id while locked.
    pub usb: UsbId,
    /// The USB product string. Another pad may share the USB id, so both must match.
    pub product: String,
}

/// The unlock sequence, sent to the locked controller's USB interface.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unlock {
    /// The USB interface to claim.
    pub interface: u8,
    /// The interrupt OUT endpoint the packets go to, such as `0x02`.
    #[serde(deserialize_with = "hex_u8")]
    pub endpoint: u8,
    /// The packets, in send order.
    pub packets: Vec<TimedPacket>,
    /// How long to wait for the config port after the last packet, in milliseconds.
    pub timeout_ms: u32,
}

/// The relock sequence, sent to the config port as output reports.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Relock {
    /// The packets, in send order, each zero padded to [`MAX_PACKET_LEN`].
    pub packets: Vec<TimedPacket>,
}

/// One packet and when to send it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimedPacket {
    /// Milliseconds after the first packet of the sequence.
    pub at_ms: u32,
    /// The bytes, written in JSON as hex without spaces, such as `"0008000100000000"`.
    #[serde(deserialize_with = "hex_bytes")]
    pub data: Vec<u8>,
}

impl Lock {
    /// The rules serde cannot express: an OUT endpoint, a timeout, a product string,
    /// and two sequences of packets that each hold bytes and run forward in time.
    pub(super) fn check(&self) -> Result<(), String> {
        if self.locked.product.is_empty() {
            return Err("lock: the locked product string must not be empty".to_owned());
        }
        if self.unlock.endpoint & ENDPOINT_IN != 0 {
            return Err(format!(
                "lock: unlock endpoint 0x{:02x} is an IN endpoint",
                self.unlock.endpoint
            ));
        }
        if self.unlock.timeout_ms == 0 {
            return Err("lock: unlock timeout_ms must be above 0".to_owned());
        }
        check_packets("unlock", &self.unlock.packets)?;
        check_packets("relock", &self.relock.packets)
    }
}

/// `packets` is not empty, each holds 1 to [`MAX_PACKET_LEN`] bytes, and no `at_ms` is
/// below the one before it.
fn check_packets(name: &str, packets: &[TimedPacket]) -> Result<(), String> {
    if packets.is_empty() {
        return Err(format!("lock: the {name} sequence needs a packet"));
    }
    if let Some(i) = packets.iter().position(|p| p.data.is_empty() || p.data.len() > MAX_PACKET_LEN)
    {
        return Err(format!("lock: {name} packet {i} needs 1 to {MAX_PACKET_LEN} bytes"));
    }
    let late = |w: &[TimedPacket]| matches!(w, [a, b] if a.at_ms > b.at_ms);
    if let Some(i) = packets.windows(2).position(late) {
        return Err(format!("lock: {name} packet {} is sent before the one above it", i + 1));
    }
    Ok(())
}

/// Deserializes `"0x02"` into a `u8`.
fn hex_u8<'de, D: Deserializer<'de>>(d: D) -> Result<u8, D::Error> {
    let s = String::deserialize(d)?;
    s.strip_prefix("0x")
        .filter(|h| h.len() == 2)
        .and_then(|h| u8::from_str_radix(h, 16).ok())
        .ok_or_else(|| {
            serde::de::Error::custom(format!("expected a byte like \"0x02\", got \"{s}\""))
        })
}

/// Deserializes `"0008ff"` into bytes. An odd length or a non-hex digit fails.
fn hex_bytes<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    let s = String::deserialize(d)?;
    let bad =
        || serde::de::Error::custom(format!("expected hex bytes like \"0008ff\", got \"{s}\""));
    if s.len() % 2 != 0 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| s.get(i..i + 2).and_then(|h| u8::from_str_radix(h, 16).ok()).ok_or_else(bad))
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
pub(super) mod tests {
    use serde_json::{json, Value};

    use super::*;
    use crate::description::tests::{minimal, parse};

    pub(in crate::description) fn a_lock() -> Value {
        json!({
            "locked": { "usb": { "vendor": "0x2dc8", "product": "0x3106" },
                        "product": "8BitDo Ultimate Controller" },
            "unlock": {
                "interface": 0, "endpoint": "0x02", "timeout_ms": 3000,
                "packets": [ { "at_ms": 0, "data": "0008000100000000" },
                             { "at_ms": 16, "data": "0008000001000000" },
                             { "at_ms": 432, "data": "0008000100000000" } ]
            },
            "relock": {
                "packets": [ { "at_ms": 0, "data": "811104070001" },
                             { "at_ms": 8, "data": "8105005102" } ]
            }
        })
    }

    fn with_lock(lock: Value) -> Value {
        let mut v = minimal();
        v["lock"] = lock;
        v
    }

    #[test]
    fn a_lock_reads_its_ids_bytes_and_times() {
        let d = parse(&with_lock(a_lock())).unwrap();
        let lock = d.lock.unwrap();
        assert_eq!((lock.locked.usb.vendor, lock.locked.usb.product), (0x2DC8, 0x3106));
        assert_eq!(lock.locked.product, "8BitDo Ultimate Controller");
        assert_eq!((lock.unlock.interface, lock.unlock.endpoint), (0, 0x02));
        assert_eq!(lock.unlock.timeout_ms, 3000);
        assert_eq!(lock.unlock.packets[1].at_ms, 16);
        assert_eq!(lock.unlock.packets[1].data, [0x00, 0x08, 0x00, 0x00, 0x01, 0, 0, 0]);
        assert_eq!(lock.relock.packets[1].data, [0x81, 0x05, 0x00, 0x51, 0x02]);
        assert!(parse(&minimal()).unwrap().lock.is_none(), "no lock: always unlocked");
    }

    #[test]
    fn the_loader_refuses_a_broken_lock() {
        let cases: [fn(&mut Value); 10] = [
            |l| l["unlock"]["packets"][0]["data"] = json!("0008000"),
            |l| l["unlock"]["packets"][0]["data"] = json!("00 08"),
            |l| l["unlock"]["packets"][0]["data"] = json!("zz"),
            |l| l["unlock"]["packets"][0]["data"] = json!(""),
            |l| l["unlock"]["packets"][0]["data"] = json!("00".repeat(MAX_PACKET_LEN + 1)),
            |l| l["unlock"]["packets"][2]["at_ms"] = json!(15),
            |l| l["relock"]["packets"] = json!([]),
            |l| l["unlock"]["endpoint"] = json!("0x82"),
            |l| l["unlock"]["timeout_ms"] = json!(0),
            |l| l["locked"]["product"] = json!(""),
        ];
        for (i, break_it) in cases.iter().enumerate() {
            let mut lock = a_lock();
            break_it(&mut lock);
            assert!(parse(&with_lock(lock)).is_err(), "case {i} should fail");
        }
        let mut lock = a_lock();
        lock["relock"]["packets"][1]["at_ms"] = json!(0);
        assert!(parse(&with_lock(lock)).is_ok(), "two packets may share a time");
    }
}
