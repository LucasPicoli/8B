//! Pro 3 profile blob layout constants and per-mode button-encoding tables.
//!
//! Ported verbatim from `src/core/profile_mapper.cpp` and `button_names.h` of
//! the C++ reference implementation. Every magic offset, stride and 4-byte
//! button encoding lives here as a named, documented constant so the decoder in
//! [`super::profile`] reads as plain logic and the golden vectors stay the
//! single source of truth.
//!
//! # Layout shift
//! The device stores blobs shifted **−2** from the canonical layout (no 2-byte
//! prefix). The offsets below are the *canonical* values; the decoder subtracts
//! 2 when the slot marker is detected at the shifted flag positions. The
//! button-map section is the exception — it always lives at the canonical base
//! [`BUTTON_MAP_BASE_OFFSET`] regardless of shift.

/// Minimum decodable blob size (`0x092C` = 2348 bytes).
pub const EXPECTED_PROFILE_SIZE: usize = 0x092C;

/// Canonical offset of the LE16 mode field (readback artifact).
pub const MODE_OFFSET: usize = 0x0012;
/// Canonical offset of the first 32-byte UTF-16BE profile name.
pub const NAME_BASE_OFFSET: usize = 0x0016;
/// Stride between per-slot name fields.
pub const NAME_STRIDE: usize = 0x20;
/// Length in bytes of a name field.
pub const NAME_BYTES: usize = 0x20;
/// Canonical offset of the first slot flag/marker block.
pub const FLAG_BASE_OFFSET: usize = 0x0002;
/// Stride between per-slot flag/marker blocks.
pub const FLAG_STRIDE: usize = 0x04;

/// Canonical base of the per-slot stick-range block (4 bytes/slot).
pub const STICK_DATA_OFFSET: usize = 0x009E;
/// Canonical base of the per-slot trigger-range block (4 bytes/slot).
pub const TRIGGER_DATA_OFFSET: usize = 0x00B6;
/// Canonical base of the per-slot stick/trigger flag block.
pub const SLOT_FLAGS_OFFSET: usize = 0x00CE;
/// Canonical base of the per-slot vibration-intensity section.
pub const VIBRATION_SECTION_OFFSET: usize = 0x0076;
/// Stride between per-slot stick/trigger/flag blocks.
pub const SLOT_DATA_STRIDE: usize = 8;
/// Stride between per-slot vibration-intensity sections.
pub const VIBRATION_SECTION_STRIDE: usize = 12;

/// Button-map base — **always canonical**, never shifted.
pub const BUTTON_MAP_BASE_OFFSET: usize = 0x00E4;
/// Stride between per-slot button-map regions.
pub const BUTTON_MAP_SLOT_STRIDE: usize = 0x5C;
/// Bytes per button-map entry (one 4-byte encoding).
pub const BUTTON_ENTRY_BYTES: usize = 4;

/// The 4-byte active-slot marker (`11 09 20 20`).
pub const SLOT_MARKER: [u8; 4] = [0x11, 0x09, 0x20, 0x20];
/// The 4-byte "disabled / unmapped" encoding (`00 00 00 00`).
pub const NULL_ENCODING: [u8; 4] = [0x00, 0x00, 0x00, 0x00];

/// Index of `home/guide` — cannot be remapped, forced to identity.
pub const HOME_GUIDE_INDEX: usize = 13;
/// First index of the null-default back paddles (`rp`, `lp`, `l4`, `r4`).
pub const NULL_DEFAULT_FIRST_INDEX: usize = 18;
/// Number of physical source buttons (indices 0..22).
pub const SOURCE_BUTTON_COUNT: usize = 22;

// Stick/trigger percent clamp ranges (schema-enforced; see profile-v1.schema).
/// Lower clamp for stick min percent.
pub const STICK_MIN_PCT_LO: i32 = 0;
/// Upper clamp for stick min percent.
pub const STICK_MIN_PCT_HI: i32 = 90;
/// Lower clamp for stick max percent.
pub const STICK_MAX_PCT_LO: i32 = 10;
/// Upper clamp for stick max percent.
pub const STICK_MAX_PCT_HI: i32 = 100;
/// Lower clamp for the Switch trigger threshold percent.
pub const SWITCH_THRESHOLD_PCT_LO: i32 = 0;
/// Upper clamp for the Switch trigger threshold percent.
pub const SWITCH_THRESHOLD_PCT_HI: i32 = 90;

/// Raw-byte denominator for stick percent conversion.
pub const STICK_RAW_MAX: i32 = 128;
/// Raw-byte denominator for trigger percent conversion.
pub const TRIGGER_RAW_MAX: i32 = 255;
/// Multiplier mapping a normalized vibration float to a 0..=5 level.
pub const VIBRATION_LEVEL_SCALE: f32 = 5.0;
/// Maximum vibration level.
pub const VIBRATION_LEVEL_MAX: i32 = 5;

/// A single button's decode entry: its canonical name, its primary 4-byte
/// encoding, and any alternate "identity" encodings the device uses for that
/// button when a remap is active elsewhere in the slot.
#[derive(Debug, Clone, Copy)]
pub struct ButtonEncodingEntry {
    /// Canonical source-control name (e.g. `"right face"`).
    pub source: &'static str,
    /// Primary 4-byte encoding observed on an all-default slot.
    pub encoding: [u8; 4],
    /// Alternate identity encodings (face buttons in customized slots).
    pub variant_identity_encodings: &'static [[u8; 4]],
}

