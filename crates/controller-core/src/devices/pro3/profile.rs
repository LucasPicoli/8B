//! Pro 3 profile decoder — `map_profile`.
//!
//! A faithful port of `core::ProfileMapper::mapProfile` from
//! `src/core/profile_mapper.cpp`. Decodes a raw 2348-byte readback blob into a
//! [`CanonicalProfileSummary`]. Verified byte-for-byte against golden vectors
//! captured from live hardware (`tests/golden_profile_decode.rs`).
//!
//! All variable-offset reads go through the bounds-checked [`crate::protocol::bytes`]
//! accessors so the decoder is panic-free even on truncated or corrupted input.

use crate::devices::pro3::tables;
use crate::devices::pro3::tables::ButtonEncodingEntry;
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, Mode, RawProfilePayload, Sticks,
    Triggers, TriggersAnalog, TriggersSwitch, Vibration,
};
use crate::protocol::bytes::{read_u32_le, read_u8, take};
use crate::protocol::text::decode_utf16be_name;

/// Detected blob layout: the device stores blobs shifted −2 from canonical.
#[derive(Debug, Clone, Copy)]
struct DecodeLayout {
    shifted_by_two: bool,
}

impl DecodeLayout {
    /// Applies the −2 shift to a canonical offset when the blob is shifted.
    const fn adjust(self, offset: usize) -> usize {
        if self.shifted_by_two {
            offset.saturating_sub(2)
        } else {
            offset
        }
    }
}

/// Detects whether the blob uses the shifted (−2) layout by scanning the flag
/// positions 0/4/8 for the active-slot marker.
fn detect_layout(payload: &[u8]) -> DecodeLayout {
    for slot_index in 0..3 {
        let offset = slot_index * tables::FLAG_STRIDE;
        if marker_at(payload, offset) {
            return DecodeLayout { shifted_by_two: true };
        }
    }
    DecodeLayout { shifted_by_two: false }
}

/// Returns true when the 4-byte active-slot marker sits at `offset`.
fn marker_at(payload: &[u8], offset: usize) -> bool {
    take(payload, offset, tables::SLOT_MARKER.len()).is_ok_and(|s| s == tables::SLOT_MARKER)
}

/// Reads a single byte, returning 0 when out of range (mirrors C++ `readByteAt`).
fn byte_at(payload: &[u8], offset: usize) -> u8 {
    read_u8(payload, offset).unwrap_or(0)
}

/// Reads a 4-byte encoding, returning zeros when out of range
/// (mirrors C++ `readBytesAt(.., 4)`).
fn bytes4_at(payload: &[u8], offset: usize) -> [u8; 4] {
    take(payload, offset, 4).map_or(tables::NULL_ENCODING, |slice| {
        <[u8; 4]>::try_from(slice).unwrap_or(tables::NULL_ENCODING)
    })
}

/// Converts a raw byte to a clamped 0..=100 percentage using `qRound` semantics
/// (round-half-up for the non-negative values produced here).
fn to_percent(raw: u8, raw_max: i32) -> i32 {
    if raw_max <= 0 {
        return 0;
    }
    let value = (f64::from(raw) * 100.0) / f64::from(raw_max);
    // qRound(d) = floor(d + 0.5) for d >= 0; clamp into 0..=100 before the cast
    // so the result is provably representable as i32.
    let rounded = (value + 0.5).floor().clamp(0.0, 100.0);
    // Safe: `rounded` is in [0.0, 100.0] and integral.
    #[allow(clippy::cast_possible_truncation)]
    let pct = rounded as i32;
    pct
}

/// Decodes the LE16 mode field (a readback artifact at `0x0012`).
///
/// Kept as a documented parity helper for the C++ `decodeMode` fallback path;
/// the production decoder prefers `raw.mode_hint`. Exercised by unit tests.
#[cfg(test)]
fn decode_mode_field(payload: &[u8], layout: DecodeLayout) -> Option<Mode> {
    let offset = layout.adjust(tables::MODE_OFFSET);
    match crate::protocol::bytes::read_u16_le(payload, offset).ok()? {
        0 => Some(Mode::Switch),
        1 => Some(Mode::DInput),
        3 => Some(Mode::XInput),
        _ => None,
    }
}

