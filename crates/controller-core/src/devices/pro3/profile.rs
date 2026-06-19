//! Pro 3 profile decoder (`map_profile`) and compiler (`compile_profile`).
//!
//! The decoder is a faithful port of `core::ProfileMapper::mapProfile` from
//! `src/core/profile_mapper.cpp`. The compiler is a faithful port of
//! `core::ProfileCompiler::compile` from `src/core/profile_compiler.cpp`.
//! Both are verified byte-for-byte against golden vectors captured from live
//! hardware and the C++ reference encoder (`tests/golden_profile_compile.rs`,
//! `tests/golden_profile_decode.rs`).
//!
//! All variable-offset reads/writes go through the bounds-checked
//! [`crate::protocol::bytes`] accessors so both codec paths are panic-free
//! even on truncated or corrupted input.

use crate::devices::pro3::macros::encode_macro_metadata;
use crate::devices::pro3::tables;
use crate::devices::pro3::tables::ButtonEncodingEntry;
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, Mode,
    RawProfilePayload, Slot, Sticks, Triggers, TriggersAnalog, TriggersSwitch, Vibration,
};
use crate::protocol::bytes::{put_slice, put_u16_le, put_u32_le, read_u32_le, read_u8, take};
use crate::protocol::crc16::crc16_modbus;
use crate::protocol::text::{decode_utf16be_name, encode_utf16be_name};

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

// ============================================================================
// Compiler — `compile_profile` (faithful inverse of `map_profile`)
// ============================================================================

// Device-native (shifted −2) layout offsets used by the compiler.
// These are the offsets the device expects in the blob it receives.
// Button-map entries and Section-4 macros are the ONLY exceptions: they
// live at the canonical (non-shifted) offsets in both directions.

/// Device-native base of the slot-flag/marker blocks (canonical `FLAG_BASE_OFFSET` − 2).
const DEV_FLAG_BASE: usize = 0x0000;
/// Stride between flag blocks (same as canonical).
const DEV_FLAG_STRIDE: usize = tables::FLAG_STRIDE;

/// Device-native offset of the LE16 gamepad-mode field (canonical `MODE_OFFSET` − 2).
const DEV_MODE_OFFSET: usize = 0x0010;
/// Device-native base of the per-slot UTF-16BE name fields (canonical `NAME_BASE_OFFSET` − 2).
const DEV_NAME_BASE: usize = 0x0014;

/// Device-native base of the per-slot vibration-intensity section (canonical
/// `VIBRATION_SECTION_OFFSET` − 2).
const DEV_VIB_BASE: usize = 0x0074;

/// Device-native base of the per-slot stick-flag block (canonical − 2).
const DEV_STICK_FLAG_BASE: usize = 0x0098;
/// Device-native base of the per-slot stick-data block (canonical `STICK_DATA_OFFSET` − 2).
const DEV_STICK_DATA_BASE: usize = 0x009C;

/// Device-native base of the per-slot trigger-flag block (canonical − 2).
const DEV_TRIG_FLAG_BASE: usize = 0x00B0;
/// Device-native base of the per-slot trigger-data block (canonical `TRIGGER_DATA_OFFSET` − 2).
const DEV_TRIG_DATA_BASE: usize = 0x00B4;

/// Device-native offset of the Section-3 global marker (canonical `0x00CA` − 2).
const DEV_SECT3_MARKER: usize = 0x00C8;
/// Device-native base of the per-slot stick/dpad flags (canonical `SLOT_FLAGS_OFFSET` − 2).
const DEV_FLAGS_BASE: usize = 0x00CC;

/// Device-native base of the per-slot button-map sub-marker (canonical `0x00E2` − 2).
const DEV_BTN_MARKER_BASE: usize = 0x00E0;
/// Button-map entries: NOT shifted (canonical `BUTTON_MAP_BASE_OFFSET` = `0x00E4`).
const DEV_BTN_DATA_BASE: usize = tables::BUTTON_MAP_BASE_OFFSET; // 0x00E4

