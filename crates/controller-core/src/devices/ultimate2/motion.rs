//! The motion settings of each Ultimate 2 slot: the stick the gyro moves, the button
//! that turns it on and how, and how strongly it moves.
//!
//! A value the profile leaves as read keeps its bytes. A slot the vendor app cleared
//! holds `0` for the method and the sensitivity, which reads as `hold` and 1 but is
//! written back as `0`.

use std::ops::RangeInclusive;

use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::model::Slot;
use crate::protocol::bytes::{put_slice, put_u32_le, read_u32_le, read_u8};

/// The settings group in a profile.
pub(super) const GROUP: &str = "motion";

/// Offset of slot 1's record. The record of each next slot follows [`STRIDE`] bytes on.
const FIRST: usize = 0x0494;
/// Bytes between two slots' records, the length of one.
const STRIDE: usize = 12;
/// Offset of the record's flag, a `u32`.
const FLAG: usize = 0;
/// Offset of the button that turns the gyro on, a `u32` code.
const BUTTON: usize = 4;
/// Offset of the method byte.
const METHOD: usize = 8;
/// Offset of the sensitivity byte.
const SENSITIVITY: usize = 9;
/// Offset of the dead zone byte, in percent.
const DEAD_ZONE: usize = 10;
/// Offset of the byte that names the stick the gyro moves.
const MAP_TO: usize = 11;

/// The flag of a record whose gyro moves a stick, as the vendor app writes it.
const FLAG_ON: u32 = 0x2020_0911;
/// The flag of a record whose gyro is off.
const FLAG_OFF: u32 = 0x2019_0000;

/// The sensitivity the vendor app offers. It clamps a stored value into this range.
const SENSITIVITY_RANGE: RangeInclusive<u8> = 1..=10;
/// The largest dead zone, in percent.
const DEAD_ZONE_MAX: u8 = 100;

/// The stick the gyro moves: the stored byte and the profile's id. Any other byte
/// reads as `off`.
const MAP_TO_IDS: [(u8, &str); 3] = [(0, "off"), (1, "right_stick"), (2, "left_stick")];
/// How the button turns the gyro on. Any other byte, such as the `0` of a cleared
/// slot, reads as `hold`.
const METHODS: [(u8, &str); 2] = [(1, "hold"), (2, "toggle")];
/// The buttons that may turn the gyro on: the vendor app's code, which names a face by
/// its printed letter, and the button id. `0`, and any code not listed, reads as `none`.
const BUTTONS: [(u32, &str); 17] = [
    (0, "none"),
    (0x2000, "bottom face"),
    (0x1000, "right face"),
    (0x0010, "left face"),
    (0x0020, "top face"),
    (0x0200, "d-pad up"),
    (0x0100, "d-pad down"),
    (0x0080, "d-pad left"),
    (0x0040, "d-pad right"),
    (0x0400, "l1"),
    (0x0800, "r1"),
    (0x4000, "l2"),
    (0x8000, "r2"),
    (0x0002, "l3"),
    (0x0004, "r3"),
    (0x0200_0000, "rp"),
    (0x0400_0000, "lp"),
];

/// The keys of the group.
const KEYS: [&str; 5] = ["map_to", "button", "method", "sensitivity", "dead_zone"];

/// Offset of `slot`'s record.
fn record(slot: Slot) -> usize {
    FIRST + (usize::from(slot.get()) - 1) * STRIDE
}

/// The id `table` gives `code`, or `fallback`.
fn id_of<T: PartialEq>(
    table: &[(T, &'static str)],
    code: &T,
    fallback: &'static str,
) -> &'static str {
    table.iter().find(|(c, _)| c == code).map_or(fallback, |(_, id)| id)
}

/// The code `table` gives the id at `key` of `group`.
///
/// # Errors
/// Returns [`Error::Validation`] if the value is no id of `table`.
fn code_of<T: Copy>(table: &[(T, &str)], group: &Value, key: &str) -> Result<T> {
    let id = group.get(key).and_then(Value::as_str);
    table
        .iter()
        .find(|(_, i)| Some(*i) == id)
        .map(|(c, _)| *c)
        .ok_or_else(|| Error::Validation(format!("{GROUP}/{key} names no choice it has")))
}

/// The byte at `key` of `group`, if it is a whole number in `range`.
///
/// # Errors
/// Returns [`Error::Validation`] if it is not.
fn byte(group: &Value, key: &str, range: &RangeInclusive<u8>) -> Result<u8> {
    group
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|v| u8::try_from(v).ok())
        .filter(|v| range.contains(v))
        .ok_or_else(|| {
            let (lo, hi) = (range.start(), range.end());
            Error::Validation(format!("{GROUP}/{key} must be {lo} to {hi}"))
        })
}

