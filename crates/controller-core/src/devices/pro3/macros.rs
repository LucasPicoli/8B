//! Pro 3 macro decoder — `decode_macro_metadata`, `decode_macro_steps`, and the
//! canonical-JSON serializer.
//!
//! A faithful port of `core::MacroDecoder` from `src/core/macro_decoder.cpp`
//! (with the `bitmaskToStepButtonNames` / `keyMapToTriggerName` tables from
//! `src/core/macro_models.cpp`). Verified byte-for-byte against golden vectors
//! captured from the C++ encoder/decoder (`tests/golden_macro_decode.rs`).
//!
//! All variable-offset reads go through the bounds-checked
//! [`crate::protocol::bytes`] accessors so the decoder is panic-free even on
//! truncated or corrupted input.

use serde_json::{json, Map, Value};

use crate::devices::pro3::tables;
use crate::error::Result;
use crate::model::{MacroDefinition, MacroStep, Mode, Slot};
use crate::protocol::bytes::{read_u16_le, read_u32_le, read_u8, take};

/// Converts a 16-bit step-button bitmask to canonical names, ordered by bit
/// position. Port of `macro_models.cpp::bitmaskToStepButtonNames`.
fn bitmask_to_step_button_names(keys: u16) -> Vec<String> {
    tables::STEP_BUTTONS
        .iter()
        .filter(|entry| keys & entry.mask != 0)
        .map(|entry| entry.name.to_owned())
        .collect()
}

/// Converts a 32-bit `KeyMap` value to its canonical trigger name, or `""` when
/// it matches no single-button trigger. Port of
/// `macro_models.cpp::keyMapToTriggerName`.
fn key_map_to_trigger_name(key_map: u32) -> String {
    tables::TRIGGERS
        .iter()
        .find(|entry| entry.key_map == key_map)
        .map_or_else(String::new, |entry| entry.name.to_owned())
}

/// Decodes the macro descriptors in Section 4 of a profile blob for `profile_slot`.
///
/// Reads up to [`tables::MACRO_SLOTS_PER_PROFILE`] descriptors at
/// `0x068C + (slot-1) * 216 + 8 + i * 52`, skipping empty slots
/// (`key_map == 0 && max_steps == 0`). Each returned definition carries
/// metadata only — its `steps` are left empty (filled separately from the step
/// stream). Faithful port of `MacroDecoder::decodeMetadata`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] only via the bounds-checked readers; in
/// practice a truncated descriptor simply terminates the scan, matching the C++.
pub fn decode_macro_metadata(blob: &[u8], profile_slot: Slot) -> Result<Vec<MacroDefinition>> {
    let mut result = Vec::new();

    let slot_index = usize::from(profile_slot.get() - 1);
    let slot_base = tables::SECTION4_BASE_OFFSET
        + (slot_index * tables::SECTION4_SLOT_STRIDE)
        + tables::SECTION4_RECORD_HEADER_SIZE;

    for macro_index in 0..tables::MACRO_SLOTS_PER_PROFILE {
        let descriptor_offset = slot_base + (macro_index * tables::MACRO_DESCRIPTOR_SIZE);

        // A descriptor that does not fully fit terminates the scan (matches C++).
        let Ok(descriptor) = take(blob, descriptor_offset, tables::MACRO_DESCRIPTOR_SIZE) else {
            break;
        };

        let key_map = read_u32_le(descriptor, tables::MACRO_KEY_MAP_OFFSET)?;
        let max_steps = read_u16_le(descriptor, tables::MACRO_MAX_STEPS_OFFSET)?;

        // Skip empty macro slots.
        if key_map == 0 && max_steps == 0 {
            continue;
        }

        let name_bytes = take(descriptor, 0, tables::MACRO_NAME_BYTES)?;
        let name = crate::protocol::text::decode_utf16be_name(name_bytes);

        let mode_byte = read_u8(descriptor, tables::MACRO_MODE_OFFSET)?;
        let mode = gamepad_byte_to_mode(mode_byte);

        let trigger = key_map_to_trigger_name(key_map);
        let repeat_count = read_u32_le(descriptor, tables::MACRO_REPEAT_COUNT_OFFSET)?;
        let interval_ms = read_u32_le(descriptor, tables::MACRO_INTERVAL_MS_OFFSET)?;

        // Macro slot is 0..=3, so the conversion never fails.
        let macro_slot = u8::try_from(macro_index).ok();

        result.push(MacroDefinition {
            name,
            mode,
            trigger,
            repeat_count,
            interval_ms,
            // Pre-populate with `max_steps` default entries so callers know how
            // many steps to read from flash (mirrors C++ `macro.steps.resize(maxSteps)`).
            steps: vec![MacroStep::default(); usize::from(max_steps)],
            macro_slot,
        });
    }

    Ok(result)
}