/// Decodes the UTF-16BE profile name for `source_slot` (1-based).
fn decode_name(payload: &[u8], source_slot: u8, layout: DecodeLayout) -> String {
    if !(1..=3).contains(&source_slot) {
        return "unnamed".to_owned();
    }
    let offset = layout.adjust(tables::NAME_BASE_OFFSET)
        + (usize::from(source_slot - 1) * tables::NAME_STRIDE);
    take(payload, offset, tables::NAME_BYTES)
        .map_or_else(|_| "unnamed".to_owned(), decode_utf16be_name)
}

/// Picks the encoding table for `mode`.
const fn encodings_for_mode(mode: Mode) -> &'static [ButtonEncodingEntry] {
    match mode {
        Mode::Switch => &tables::SWITCH_ENCODINGS,
        // dinput readback table is not yet hardware-verified; fall back to XInput.
        Mode::XInput | Mode::DInput => &tables::XINPUT_ENCODINGS,
    }
}

/// Resolves the target-control name for a single button entry.
///
/// Faithful port of C++ `decodeTargetControl`.
fn decode_target_control(
    entries: &[ButtonEncodingEntry],
    source_index: usize,
    value: [u8; 4],
) -> String {
    let valid_source = source_index < entries.len();

    // Step 0: home/guide cannot be remapped — force identity.
    if source_index == tables::HOME_GUIDE_INDEX {
        if let Some(entry) = entries.get(source_index) {
            return entry.source.to_owned();
        }
    }

    // Step 1: null/disabled. Checked before identity so unmapped back paddles
    // (whose default IS null) report "disabled" rather than identity.
    if value == tables::NULL_ENCODING {
        return "disabled".to_owned();
    }

    // Step 2: identity — value matches the source's primary encoding.
    if let Some(entry) = entries.get(source_index) {
        if value == entry.encoding {
            return entry.source.to_owned();
        }
        // Step 3: variant identity — value matches an alternate face encoding.
        if entry.variant_identity_encodings.contains(&value) {
            return entry.source.to_owned();
        }
    }

    // Step 4: search the table for matching targets (primary then variant).
    let mut primary_matches: Vec<usize> = Vec::new();
    let mut variant_matches: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if entry.encoding == value {
            primary_matches.push(i);
        }
        if entry.variant_identity_encodings.contains(&value) {
            variant_matches.push(i);
        }
    }

    // Step 5: rp/lp/l4/r4 (18-21) are not valid TARGETS; filter them out.
    // Target-only entries (index >= SOURCE_BUTTON_COUNT, e.g. screenshot) stay.
    let is_invalid_target = |idx: &usize| {
        *idx >= tables::NULL_DEFAULT_FIRST_INDEX && *idx < tables::SOURCE_BUTTON_COUNT
    };
    primary_matches.retain(|idx| !is_invalid_target(idx));
    variant_matches.retain(|idx| !is_invalid_target(idx));

    // Step 7: prefer primary over variant; prefer a match that is not the source.
    let pick_best = |matches: &[usize]| -> Option<usize> {
        matches
            .iter()
            .copied()
            .find(|idx| *idx != source_index)
            .or_else(|| matches.first().copied())
    };

    let best = pick_best(&primary_matches).or_else(|| pick_best(&variant_matches));
    if let Some(idx) = best {
        if let Some(entry) = entries.get(idx) {
            return entry.source.to_owned();
        }
    }

    // Step 8: no match — fall back to identity, else right face.
    if valid_source {
        if let Some(entry) = entries.get(source_index) {
            return entry.source.to_owned();
        }
    }
    "right face".to_owned()
}

