//! The light settings of each Ultimate 2 slot: the colours of its three effects.
//!
//! Light-tracing has a colour and a background, Fire Ring a colour, a background and a
//! speed, and Custom one colour per ring segment, 12 on the left ring then 12 on the
//! right. Which effect plays is not stored in the slot. A value the profile leaves as
//! read keeps its bytes.

use std::ops::RangeInclusive;

use serde_json::{json, Value};

use crate::devices::pro3::tables::SLOT_MARKER;
use crate::error::{Error, Result};
use crate::model::{colour, Slot};
use crate::protocol::bytes::{put_slice, put_u32_le, read_u32_le, read_u8};

/// The settings group in a profile.
pub(super) const GROUP: &str = "lights";

/// Offset of slot 1's Light-tracing record, and the bytes between two slots' records.
const TRACING: (usize, usize) = (0x04B8, 12);
/// Offset of slot 1's Fire Ring record, and the bytes between two slots' records.
const FIRE: (usize, usize) = (0x04DC, 16);
/// Offset of slot 1's Custom record, and the bytes between two slots' records.
const CUSTOM: (usize, usize) = (0x050C, 100);
/// Offset of a record's first colour, a `u32` `0x00RRGGBB`, after its flag.
const COLOUR: usize = 4;
/// Offset of the background colour in the Light-tracing and Fire Ring records.
const BACKGROUND: usize = 8;
/// Offset of the Fire Ring speed byte.
const SPEED: usize = 12;
/// Bytes per colour.
const COLOUR_LEN: usize = 4;
/// Ring segments in the Custom record: 12 per ring.
const SEGMENTS: usize = 24;
/// The Fire Ring speeds the vendor app offers.
const SPEED_RANGE: RangeInclusive<u8> = 1..=15;

/// Offset of the record at `(first, stride)` for `slot`.
fn record((first, stride): (usize, usize), slot: Slot) -> usize {
    first + (usize::from(slot.get()) - 1) * stride
}

/// The colour at `at` of `image`, as the profile writes it.
fn colour_at(image: &[u8], at: usize) -> Result<Value> {
    Ok(colour::format(read_u32_le(image, at)?).into())
}

/// The light settings group of `slot` in `image`.
///
/// # Errors
/// Returns [`Error::Decode`] if `image` is too short to hold the records.
pub(super) fn read(image: &[u8], slot: Slot) -> Result<Value> {
    let (tracing, fire, custom) = (record(TRACING, slot), record(FIRE, slot), record(CUSTOM, slot));
    let (lo, hi) = (*SPEED_RANGE.start(), *SPEED_RANGE.end());
    let segments = (0..SEGMENTS)
        .map(|i| colour_at(image, custom + COLOUR + i * COLOUR_LEN))
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "tracing": colour_at(image, tracing + COLOUR)?,
        "tracing_background": colour_at(image, tracing + BACKGROUND)?,
        "fire": colour_at(image, fire + COLOUR)?,
        "fire_background": colour_at(image, fire + BACKGROUND)?,
        "fire_speed": read_u8(image, fire + SPEED)?.clamp(lo, hi),
        "custom": segments,
    }))
}

/// One record's part of a write: the record, and whether it changed.
struct Record<'a> {
    image: &'a mut [u8],
    at: usize,
    changed: bool,
}

impl Record<'_> {
    /// Writes the colour `want` at `offset` when it differs from `held`.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `want` is no colour, or [`Error::Decode`] if the
    /// image is short.
    fn colour(
        &mut self,
        offset: usize,
        want: Option<&Value>,
        held: Option<&Value>,
        key: &str,
    ) -> Result<()> {
        let parse = |v: Option<&Value>| v.and_then(Value::as_str).and_then(colour::parse);
        let code = parse(want)
            .ok_or_else(|| Error::Validation(format!("{GROUP}/{key} must be a colour, #rrggbb")))?;
        if Some(code) != parse(held) {
            put_u32_le(self.image, self.at + offset, code)?;
            self.changed = true;
        }
        Ok(())
    }

    /// Marks the record in use when a value of it changed, as the vendor app does.
    ///
    /// # Errors
    /// Returns [`Error::Decode`] if the image is short.
    fn finish(self) -> Result<()> {
        if self.changed {
            put_slice(self.image, self.at, &SLOT_MARKER)?;
        }
        Ok(())
    }
}