/// Maps the `gamepad_mode` descriptor byte to a [`Mode`] (`3` → `XInput`, else
/// `Switch`). Mirrors `MacroDecoder::gamepadByteToMode`.
const fn gamepad_byte_to_mode(byte: u8) -> Mode {
    if byte == tables::MACRO_GAMEPAD_MODE_XINPUT {
        Mode::XInput
    } else {
        Mode::Switch
    }
}

/// Decodes `count` × 10-byte step records from a raw step stream.
///
/// Each record is `ms_time` LE16, `keys` LE16, `trigger_value` LE16, `left_joy`
/// LE16 (`(Y<<8)|X`), `right_joy` LE16. L2/R2 decode is mode-aware: in `XInput`
/// they come from `trigger_value` (`(L2<<8)|R2`) and the top two `keys` bits are
/// cleared; in Switch they are the `keys` bits 14–15 (0 or 255). Faithful port
/// of `MacroDecoder::decodeStepStream`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] only via the bounds-checked readers; a
/// record that does not fully fit terminates the walk, matching the C++.
pub fn decode_macro_steps(stream: &[u8], count: usize, mode: Mode) -> Result<Vec<MacroStep>> {
    let is_xinput = mode == Mode::XInput;
    let mut result = Vec::with_capacity(count);

    for i in 0..count {
        let offset = i * tables::MACRO_STEP_RECORD_SIZE;

        // A record that does not fully fit terminates the walk (matches C++).
        let Ok(record) = take(stream, offset, tables::MACRO_STEP_RECORD_SIZE) else {
            break;
        };

        let duration_ms = read_u16_le(record, tables::STEP_MS_TIME_OFFSET)?;
        let mut keys = read_u16_le(record, tables::STEP_KEYS_OFFSET)?;
        let trigger_value = read_u16_le(record, tables::STEP_TRIGGER_VALUE_OFFSET)?;

        let (trigger_left, trigger_right) = if is_xinput {
            // XInput: trigger_value = (L2 << 8) | R2.
            let left = u8::try_from((trigger_value >> 8) & 0xFF).unwrap_or(0);
            let right = u8::try_from(trigger_value & 0xFF).unwrap_or(0);
            (left, right)
        } else {
            // Switch: L2/R2 are keys bits 14–15 (full press or released).
            let left = if keys & tables::STEP_SWITCH_L2_MASK != 0 { 255 } else { 0 };
            let right = if keys & tables::STEP_SWITCH_R2_MASK != 0 { 255 } else { 0 };
            (left, right)
        };
        // Clear bits 14–15 before decoding button names (both modes).
        keys &= tables::STEP_BUTTON_BITS_MASK;

        let pressed_buttons = bitmask_to_step_button_names(keys);

        let left_joy = read_u16_le(record, tables::STEP_LEFT_JOY_OFFSET)?;
        let left_stick_x = u8::try_from(left_joy & 0xFF).unwrap_or(0);
        let left_stick_y = u8::try_from((left_joy >> 8) & 0xFF).unwrap_or(0);

        let right_joy = read_u16_le(record, tables::STEP_RIGHT_JOY_OFFSET)?;
        let right_stick_x = u8::try_from(right_joy & 0xFF).unwrap_or(0);
        let right_stick_y = u8::try_from((right_joy >> 8) & 0xFF).unwrap_or(0);

        result.push(MacroStep {
            duration_ms,
            pressed_buttons,
            left_stick_x,
            left_stick_y,
            right_stick_x,
            right_stick_y,
            trigger_left,
            trigger_right,
        });
    }

    Ok(result)
}