/// Decodes all 22 button mappings for `source_slot`.
fn decode_button_mappings(payload: &[u8], mode: Mode, source_slot: u8) -> Vec<ButtonMapping> {
    let entries = encodings_for_mode(mode);
    let mut mappings = Vec::new();
    if !(1..=3).contains(&source_slot) || entries.len() < tables::SOURCE_BUTTON_COUNT {
        return mappings;
    }

    // The button-map section uses the CANONICAL base regardless of layout shift.
    let slot_map_base = tables::BUTTON_MAP_BASE_OFFSET
        + (usize::from(source_slot - 1) * tables::BUTTON_MAP_SLOT_STRIDE);

    for index in 0..tables::SOURCE_BUTTON_COUNT {
        let value = bytes4_at(payload, slot_map_base + (index * tables::BUTTON_ENTRY_BYTES));
        // A bleeding slot marker corrupts the entry — report it disabled.
        let target = if value == tables::SLOT_MARKER {
            "disabled".to_owned()
        } else {
            decode_target_control(entries, index, value)
        };
        let source = entries.get(index).map_or_else(String::new, |e| e.source.to_owned());
        mappings.push(ButtonMapping { source, target });
    }
    mappings
}

/// Decodes stick ranges and inversion/swap flags for `source_slot`.
fn decode_sticks(payload: &[u8], source_slot: u8, layout: DecodeLayout) -> Sticks {
    if !(1..=3).contains(&source_slot) {
        return Sticks {
            left_min_pct: 0,
            left_max_pct: 100,
            right_min_pct: 0,
            right_max_pct: 100,
            invert_left_x: false,
            invert_left_y: false,
            invert_right_x: false,
            invert_right_y: false,
            swap_sticks: false,
            swap_dpad_with_left_stick: false,
        };
    }

    let slot = usize::from(source_slot - 1);
    let stick_off = layout.adjust(tables::STICK_DATA_OFFSET) + (slot * tables::SLOT_DATA_STRIDE);
    let left_min = byte_at(payload, stick_off);
    let left_max = byte_at(payload, stick_off + 1);
    let right_min = byte_at(payload, stick_off + 2);
    let right_max = byte_at(payload, stick_off + 3);

    let flag_off = layout.adjust(tables::SLOT_FLAGS_OFFSET) + (slot * tables::SLOT_DATA_STRIDE);
    let flags0 = byte_at(payload, flag_off);
    let flags1 = byte_at(payload, flag_off + 1);

    Sticks {
        left_min_pct: to_percent(left_min, tables::STICK_RAW_MAX)
            .clamp(tables::STICK_MIN_PCT_LO, tables::STICK_MIN_PCT_HI),
        left_max_pct: to_percent(left_max, tables::STICK_RAW_MAX)
            .clamp(tables::STICK_MAX_PCT_LO, tables::STICK_MAX_PCT_HI),
        right_min_pct: to_percent(right_min, tables::STICK_RAW_MAX)
            .clamp(tables::STICK_MIN_PCT_LO, tables::STICK_MIN_PCT_HI),
        right_max_pct: to_percent(right_max, tables::STICK_RAW_MAX)
            .clamp(tables::STICK_MAX_PCT_LO, tables::STICK_MAX_PCT_HI),
        invert_left_x: (flags0 & 0x01) != 0,
        invert_left_y: (flags0 & 0x02) != 0,
        invert_right_x: (flags0 & 0x04) != 0,
        invert_right_y: (flags0 & 0x08) != 0,
        swap_sticks: (flags0 & 0x10) != 0,
        swap_dpad_with_left_stick: (flags1 & 0x01) != 0,
    }
}

