//! The Ultimate's blob layout. Codes, indexes and mode bytes live in its description.
//!
//! One image is 1652 bytes, little-endian throughout, and holds three profile slots.
//! Each slot keeps its fields in seven sections spread over the image, each section with
//! a flag of its own. `s` below is the 0-based slot index.
//!
//! | Offset | Bytes | Holds |
//! | --- | --- | --- |
//! | `0x000 + 4s` | 4 | Slot flag |
//! | `0x00C` | 4 | Blob CRC, which the pad does not check |
//! | `0x010` | 2 | The image's mode: 3 `XInput`, 0 Switch |
//! | `0x012` | 2 | The profile the pad plays now, which the pad ignores on write |
//! | `0x014 + 32s` | 32 | Name, UTF-16LE |
//! | `0x074 + 12s` | 12 | Vibration: flag, then left and right as `f32` |
//! | `0x098 + 8s` | 8 | Sticks: flag, then L min, L max, R min, R max |
//! | `0x0B0 + 8s` | 8 | Triggers: flag, then L min, L max, R min, R max |
//! | `0x0C8 + 8s` | 8 | Toggles: flag, then a `u32` of bits |
//! | `0x0E0 + 84s` | 84 | Button map: flag, then 20 `u32` output codes |
//! | `0x1DC + 392s` | 392 | Macros: flag, count, 3 padding bytes, 4 records of 96 |

/// Bytes in one image.
pub const BLOB_SIZE: usize = 0x0674;
/// Profile slots per image.
pub const SLOT_COUNT: u8 = 3;
/// The only flag value that marks a slot or a section in use.
pub const IN_USE: u32 = 0x2020_0911;
/// Bytes of every flag.
pub const FLAG_LEN: usize = 4;

/// Where the slot flags start.
pub const SLOT_FLAG_OFFSET: usize = 0x000;
/// Where the blob CRC sits.
pub const CRC_OFFSET: usize = 0x00C;
/// Bytes of the blob CRC.
pub const CRC_LEN: usize = 4;
/// Where the image's mode sits.
pub const GAMEPAD_MODE_OFFSET: usize = 0x010;
/// Where the profile the pad plays now sits.
pub const SELECTION_OFFSET: usize = 0x012;
/// The selection of an erased image: no profile.
pub const NO_SELECTION: u16 = 6;

/// Where the names start.
pub const NAME_OFFSET: usize = 0x014;
/// Bytes of one name.
pub const NAME_LEN: usize = 32;

/// Where the vibration sections start.
pub const VIBRATION_OFFSET: usize = 0x074;
/// Bytes of one vibration section.
pub const VIBRATION_LEN: usize = 12;
/// Where the stick sections start.
pub const STICKS_OFFSET: usize = 0x098;
/// Where the trigger sections start.
pub const TRIGGERS_OFFSET: usize = 0x0B0;
/// Bytes of one stick or trigger section.
pub const RANGE_LEN: usize = 8;
/// Where the toggle sections start.
pub const TOGGLES_OFFSET: usize = 0x0C8;
/// Bytes of one toggle section.
pub const TOGGLES_LEN: usize = 8;
/// Where the button map sections start.
pub const MAP_OFFSET: usize = 0x0E0;
/// Bytes of one button map section.
pub const MAP_LEN: usize = FLAG_LEN + MAP_ENTRIES * 4;
/// Entries in one button map.
pub const MAP_ENTRIES: usize = 20;
/// Where the macro sections start.
pub const MACROS_OFFSET: usize = 0x1DC;
/// Bytes of one macro section.
pub const MACROS_LEN: usize = MACRO_RECORDS_AT + MACRO_SLOTS * MACRO_RECORD_LEN;

/// Raw value of a stick range end at full travel.
pub const STICK_RAW_MAX: i32 = 128;
/// Raw value of a trigger range end at full travel.
pub const TRIGGER_RAW_MAX: i32 = 255;
/// Vibration levels: a level is the stored strength times this.
pub const VIBRATION_LEVELS: f32 = 5.0;
/// The highest vibration level.
pub const VIBRATION_LEVEL_MAX: i32 = 5;
/// The lowest end of a Switch trigger range by default, about 30%.
pub const SWITCH_TRIGGER_START: i32 = 30;

/// Toggle bit: invert the left stick's X axis.
pub const INVERT_LEFT_X: u32 = 0x1;
/// Toggle bit: invert the left stick's Y axis.
pub const INVERT_LEFT_Y: u32 = 0x2;
/// Toggle bit: invert the right stick's X axis.
pub const INVERT_RIGHT_X: u32 = 0x4;
/// Toggle bit: invert the right stick's Y axis.
pub const INVERT_RIGHT_Y: u32 = 0x8;
/// Toggle bit: swap the left and right sticks.
pub const SWAP_STICKS: u32 = 0x10;
/// Toggle bit: swap the left and right triggers.
pub const SWAP_TRIGGERS: u32 = 0x80;
/// Toggle bit: swap the D-pad and the left stick.
pub const SWAP_DPAD_WITH_LEFT_STICK: u32 = 0x100;
/// Toggle bit: swap the right stick and the triggers (a Switch setting).
pub const SWAP_RIGHT_STICK_WITH_TRIGGERS: u32 = 0x400;
/// Toggle bit: turn the stick dead zone off (an `XInput` setting).
pub const NO_DEAD_ZONE: u32 = 0x1000;
/// Toggle bit: the D-pad gives four directions only.
pub const DPAD_FOUR_WAY: u32 = 0x2000;

/// Macro section: where the macro count sits.
pub const MACRO_COUNT_AT: usize = 4;
/// Macro section: where the 3 padding bytes after the count start.
pub const MACRO_PADDING_AT: usize = 5;
/// Macro section: where the records start.
pub const MACRO_RECORDS_AT: usize = 8;
/// Macro records per slot.
pub const MACRO_SLOTS: usize = 4;
/// Bytes of one macro record.
pub const MACRO_RECORD_LEN: usize = 96;
/// Record: the trigger code, a `u32`.
pub const RECORD_TRIGGER_AT: usize = 0;
/// Record: the step times, one `u16` per step, in ms.
pub const RECORD_TIMES_AT: usize = 4;
/// Record: the step button codes, one `u16` per step.
pub const RECORD_CODES_AT: usize = 40;
/// Record: the step stick bytes, one per step.
pub const RECORD_STICKS_AT: usize = 76;
/// Record: the step count.
pub const RECORD_COUNT_AT: usize = 94;
/// Record: the padding byte at its end.
pub const RECORD_PADDING_AT: usize = 95;
/// Steps per macro record.
pub const MACRO_STEPS: usize = 18;
/// The time a used record stores for each step past its count.
pub const UNUSED_STEP_MS: u16 = 51;

/// Stick byte bits, in the order up, down, left, right: left stick, then right stick.
pub const LEFT_STICK_BITS: [u8; 4] = [0x01, 0x02, 0x04, 0x08];
/// Stick byte bits of the right stick, in the order up, down, left, right.
pub const RIGHT_STICK_BITS: [u8; 4] = [0x10, 0x20, 0x40, 0x80];
