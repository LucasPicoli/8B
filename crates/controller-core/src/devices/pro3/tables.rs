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

// ---------------------------------------------------------------------------
// Macro Section-4 metadata layout (`record_macro_content_t`, 52B each).
//
// Ported from `src/core/macro_decoder.cpp::decodeMetadata`. Section 4 holds, per
// profile slot, a `record_macro_fun_record_t` (216B): an 8-byte header followed
// by 4 × 52-byte macro descriptors.
// ---------------------------------------------------------------------------

/// Base offset of Section 4 in the profile blob (`0x068C`).
pub const SECTION4_BASE_OFFSET: usize = 0x068C;
/// Stride between per-profile-slot macro record blocks (216 bytes).
pub const SECTION4_SLOT_STRIDE: usize = 216;
/// Size of the per-slot record header (`flag` LE32 + `total_cnt` LE32).
pub const SECTION4_RECORD_HEADER_SIZE: usize = 8;
/// Size of a single macro descriptor (`record_macro_content_t`).
pub const MACRO_DESCRIPTOR_SIZE: usize = 52;
/// Number of macro descriptor slots per profile slot.
pub const MACRO_SLOTS_PER_PROFILE: usize = 4;

/// Length of the UTF-16BE name field at the start of a descriptor.
pub const MACRO_NAME_BYTES: usize = 32;
/// Offset of the `gamepad_mode` byte within a descriptor.
pub const MACRO_MODE_OFFSET: usize = 32;
/// Offset of the `max_steps` LE16 within a descriptor.
pub const MACRO_MAX_STEPS_OFFSET: usize = 34;
/// Offset of the `key_map` LE32 (trigger) within a descriptor.
pub const MACRO_KEY_MAP_OFFSET: usize = 40;
/// Offset of the `cycles_num` (repeat count) LE32 within a descriptor.
pub const MACRO_REPEAT_COUNT_OFFSET: usize = 44;
/// Offset of the `interval_ms` LE32 within a descriptor.
pub const MACRO_INTERVAL_MS_OFFSET: usize = 48;

/// `gamepad_mode` byte value that denotes `XInput`.
pub const MACRO_GAMEPAD_MODE_XINPUT: u8 = 3;

// ---------------------------------------------------------------------------
// Macro step layout (`record_content_t`, 10B each).
//
// Ported from `src/core/macro_decoder.cpp::decodeStepStream`.
// ---------------------------------------------------------------------------

/// Size of a single macro step record (`record_content_t`).
pub const MACRO_STEP_RECORD_SIZE: usize = 10;
/// Offset of the `ms_time` LE16 within a step record.
pub const STEP_MS_TIME_OFFSET: usize = 0;
/// Offset of the `keys` bitmask LE16 within a step record.
pub const STEP_KEYS_OFFSET: usize = 2;
/// Offset of the `trigger_value` LE16 within a step record.
pub const STEP_TRIGGER_VALUE_OFFSET: usize = 4;
/// Offset of the `left_joy` LE16 (`(Y<<8)|X`) within a step record.
pub const STEP_LEFT_JOY_OFFSET: usize = 6;
/// Offset of the `right_joy` LE16 (`(Y<<8)|X`) within a step record.
pub const STEP_RIGHT_JOY_OFFSET: usize = 8;

/// Mask preserving the 14 button bits (clears the L2/R2 bits 14–15).
pub const STEP_BUTTON_BITS_MASK: u16 = 0x3FFF;
/// `keys` bit flagging L2 pressed in Switch mode (bit 14).
pub const STEP_SWITCH_L2_MASK: u16 = 0x4000;
/// `keys` bit flagging R2 pressed in Switch mode (bit 15).
pub const STEP_SWITCH_R2_MASK: u16 = 0x8000;

/// Default centered stick axis value (matches [`crate::model::MacroStep`]).
pub const STICK_CENTER: u8 = 127;