/// Decodes trigger ranges/thresholds for `source_slot`, shape depending on mode.
fn decode_triggers(payload: &[u8], mode: Mode, source_slot: u8, layout: DecodeLayout) -> Triggers {
    let is_switch = mode == Mode::Switch;
    if !(1..=3).contains(&source_slot) {
        return if is_switch {
            Triggers::Switch(TriggersSwitch {
                left_threshold_pct: 0,
                right_threshold_pct: 0,
                swap_triggers: false,
            })
        } else {
            Triggers::Analog(TriggersAnalog {
                left_min_pct: 0,
                left_max_pct: 100,
                right_min_pct: 0,
                right_max_pct: 100,
                swap_triggers: false,
            })
        };
    }

    let slot = usize::from(source_slot - 1);
    let trig_off = layout.adjust(tables::TRIGGER_DATA_OFFSET) + (slot * tables::SLOT_DATA_STRIDE);
    let left_a = byte_at(payload, trig_off);
    let left_b = byte_at(payload, trig_off + 1);
    let right_a = byte_at(payload, trig_off + 2);
    let right_b = byte_at(payload, trig_off + 3);

    let flag_off = layout.adjust(tables::SLOT_FLAGS_OFFSET) + (slot * tables::SLOT_DATA_STRIDE);
    let swap_triggers = (byte_at(payload, flag_off) & 0x80) != 0;

    if is_switch {
        return Triggers::Switch(TriggersSwitch {
            left_threshold_pct: to_percent(left_a, tables::TRIGGER_RAW_MAX)
                .clamp(tables::SWITCH_THRESHOLD_PCT_LO, tables::SWITCH_THRESHOLD_PCT_HI),
            right_threshold_pct: to_percent(right_a, tables::TRIGGER_RAW_MAX)
                .clamp(tables::SWITCH_THRESHOLD_PCT_LO, tables::SWITCH_THRESHOLD_PCT_HI),
            swap_triggers,
        });
    }

    // Auto-fix inverted analog ranges (firmware may store min > max).
    let mut left_min = to_percent(left_a, tables::TRIGGER_RAW_MAX);
    let mut left_max = to_percent(left_b, tables::TRIGGER_RAW_MAX);
    if left_min > left_max {
        std::mem::swap(&mut left_min, &mut left_max);
    }
    let mut right_min = to_percent(right_a, tables::TRIGGER_RAW_MAX);
    let mut right_max = to_percent(right_b, tables::TRIGGER_RAW_MAX);
    if right_min > right_max {
        std::mem::swap(&mut right_min, &mut right_max);
    }
    Triggers::Analog(TriggersAnalog {
        left_min_pct: left_min,
        left_max_pct: left_max,
        right_min_pct: right_min,
        right_max_pct: right_max,
        swap_triggers,
    })
}

/// Decodes the two vibration intensity levels (0..=5) for `source_slot`.
fn decode_vibration(payload: &[u8], source_slot: u8, layout: DecodeLayout) -> Vibration {
    if !(1..=3).contains(&source_slot) {
        return Vibration { left_level: 0, right_level: 0 };
    }
    let section = layout.adjust(tables::VIBRATION_SECTION_OFFSET)
        + (usize::from(source_slot - 1) * tables::VIBRATION_SECTION_STRIDE);
    let left_bits = read_u32_le(payload, section + 4).unwrap_or(0);
    let right_bits = read_u32_le(payload, section + 8).unwrap_or(0);
    let left = f32::from_bits(left_bits);
    let right = f32::from_bits(right_bits);
    Vibration { left_level: scale_vibration(left), right_level: scale_vibration(right) }
}

/// Maps a normalized vibration float to a clamped 0..=5 level (`qRound`-equiv).
fn scale_vibration(value: f32) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    let scaled = f64::from(value * tables::VIBRATION_LEVEL_SCALE);
    // qRound for floats: floor(x + 0.5); clamp into the level range before the
    // cast so the result is provably representable as i32.
    let rounded = (scaled + 0.5).floor().clamp(0.0, f64::from(tables::VIBRATION_LEVEL_MAX));
    // Safe: `rounded` is in [0.0, 5.0] and integral.
    #[allow(clippy::cast_possible_truncation)]
    let level = rounded as i32;
    level
}

/// Detects the 1-based active source slot via the flag-position markers.
fn detect_source_slot(payload: &[u8], layout: DecodeLayout) -> u8 {
    for slot_index in 0..3u8 {
        let offset = layout.adjust(tables::FLAG_BASE_OFFSET)
            + (usize::from(slot_index) * tables::FLAG_STRIDE);
        if marker_at(payload, offset) {
            return slot_index + 1;
        }
    }
    0
}

/// Builds the canonical `{mode}-slot-{slot}-index-{index}` id.
fn canonical_id(mode: Mode, source_slot: u8, source_profile_index: u8) -> String {
    format!("{}-slot-{source_slot}-index-{source_profile_index}", mode.as_str())
}