/// Writes `group` into `slot`'s records of `image`. Each value that differs from what
/// the records read as is written, and a record with a new value is marked in use. A
/// record that already reads as `group` is not touched.
///
/// # Errors
/// Returns [`Error::Validation`] if a value is missing or no colour, the speed is out of
/// range or Custom has not 24 colours, or [`Error::Decode`] if `image` is too short.
pub(super) fn write(image: &mut [u8], slot: Slot, group: &Value) -> Result<()> {
    let held = read(image, slot)?;
    let pair = |key: &str| (group.get(key), held.get(key));

    let mut tracing = Record { at: record(TRACING, slot), image: &mut *image, changed: false };
    for (offset, key) in [(COLOUR, "tracing"), (BACKGROUND, "tracing_background")] {
        let (want, had) = pair(key);
        tracing.colour(offset, want, had, key)?;
    }
    tracing.finish()?;

    let mut fire = Record { at: record(FIRE, slot), image: &mut *image, changed: false };
    for (offset, key) in [(COLOUR, "fire"), (BACKGROUND, "fire_background")] {
        let (want, had) = pair(key);
        fire.colour(offset, want, had, key)?;
    }
    let (want, had) = pair("fire_speed");
    if want != had {
        let speed = want.and_then(Value::as_u64).and_then(|v| u8::try_from(v).ok());
        let speed = speed.filter(|v| SPEED_RANGE.contains(v)).ok_or_else(|| {
            let (lo, hi) = (SPEED_RANGE.start(), SPEED_RANGE.end());
            Error::Validation(format!("{GROUP}/fire_speed must be {lo} to {hi}"))
        })?;
        put_slice(fire.image, fire.at + SPEED, &[speed])?;
        fire.changed = true;
    }
    fire.finish()?;

    let wants = group.get("custom").and_then(Value::as_array).filter(|c| c.len() == SEGMENTS);
    let wants = wants
        .ok_or_else(|| Error::Validation(format!("{GROUP}/custom must hold {SEGMENTS} colours")))?;
    let mut custom = Record { at: record(CUSTOM, slot), image: &mut *image, changed: false };
    for (i, want) in wants.iter().enumerate() {
        let had = held.get("custom").and_then(|c| c.get(i));
        custom.colour(COLOUR + i * COLOUR_LEN, Some(want), had, &format!("custom/{i}"))?;
    }
    custom.finish()
}

/// The group of a new slot, with the vendor app's colours for a new profile.
pub(super) fn default() -> Value {
    json!({
        "tracing": "#260032",
        "tracing_background": "#084e50",
        "fire": "#cc6200",
        "fire_background": "#cc0000",
        "fire_speed": 7,
        "custom": vec!["#7e3200"; SEGMENTS],
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Bank read of a real Ultimate 2 in `XInput`. Only slot 1 has been written.
    const XINPUT_BLOB: &[u8] = include_bytes!("../../../../../fixtures/ultimate-2/xinput.blob");

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    #[test]
    fn the_capture_reads_as_the_pad_shows_it() {
        let group = read(XINPUT_BLOB, slot(1)).unwrap();
        assert_eq!(group["tracing"], "#4f101e");
        assert_eq!(group["tracing_background"], "#181000");
        assert_eq!(group["fire"], "#602e00");
        assert_eq!(group["fire_background"], "#cc0000");
        assert_eq!(group["fire_speed"], 7);
        assert_eq!(group["custom"], json!(vec!["#7e3200"; SEGMENTS]));
    }

    #[test]
    fn an_edit_writes_only_its_colour() {
        let mut image = XINPUT_BLOB.to_vec();
        let mut group = read(&image, slot(1)).unwrap();
        group["custom"][13] = "#00FF80".into();
        write(&mut image, slot(1), &group).unwrap();
        let at = record(CUSTOM, slot(1)) + COLOUR + 13 * COLOUR_LEN;
        let changed: Vec<usize> =
            (0..image.len()).filter(|&i| image[i] != XINPUT_BLOB[i]).collect();
        assert_eq!(changed, [at, at + 1, at + 2], "the top byte was 0 already");
        assert_eq!(image[at..at + 4], [0x80, 0xFF, 0x00, 0x00]);
        assert_eq!(read(&image, slot(1)).unwrap()["custom"][13], "#00ff80");
    }

    #[test]
    fn a_new_value_in_an_erased_slot_marks_its_record() {
        let mut image = XINPUT_BLOB.to_vec();
        let at = record(FIRE, slot(2));
        image[at..at + 16].fill(0);
        let mut group = read(&image, slot(2)).unwrap();
        group["fire_speed"] = 3.into();
        write(&mut image, slot(2), &group).unwrap();
        assert_eq!(
            image[at..at + 16],
            [0x11, 0x09, 0x20, 0x20, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0]
        );
    }

    #[test]
    fn a_bad_colour_speed_or_ring_is_refused() {
        let mut image = XINPUT_BLOB.to_vec();
        for (key, value) in
            [("fire", json!("red")), ("fire_speed", json!(16)), ("custom", json!(["#000000"]))]
        {
            let mut group = default();
            group[key] = value;
            assert!(
                matches!(write(&mut image, slot(1), &group), Err(Error::Validation(_))),
                "{key}"
            );
        }
    }
}