/// A canonical step-button name paired with its 16-bit bitmask, ordered by bit
/// position for deterministic decode output. Ported from
/// `src/core/macro_models.cpp::kStepButtons`.
#[derive(Debug, Clone, Copy)]
pub struct StepButtonEntry {
    /// Canonical step-button name (e.g. `"bottom face"`).
    pub name: &'static str,
    /// 16-bit bitmask flag for this button.
    pub mask: u16,
}

/// The 16 step-button bitmask entries, ordered by ascending bit position.
pub const STEP_BUTTONS: [StepButtonEntry; 16] = [
    StepButtonEntry { name: "start/menu", mask: 0x0001 }, // bit 0
    StepButtonEntry { name: "l3", mask: 0x0002 },         // bit 1
    StepButtonEntry { name: "r3", mask: 0x0004 },         // bit 2
    StepButtonEntry { name: "select/back", mask: 0x0008 }, // bit 3
    StepButtonEntry { name: "top face", mask: 0x0010 },   // bit 4
    StepButtonEntry { name: "left face", mask: 0x0020 },  // bit 5
    StepButtonEntry { name: "d-pad right", mask: 0x0040 }, // bit 6
    StepButtonEntry { name: "d-pad left", mask: 0x0080 }, // bit 7
    StepButtonEntry { name: "d-pad down", mask: 0x0100 }, // bit 8
    StepButtonEntry { name: "d-pad up", mask: 0x0200 },   // bit 9
    StepButtonEntry { name: "l1", mask: 0x0400 },         // bit 10
    StepButtonEntry { name: "r1", mask: 0x0800 },         // bit 11
    StepButtonEntry { name: "bottom face", mask: 0x1000 }, // bit 12
    StepButtonEntry { name: "right face", mask: 0x2000 }, // bit 13
    StepButtonEntry { name: "l2", mask: 0x4000 },         // bit 14
    StepButtonEntry { name: "r2", mask: 0x8000 },         // bit 15
];

/// A canonical trigger name paired with its 32-bit `KeyMap` value.
///
/// Ported from `src/core/macro_models.cpp::triggerEncodeTable` (all 21
/// `MacroTrigger` values); used as a reverse lookup for `keyMapToTriggerName`.
#[derive(Debug, Clone, Copy)]
pub struct TriggerEntry {
    /// Canonical trigger name (e.g. `"l1"`).
    pub name: &'static str,
    /// 32-bit `KeyMap` value identifying the trigger button.
    pub key_map: u32,
}

/// The 21 trigger `KeyMap` entries.
pub const TRIGGERS: [TriggerEntry; 21] = [
    TriggerEntry { name: "start/menu", key_map: 0x0000_0001 },
    TriggerEntry { name: "l3", key_map: 0x0000_0002 },
    TriggerEntry { name: "r3", key_map: 0x0000_0004 },
    TriggerEntry { name: "select/back", key_map: 0x0000_0008 },
    TriggerEntry { name: "top face", key_map: 0x0000_0010 },
    TriggerEntry { name: "left face", key_map: 0x0000_0020 },
    TriggerEntry { name: "d-pad right", key_map: 0x0000_0040 },
    TriggerEntry { name: "d-pad left", key_map: 0x0000_0080 },
    TriggerEntry { name: "d-pad down", key_map: 0x0000_0100 },
    TriggerEntry { name: "d-pad up", key_map: 0x0000_0200 },
    TriggerEntry { name: "l1", key_map: 0x0000_0400 },
    TriggerEntry { name: "r1", key_map: 0x0000_0800 },
    TriggerEntry { name: "bottom face", key_map: 0x0000_1000 },
    TriggerEntry { name: "right face", key_map: 0x0000_2000 },
    TriggerEntry { name: "l2", key_map: 0x0000_4000 },
    TriggerEntry { name: "r2", key_map: 0x0000_8000 },
    TriggerEntry { name: "turbo", key_map: 0x0001_0000 },
    TriggerEntry { name: "l4", key_map: 0x0020_0000 },
    TriggerEntry { name: "lp", key_map: 0x0400_0000 },
    TriggerEntry { name: "rp", key_map: 0x0200_0000 },
    TriggerEntry { name: "r4", key_map: 0x4000_0000 },
];