/// Decodes a raw Pro 3 profile blob into a canonical profile summary.
///
/// Faithful port of `core::ProfileMapper::mapProfile`. The profile mode is
/// taken from `raw.mode_hint` (always set on readback); the mode field at
/// `0x0012` is only consulted as a fallback.
///
/// # Errors
/// Returns [`Error::Decode`] if the payload is smaller than
/// [`tables::EXPECTED_PROFILE_SIZE`] or has no active slot marker.
pub fn map_profile(_device: &Pro3, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
    if raw.payload.len() < tables::EXPECTED_PROFILE_SIZE {
        return Err(Error::Decode(
            "profile payload is too small to decode canonical fields".to_owned(),
        ));
    }

    let payload = raw.payload.as_slice();
    let layout = detect_layout(payload);

    let source_slot =
        if raw.source_slot > 0 { raw.source_slot } else { detect_source_slot(payload, layout) };
    if source_slot == 0 {
        return Err(Error::Decode("profile payload is missing an active slot marker".to_owned()));
    }

    // The hinted mode always wins on readback (it is always present and valid in
    // Rust, where `mode_hint` is a typed `Mode`). The LE16 field at `0x0012` is a
    // readback artifact and is only consulted via [`decode_mode_field`] for
    // hint-less paths exercised by unit tests.
    let mode = raw.mode_hint;

    let name = decode_name(payload, source_slot, layout);
    let id = canonical_id(mode, source_slot, raw.source_profile_index);

    let canonical = CanonicalProfile {
        id: id.clone(),
        name: name.clone(),
        version: 1,
        kind: "8bitdo.pro3.profile".to_owned(),
        device: "8bitdo-pro3".to_owned(),
        mode,
        preferred_slot: None,
        sticks: decode_sticks(payload, source_slot, layout),
        triggers: decode_triggers(payload, mode, source_slot, layout),
        vibration: decode_vibration(payload, source_slot, layout),
        button_mappings: decode_button_mappings(payload, mode, source_slot),
        macro_refs: Vec::new(),
    };

    Ok(CanonicalProfileSummary {
        id,
        name,
        mode,
        source_slot,
        source_profile_index: raw.source_profile_index,
        canonical,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn to_percent_rounds_half_up_and_clamps() {
        assert_eq!(to_percent(0, 128), 0);
        assert_eq!(to_percent(128, 128), 100);
        // 26 * 100 / 128 = 20.31 -> 20 ; 102 * 100 / 128 = 79.69 -> 80.
        assert_eq!(to_percent(26, 128), 20);
        assert_eq!(to_percent(102, 128), 80);
        assert_eq!(to_percent(200, 128), 100);
        assert_eq!(to_percent(5, 0), 0);
    }

    #[test]
    fn canonical_id_uses_mode_slot_index() {
        assert_eq!(canonical_id(Mode::XInput, 1, 0), "xinput-slot-1-index-0");
        assert_eq!(canonical_id(Mode::Switch, 2, 1), "switch-slot-2-index-1");
    }

    #[test]
    fn decode_mode_field_parses_shifted_le16() {
        // Shifted layout: canonical 0x0012 -> 0x0010. XInput value = 0x0003 LE.
        let mut blob = vec![0u8; tables::EXPECTED_PROFILE_SIZE];
        blob[0x0010] = 0x03;
        let layout = DecodeLayout { shifted_by_two: true };
        assert_eq!(decode_mode_field(&blob, layout), Some(Mode::XInput));
        blob[0x0010] = 0x00;
        assert_eq!(decode_mode_field(&blob, layout), Some(Mode::Switch));
        blob[0x0010] = 0x01;
        assert_eq!(decode_mode_field(&blob, layout), Some(Mode::DInput));
        blob[0x0010] = 0x07;
        assert_eq!(decode_mode_field(&blob, layout), None);
    }

    #[test]
    fn too_small_payload_is_decode_error() {
        let raw = RawProfilePayload {
            payload: vec![0u8; 16],
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: Mode::XInput,
        };
        assert!(matches!(map_profile(&Pro3, &raw), Err(Error::Decode(_))));
    }

    #[test]
    fn home_guide_is_forced_identity_even_with_spurious_encoding() {
        let entries = &tables::XINPUT_ENCODINGS;
        // A spurious turbo-like encoding at the home/guide index must stay identity.
        let target =
            decode_target_control(entries, tables::HOME_GUIDE_INDEX, [0x00, 0x00, 0x01, 0x00]);
        assert_eq!(target, "home/guide");
    }
}