/// The motion settings group of `slot` in `image`.
///
/// # Errors
/// Returns [`Error::Decode`] if `image` is too short to hold the record.
pub(super) fn read(image: &[u8], slot: Slot) -> Result<Value> {
    let at = record(slot);
    let (lo, hi) = (*SENSITIVITY_RANGE.start(), *SENSITIVITY_RANGE.end());
    Ok(json!({
        "map_to": id_of(&MAP_TO_IDS, &read_u8(image, at + MAP_TO)?, "off"),
        "button": id_of(&BUTTONS, &read_u32_le(image, at + BUTTON)?, "none"),
        "method": id_of(&METHODS, &read_u8(image, at + METHOD)?, "hold"),
        "sensitivity": read_u8(image, at + SENSITIVITY)?.clamp(lo, hi),
        "dead_zone": read_u8(image, at + DEAD_ZONE)?.min(DEAD_ZONE_MAX),
    }))
}

/// Writes `group` into `slot`'s record of `image`. Each value that differs from what
/// the record reads as is written, and then the flag, which is on when the gyro moves
/// a stick. A record that already reads as `group` is not touched.
///
/// # Errors
/// Returns [`Error::Validation`] if a changed value is missing or out of range, or
/// [`Error::Decode`] if `image` is too short to hold the record.
pub(super) fn write(image: &mut [u8], slot: Slot, group: &Value) -> Result<()> {
    let held = read(image, slot)?;
    let changed = |key: &str| group.get(key) != held.get(key);
    let at = record(slot);
    if changed("map_to") {
        put_slice(image, at + MAP_TO, &[code_of(&MAP_TO_IDS, group, "map_to")?])?;
    }
    if changed("button") {
        put_u32_le(image, at + BUTTON, code_of(&BUTTONS, group, "button")?)?;
    }
    if changed("method") {
        put_slice(image, at + METHOD, &[code_of(&METHODS, group, "method")?])?;
    }
    if changed("sensitivity") {
        put_slice(image, at + SENSITIVITY, &[byte(group, "sensitivity", &SENSITIVITY_RANGE)?])?;
    }
    if changed("dead_zone") {
        put_slice(image, at + DEAD_ZONE, &[byte(group, "dead_zone", &(0..=DEAD_ZONE_MAX))?])?;
    }
    if KEYS.iter().any(|key| changed(key)) {
        let on = code_of(&MAP_TO_IDS, group, "map_to")? != 0;
        put_u32_le(image, at + FLAG, if on { FLAG_ON } else { FLAG_OFF })?;
    }
    Ok(())
}

/// The group of a new slot, as the vendor app makes one: the gyro off, sensitivity 10
/// and a 40% dead zone.
pub(super) fn default() -> Value {
    json!({
        "map_to": "off",
        "button": "none",
        "method": "hold",
        "sensitivity": 10,
        "dead_zone": 40,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Bank read of a real Ultimate 2 in `XInput`. Slot 1's gyro is off.
    const XINPUT_BLOB: &[u8] = include_bytes!("../../../../../fixtures/ultimate-2/xinput.blob");

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    #[test]
    fn the_capture_reads_with_the_gyro_off() {
        assert_eq!(read(XINPUT_BLOB, slot(1)).unwrap(), default());
    }

    #[test]
    fn a_stick_on_l2_writes_the_vendor_bytes() {
        let mut image = XINPUT_BLOB.to_vec();
        let mut group = default();
        group["map_to"] = "right_stick".into();
        group["button"] = "l2".into();
        group["method"] = "toggle".into();
        group["dead_zone"] = 25.into();
        write(&mut image, slot(1), &group).unwrap();
        let at = record(slot(1));
        assert_eq!(image[at..at + STRIDE], [0x11, 0x09, 0x20, 0x20, 0, 0x40, 0, 0, 2, 10, 25, 1]);
        assert_eq!(read(&image, slot(1)).unwrap(), group);
        // Off again: the flag goes back to off.
        write(&mut image, slot(1), &default()).unwrap();
        assert_eq!(image[at..at + 4], [0, 0, 0x19, 0x20]);
    }

    #[test]
    fn a_cleared_record_written_as_read_keeps_its_bytes() {
        let mut image = XINPUT_BLOB.to_vec();
        let at = record(slot(2));
        image[at..at + STRIDE].fill(0);
        let held = read(&image, slot(2)).unwrap();
        assert_eq!(held["sensitivity"], 1, "clamped as the vendor app does");
        write(&mut image, slot(2), &held).unwrap();
        assert_eq!(image, {
            let mut want = XINPUT_BLOB.to_vec();
            want[at..at + STRIDE].fill(0);
            want
        });
    }

    #[test]
    fn a_value_out_of_range_or_unknown_is_refused() {
        let mut image = XINPUT_BLOB.to_vec();
        for (key, value) in [("sensitivity", json!(11)), ("button", json!("home/guide"))] {
            let mut group = default();
            group[key] = value;
            assert!(
                matches!(write(&mut image, slot(1), &group), Err(Error::Validation(_))),
                "{key}"
            );
        }
    }
}