/// Serializes a [`MacroDefinition`] to canonical macro JSON matching
/// `schemas/macro-v1.schema.json`.
///
/// Top-level: `version:1, device:"8bitdo-pro3", mode, name, trigger,
/// repeat:{count,interval_ms}, steps:[...]`. Per step, `actions.buttons` is
/// ALWAYS emitted (with `press`+`release` arrays; the wire format only tracks
/// the currently-pressed set, so `release` is always empty); `left_stick`,
/// `right_stick` and `triggers` are OMITTED when at their defaults (stick
/// `127/127`, triggers `0/0`). Faithful port of `MacroDecoder::toJson`.
#[must_use]
pub fn macro_to_canonical_json(def: &MacroDefinition) -> Value {
    let steps: Vec<Value> = def.steps.iter().map(step_to_json).collect();

    json!({
        "version": 1,
        "device": "8bitdo-pro3",
        "mode": def.mode.as_str(),
        "name": def.name,
        "trigger": def.trigger,
        "repeat": {
            "count": def.repeat_count,
            "interval_ms": def.interval_ms,
        },
        "steps": steps,
    })
}

/// Serializes a single [`MacroStep`] to its canonical JSON object.
fn step_to_json(step: &MacroStep) -> Value {
    let mut actions = Map::new();

    // Buttons — always emitted; `release` mirrors the C++ (always empty).
    let press: Vec<Value> =
        step.pressed_buttons.iter().map(|name| Value::String(name.clone())).collect();
    actions.insert("buttons".to_owned(), json!({ "press": press, "release": Vec::<Value>::new() }));

    // Left stick — omitted when centered.
    if step.left_stick_x != tables::STICK_CENTER || step.left_stick_y != tables::STICK_CENTER {
        actions.insert(
            "left_stick".to_owned(),
            json!({ "x": step.left_stick_x, "y": step.left_stick_y }),
        );
    }

    // Right stick — omitted when centered.
    if step.right_stick_x != tables::STICK_CENTER || step.right_stick_y != tables::STICK_CENTER {
        actions.insert(
            "right_stick".to_owned(),
            json!({ "x": step.right_stick_x, "y": step.right_stick_y }),
        );
    }

    // Triggers — omitted when both released.
    if step.trigger_left != 0 || step.trigger_right != 0 {
        actions.insert(
            "triggers".to_owned(),
            json!({ "left": step.trigger_left, "right": step.trigger_right }),
        );
    }

    json!({
        "duration_ms": step.duration_ms,
        "actions": Value::Object(actions),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn bitmask_decodes_buttons_in_bit_order() {
        // top face (bit 4) + r1 (bit 11) => 0x0810.
        assert_eq!(bitmask_to_step_button_names(0x0810), vec!["top face", "r1"]);
        assert!(bitmask_to_step_button_names(0).is_empty());
    }

    #[test]
    fn key_map_reverses_to_trigger_name() {
        assert_eq!(key_map_to_trigger_name(0x0000_0400), "l1");
        assert_eq!(key_map_to_trigger_name(0x4000_0000), "r4");
        assert_eq!(key_map_to_trigger_name(0xDEAD_BEEF), "");
    }

    #[test]
    fn gamepad_byte_maps_mode() {
        assert_eq!(gamepad_byte_to_mode(3), Mode::XInput);
        assert_eq!(gamepad_byte_to_mode(0), Mode::Switch);
    }

    #[test]
    fn switch_mode_reads_triggers_from_keys_bits() {
        // keys = 0x4000 (L2 bit) only; trigger_value irrelevant in Switch.
        let mut rec = [0u8; 10];
        rec[2] = 0x00;
        rec[3] = 0x40; // keys = 0x4000 LE
        rec[6] = 127;
        rec[7] = 127;
        rec[8] = 127;
        rec[9] = 127;
        let steps = decode_macro_steps(&rec, 1, Mode::Switch).unwrap();
        assert_eq!(steps[0].trigger_left, 255);
        assert_eq!(steps[0].trigger_right, 0);
        assert!(steps[0].pressed_buttons.is_empty());
    }
}