/// Section-4 macro base: NOT shifted (canonical `SECTION4_BASE_OFFSET` = `0x068C`).
const DEV_SECT4_BASE: usize = tables::SECTION4_BASE_OFFSET; // 0x068C

/// Device-native base of Section 5 vibration motor range (canonical `0x0916` − 2).
const DEV_SECT5_BASE: usize = 0x0914;

/// Device-native offset of the CRC field (canonical `0x000E` − 2).
const DEV_CRC_OFFSET: usize = 0x000C;

/// Mode LE16 encoding: Switch.
const MODE_SWITCH: u16 = 0x0000;
/// Mode LE16 encoding: `DInput`.
const MODE_DINPUT: u16 = 0x0001;
/// Mode LE16 encoding: `XInput`.
const MODE_XINPUT: u16 = 0x0003;

/// Converts a percent (0..=100) to the stick raw byte using `qRound` semantics
/// (round-half-up for non-negative values, clamp to `0..=STICK_RAW_MAX`).
fn percent_to_stick_byte(pct: i32) -> u8 {
    let clamped = pct.clamp(0, 100);
    // qRound: round(clamped * 128 / 100) — use f64 to match C++ qRound(double).
    let raw = (f64::from(clamped) * f64::from(tables::STICK_RAW_MAX) / 100.0 + 0.5).floor();
    // Safe: raw in [0.0, 128.0], representable as u8.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = raw.clamp(0.0, f64::from(tables::STICK_RAW_MAX)) as u8;
    byte
}

/// Converts a percent (0..=100) to the trigger raw byte using `qRound` semantics
/// (round-half-up, clamp to `0..=TRIGGER_RAW_MAX`).
fn percent_to_trigger_byte(pct: i32) -> u8 {
    let clamped = pct.clamp(0, 100);
    let raw = (f64::from(clamped) * f64::from(tables::TRIGGER_RAW_MAX) / 100.0 + 0.5).floor();
    // Safe: raw in [0.0, 255.0], representable as u8.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let byte = raw.clamp(0.0, f64::from(tables::TRIGGER_RAW_MAX)) as u8;
    byte
}

/// Maps a canonical control name to its 0-based index in the encoding tables.
///
/// Returns `usize::MAX` (sentinel for "not found") rather than panicking.
fn control_name_to_index(name: &str) -> usize {
    encodings_for(Mode::XInput).iter().position(|e| e.source == name).unwrap_or(usize::MAX)
}

/// Picks the write-mode encoding table for `mode`.
const fn encodings_for(mode: Mode) -> &'static [tables::ButtonEncodingEntry] {
    match mode {
        Mode::Switch => &tables::SWITCH_ENCODINGS,
        _ => &tables::XINPUT_ENCODINGS,
    }
}

/// Returns the on-wire write encoding for a target control name.
///
/// The variant set wins for `XInput` face buttons (right/left/bottom face carry
/// alternate encodings that the official app always writes when any remap is
/// active). `disabled`/`screenshot` are handled by the caller's remap logic.
///
/// # Errors
/// Returns [`Error::Validation`] when `name` is not in the mode's encoding table.
fn write_encoding(name: &str, mode: Mode) -> Result<[u8; 4]> {
    encodings_for(mode)
        .iter()
        .find(|e| e.source == name)
        .map(|e| e.variant_identity_encodings.first().copied().unwrap_or(e.encoding))
        .ok_or_else(|| Error::Validation(format!("unknown target control: {name}")))
}