/// Readback-derived encoding table for **`XInput`** mode.
///
/// Primary encodings come from an all-default slot 1 on live hardware. The
/// face-button variant encodings are the alternate representations the device
/// writes in slots that have any remap active.
pub const XINPUT_ENCODINGS: [ButtonEncodingEntry; SOURCE_BUTTON_COUNT] = [
    // 0: right face — primary collides with left face; variant 00 10 00 00.
    ButtonEncodingEntry {
        source: "right face",
        encoding: [0x00, 0x00, 0x00, 0x20],
        variant_identity_encodings: &[[0x00, 0x10, 0x00, 0x00]],
    },
    // 1: bottom face — primary 00 00 20 00; variant 00 20 00 00.
    ButtonEncodingEntry {
        source: "bottom face",
        encoding: [0x00, 0x00, 0x20, 0x00],
        variant_identity_encodings: &[[0x00, 0x20, 0x00, 0x00]],
    },
    // 2: top face — 20 00 00 00 (same in both sets).
    ButtonEncodingEntry {
        source: "top face",
        encoding: [0x20, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 3: left face — primary collides with right face; variant 10 00 00 00.
    ButtonEncodingEntry {
        source: "left face",
        encoding: [0x00, 0x00, 0x00, 0x20],
        variant_identity_encodings: &[[0x10, 0x00, 0x00, 0x00]],
    },
    // 4-9: shoulder + stick clicks.
    ButtonEncodingEntry {
        source: "l1",
        encoding: [0x00, 0x04, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r1",
        encoding: [0x00, 0x08, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "l2",
        encoding: [0x00, 0x40, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r2",
        encoding: [0x00, 0x80, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "l3",
        encoding: [0x02, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r3",
        encoding: [0x04, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 10-13: menu cluster.
    ButtonEncodingEntry {
        source: "select/back",
        encoding: [0x08, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "start/menu",
        encoding: [0x01, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "turbo",
        encoding: [0x00, 0x00, 0x01, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "home/guide",
        encoding: [0x00, 0x00, 0x02, 0x00],
        variant_identity_encodings: &[],
    },
    // 14-17: d-pad.
    ButtonEncodingEntry {
        source: "d-pad up",
        encoding: [0x00, 0x02, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad down",
        encoding: [0x00, 0x01, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad left",
        encoding: [0x80, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad right",
        encoding: [0x40, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 18-21: back paddles — null default, valid as SOURCE only.
    ButtonEncodingEntry { source: "rp", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "lp", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "l4", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "r4", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
];

/// Readback-derived encoding table for **Switch** mode.
///
/// Differs from `XInput` on face buttons (0-3) and turbo (12). The trailing
/// `screenshot` entry is a target-only lookup entry (not a physical source
/// button); Switch turbo defaults to Screenshot (`00 00 40 00`).
pub const SWITCH_ENCODINGS: [ButtonEncodingEntry; SOURCE_BUTTON_COUNT + 1] = [
    // 0-3: switch-specific face buttons.
    ButtonEncodingEntry {
        source: "right face",
        encoding: [0x00, 0x20, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "bottom face",
        encoding: [0x00, 0x10, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "top face",
        encoding: [0x10, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "left face",
        encoding: [0x20, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 4-9: shoulder + stick clicks (same as XInput).
    ButtonEncodingEntry {
        source: "l1",
        encoding: [0x00, 0x04, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r1",
        encoding: [0x00, 0x08, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "l2",
        encoding: [0x00, 0x40, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r2",
        encoding: [0x00, 0x80, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "l3",
        encoding: [0x02, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "r3",
        encoding: [0x04, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 10-11: menu buttons (same as XInput).
    ButtonEncodingEntry {
        source: "select/back",
        encoding: [0x08, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "start/menu",
        encoding: [0x01, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 12: turbo — switch uses the XInput-style turbo signal (00 00 01 00).
    ButtonEncodingEntry {
        source: "turbo",
        encoding: [0x00, 0x00, 0x01, 0x00],
        variant_identity_encodings: &[],
    },
    // 13: home/guide (same as XInput).
    ButtonEncodingEntry {
        source: "home/guide",
        encoding: [0x00, 0x00, 0x02, 0x00],
        variant_identity_encodings: &[],
    },
    // 14-17: d-pad (same as XInput).
    ButtonEncodingEntry {
        source: "d-pad up",
        encoding: [0x00, 0x02, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad down",
        encoding: [0x00, 0x01, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad left",
        encoding: [0x80, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    ButtonEncodingEntry {
        source: "d-pad right",
        encoding: [0x40, 0x00, 0x00, 0x00],
        variant_identity_encodings: &[],
    },
    // 18-21: back paddles — null default, valid as SOURCE only.
    ButtonEncodingEntry { source: "rp", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "lp", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "l4", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    ButtonEncodingEntry { source: "r4", encoding: NULL_ENCODING, variant_identity_encodings: &[] },
    // 22: screenshot — target-only entry (switch turbo default).
    ButtonEncodingEntry {
        source: "screenshot",
        encoding: [0x00, 0x00, 0x40, 0x00],
        variant_identity_encodings: &[],
    },
];