/// Compiles a [`CanonicalProfile`] into the device-native 2348-byte blob.
///
/// The compiler is a faithful, section-by-section port of
/// `core::ProfileCompiler::compile` from `src/core/profile_compiler.cpp`.
/// The produced blob is shifted −2 from the canonical layout (no 2-byte prefix),
/// with button-map entries (`0x00E4`) and Section-4 macros (`0x068C`) as the
/// only non-shifted exceptions.
///
/// `base_blob` is used as the read-modify-write baseline when its length equals
/// [`tables::EXPECTED_PROFILE_SIZE`]; otherwise a zeroed blob is used. Only
/// the sections belonging to `target_slot` are overwritten; other slots are
/// preserved as-is from `base_blob`.
///
/// `macros` carries the resolved [`MacroDefinition`] items for the target slot.
///
/// # Errors
/// Returns [`Error::Validation`] on an unknown control/trigger name, an
/// out-of-range slot, or an encoding overflow.
// The function is a faithful section-by-section port of a single C++ function;
// splitting it would hurt readability more than it helps.
#[allow(clippy::cognitive_complexity, clippy::too_many_lines)]
pub fn compile_profile(
    profile: &CanonicalProfile,
    target_slot: Slot,
    base_blob: &[u8],
    macros: &[MacroDefinition],
) -> Result<Vec<u8>> {
    // Start from base (read-modify-write) or a fresh zeroed buffer.
    let mut buf = if base_blob.len() == tables::EXPECTED_PROFILE_SIZE {
        base_blob.to_vec()
    } else {
        vec![0u8; tables::EXPECTED_PROFILE_SIZE]
    };

    let s = target_slot.get();
    let idx = usize::from(s - 1);

    // --- Section 0: flags, mode, name ---

    // Active-slot marker at the target flag block (device-native, shifted −2).
    let flag_off = DEV_FLAG_BASE + idx * DEV_FLAG_STRIDE;
    put_slice(&mut buf, flag_off, &tables::SLOT_MARKER)?;

    // Mode LE16.
    let mode_enc = match profile.mode {
        Mode::Switch => MODE_SWITCH,
        Mode::DInput => MODE_DINPUT,
        Mode::XInput => MODE_XINPUT,
    };
    put_u16_le(&mut buf, DEV_MODE_OFFSET, mode_enc)?;

    // UTF-16BE name into the per-slot field.
    let name_off = DEV_NAME_BASE + idx * tables::NAME_STRIDE;
    let name_bytes = encode_utf16be_name(&profile.name, tables::NAME_BYTES);
    put_slice(&mut buf, name_off, &name_bytes)?;

    // --- Section 1: vibration intensity ---

    let vib_off = DEV_VIB_BASE + idx * tables::VIBRATION_SECTION_STRIDE;
    put_slice(&mut buf, vib_off, &tables::SLOT_MARKER)?;

    // Vibration float = clamp(level, 0, 5) / 5.0 written as IEEE 754 LE.
    // The i32 value is clamped to 0..=5 before the cast, so cast is exact.
    #[allow(clippy::cast_precision_loss)]
    let left_float = (profile.vibration.left_level.clamp(0, tables::VIBRATION_LEVEL_MAX) as f32)
        / tables::VIBRATION_LEVEL_SCALE;
    #[allow(clippy::cast_precision_loss)]
    let right_float = (profile.vibration.right_level.clamp(0, tables::VIBRATION_LEVEL_MAX) as f32)
        / tables::VIBRATION_LEVEL_SCALE;
    put_u32_le(&mut buf, vib_off + 4, left_float.to_bits())?;
    put_u32_le(&mut buf, vib_off + 8, right_float.to_bits())?;

    // --- Section 2A: stick ranges ---

    let stick_flag_off = DEV_STICK_FLAG_BASE + idx * tables::SLOT_DATA_STRIDE;
    put_slice(&mut buf, stick_flag_off, &tables::SLOT_MARKER)?;

    let stick_data_off = DEV_STICK_DATA_BASE + idx * tables::SLOT_DATA_STRIDE;
    let sticks = &profile.sticks;
    put_slice(
        &mut buf,
        stick_data_off,
        &[
            percent_to_stick_byte(sticks.left_min_pct),
            percent_to_stick_byte(sticks.left_max_pct),
            percent_to_stick_byte(sticks.right_min_pct),
            percent_to_stick_byte(sticks.right_max_pct),
        ],
    )?;

    // --- Section 2B: trigger ranges ---

    let trig_flag_off = DEV_TRIG_FLAG_BASE + idx * tables::SLOT_DATA_STRIDE;
    put_slice(&mut buf, trig_flag_off, &tables::SLOT_MARKER)?;

    let trig_data_off = DEV_TRIG_DATA_BASE + idx * tables::SLOT_DATA_STRIDE;
    let trig_bytes: [u8; 4] = match &profile.triggers {
        Triggers::Analog(a) => [
            percent_to_trigger_byte(a.left_min_pct),
            percent_to_trigger_byte(a.left_max_pct),
            percent_to_trigger_byte(a.right_min_pct),
            percent_to_trigger_byte(a.right_max_pct),
        ],
        Triggers::Switch(sw) => [
            percent_to_trigger_byte(sw.left_threshold_pct),
            0xFF,
            percent_to_trigger_byte(sw.right_threshold_pct),
            0xFF,
        ],
    };
    put_slice(&mut buf, trig_data_off, &trig_bytes)?;

    // --- Section 3: stick/dpad flags + button map ---

    // Global Section-3 marker (always at device-native 0x00C8).
    put_slice(&mut buf, DEV_SECT3_MARKER, &tables::SLOT_MARKER)?;

    // Per-slot flags (stride 8: [4B marker][2B flags][2B pad]).
    let slot_flags_off = DEV_FLAGS_BASE + idx * tables::SLOT_DATA_STRIDE;
    let sticks = &profile.sticks;
    let mut flags0: u8 = 0;
    let mut flags1: u8 = 0;
    if sticks.invert_left_x {
        flags0 |= 0x01;
    }
    if sticks.invert_left_y {
        flags0 |= 0x02;
    }
    if sticks.invert_right_x {
        flags0 |= 0x04;
    }
    if sticks.invert_right_y {
        flags0 |= 0x08;
    }
    if sticks.swap_sticks {
        flags0 |= 0x10;
    }
    let swap_triggers = match &profile.triggers {
        Triggers::Analog(a) => a.swap_triggers,
        Triggers::Switch(sw) => sw.swap_triggers,
    };
    if swap_triggers {
        flags0 |= 0x80;
    }
    if sticks.swap_dpad_with_left_stick {
        flags1 |= 0x01;
    }
    put_slice(&mut buf, slot_flags_off, &[flags0, flags1])?;

    // Inter-slot marker after each slot's flags (slots 1 and 2 only).
    if s < 3 {
        put_slice(&mut buf, slot_flags_off + 4, &tables::SLOT_MARKER)?;
    }

    // Button-map sub-marker (device-native 0x00E0 + idx*0x5C).
    let btn_marker_off = DEV_BTN_MARKER_BASE + idx * tables::BUTTON_MAP_SLOT_STRIDE;
    put_slice(&mut buf, btn_marker_off, &tables::SLOT_MARKER)?;

    // Build remap-override lookup: source_index → 4-byte wire encoding.
    // Faithful port of C++ `populateSection3` inner loop.
    let mode = profile.mode;
    let mut remap_overrides: Vec<Option<[u8; 4]>> = vec![None; tables::SOURCE_BUTTON_COUNT];
    for mapping in &profile.button_mappings {
        let source_idx = control_name_to_index(&mapping.source);
        if source_idx >= tables::SOURCE_BUTTON_COUNT {
            continue;
        }
        let enc = if mapping.target == "disabled" {
            tables::NULL_ENCODING
        } else if mapping.target == "screenshot" && mode == Mode::Switch {
            [0x00, 0x00, 0x40, 0x00]
        } else if mapping.target == "screenshot" {
            // XInput: "screenshot" is not a valid XInput target; treat as identity.
            write_encoding(&mapping.source, mode)?
        } else {
            write_encoding(&mapping.target, mode)?
        };
        // Remap for home/guide is silently forced to identity (encoder parity with
        // the decoder which always returns identity for index 13).
        let forced = if source_idx == tables::HOME_GUIDE_INDEX {
            Some(write_encoding("home/guide", mode)?)
        } else {
            Some(enc)
        };
        if let Some(slot) = remap_overrides.get_mut(source_idx) {
            *slot = forced;
        }
    }

    // Switch turbo default override (index 12): screenshot encoding when not remapped.
    let switch_turbo_default: Option<[u8; 4]> =
        if mode == Mode::Switch { Some([0x00, 0x00, 0x40, 0x00]) } else { None };

    // Write 22 button entries (button-map section is NOT shifted — canonical 0x00E4).
    let entry_base = DEV_BTN_DATA_BASE + idx * tables::BUTTON_MAP_SLOT_STRIDE;
    for i in 0..tables::SOURCE_BUTTON_COUNT {
        let entry_off = entry_base + i * tables::BUTTON_ENTRY_BYTES;
        let enc = remap_overrides.get(i).copied().flatten().unwrap_or_else(|| {
            if i == 12 {
                // Turbo (index 12): apply switch turbo default when in Switch mode.
                switch_turbo_default.unwrap_or_else(|| {
                    write_encoding("turbo", mode).unwrap_or(tables::NULL_ENCODING)
                })
            } else {
                write_encoding(encodings_for(mode).get(i).map_or("", |e| e.source), mode)
                    .unwrap_or(tables::NULL_ENCODING)
            }
        });
        put_slice(&mut buf, entry_off, &enc)?;
    }

    // --- Section 4: macro metadata ---

    let sect4_base = DEV_SECT4_BASE + idx * tables::SECTION4_SLOT_STRIDE;
    put_slice(&mut buf, sect4_base, &tables::SLOT_MARKER)?;
    put_u32_le(
        &mut buf,
        sect4_base + 4,
        u32::try_from(tables::MACRO_SLOTS_PER_PROFILE)
            .map_err(|_| Error::Validation("macro slot count overflow".into()))?,
    )?;
    // Zero the descriptor region (208 bytes = 4 × 52).
    let desc_region_size = tables::MACRO_SLOTS_PER_PROFILE * tables::MACRO_DESCRIPTOR_SIZE;
    put_slice(
        &mut buf,
        sect4_base + tables::SECTION4_RECORD_HEADER_SIZE,
        &vec![0u8; desc_region_size],
    )?;
    // Write each provided macro descriptor into its slot.
    for m in macros {
        let slot_idx = usize::from(m.macro_slot.unwrap_or(0));
        if slot_idx >= tables::MACRO_SLOTS_PER_PROFILE {
            continue;
        }
        let macro_slot = MacroSlot::new(
            u8::try_from(slot_idx)
                .map_err(|_| Error::Validation("macro slot index overflow".into()))?,
        )?;
        let descriptor = encode_macro_metadata(m, macro_slot)?;
        let desc_off = sect4_base
            + tables::SECTION4_RECORD_HEADER_SIZE
            + slot_idx * tables::MACRO_DESCRIPTOR_SIZE;
        put_slice(&mut buf, desc_off, &descriptor)?;
    }

    // --- Section 5: vibration motor range ---

    // Stride for section 5 is 8 bytes per slot (not 216).
    let sect5_off = DEV_SECT5_BASE + idx * 8;
    put_slice(&mut buf, sect5_off, &tables::SLOT_MARKER)?;
    // Default motor range: L_start=1, L_end=100, R_start=1, R_end=100.
    // The last byte may be truncated by the 2348-byte buffer end — put_slice handles this via
    // its bounds check; we use individual puts to only write what fits.
    let range_off = sect5_off + 4;
    put_slice(&mut buf, range_off, &[1u8, 100u8])?;
    // R range: only write if it fits within EXPECTED_PROFILE_SIZE.
    if range_off + 4 <= tables::EXPECTED_PROFILE_SIZE {
        put_slice(&mut buf, range_off + 2, &[1u8, 100u8])?;
    }

    // --- CRC (final step) ---
    // Zero the 4-byte CRC field, compute CRC-16/MODBUS over whole buffer, write LE16.
    put_u32_le(&mut buf, DEV_CRC_OFFSET, 0)?;
    let crc = crc16_modbus(&buf);
    put_u16_le(&mut buf, DEV_CRC_OFFSET, crc)?;

    Ok(buf)
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
