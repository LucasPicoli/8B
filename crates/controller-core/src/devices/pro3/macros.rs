//! Pro 3 macro decoder — `decode_macro_metadata`, `decode_macro_steps`, and the
//! canonical-JSON serializer.
//!
//! Verified byte for byte against golden vectors (`tests/pro3/golden_macro_decode.rs`).
//!
//! All variable-offset reads go through the bounds-checked
//! [`crate::protocol::bytes`] accessors so the decoder is panic-free even on
//! truncated or corrupted input.

use serde_json::Value;

use crate::devices::pro3::{tables, DINPUT, SWITCH, XINPUT};
use crate::error::{Error, Result};
use crate::model::macros::macro_to_json;
use crate::model::{MacroDefinition, MacroSlot, MacroStep, Mode, Slot};
use crate::protocol::bytes::{
    put_slice, put_u16_le, put_u32_le, read_u16_le, read_u32_le, read_u8, take,
};
use crate::protocol::text::encode_utf16be_name;

/// Converts a 16-bit step-button bitmask to canonical names, ordered by bit
/// position.
fn bitmask_to_step_button_names(keys: u16) -> Vec<String> {
    tables::STEP_BUTTONS
        .iter()
        .filter(|entry| keys & entry.mask != 0)
        .map(|entry| entry.name.to_owned())
        .collect()
}

/// Converts a 32-bit `KeyMap` value to its canonical trigger name, or `""` when
/// it matches no single-button trigger.
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
/// stream).
///
/// # Errors
/// Returns [`crate::Error::Decode`] only via the bounds-checked readers; in
/// practice a truncated descriptor simply terminates the scan.
pub fn decode_macro_metadata(blob: &[u8], profile_slot: Slot) -> Result<Vec<MacroDefinition>> {
    let mut result = Vec::new();

    let slot_index = super::slot_index(profile_slot)?;
    let slot_base = tables::SECTION4_BASE_OFFSET
        + (slot_index * tables::SECTION4_SLOT_STRIDE)
        + tables::SECTION4_RECORD_HEADER_SIZE;

    for macro_index in 0..tables::MACRO_SLOTS_PER_PROFILE {
        let descriptor_offset = slot_base + (macro_index * tables::MACRO_DESCRIPTOR_SIZE);

        // A descriptor that does not fully fit terminates the scan.
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
            // many steps to read from flash.
            steps: vec![MacroStep::default(); usize::from(max_steps)],
            macro_slot,
        });
    }

    Ok(result)
}

/// Maps the `gamepad_mode` descriptor byte to a [`Mode`] (`3` → `XInput`, `1` → `DInput`,
/// else `Switch`).
const fn gamepad_byte_to_mode(byte: u8) -> Mode {
    match byte {
        tables::MACRO_GAMEPAD_MODE_XINPUT => XINPUT,
        tables::MACRO_GAMEPAD_MODE_DINPUT => DINPUT,
        _ => SWITCH,
    }
}

/// Decodes `count` × 10-byte step records from a raw step stream.
///
/// Each record is `ms_time` LE16, `keys` LE16, `trigger_value` LE16, `left_joy`
/// LE16 (`(Y<<8)|X`), `right_joy` LE16. L2/R2 decode is mode-aware: in `XInput`
/// they come from `trigger_value` (`(L2<<8)|R2`) and the top two `keys` bits are
/// cleared; in Switch they are the `keys` bits 14 to 15 (0 or 255).
///
/// # Errors
/// Returns [`crate::Error::Decode`] only via the bounds-checked readers; a
/// record that does not fully fit terminates the walk.
pub fn decode_macro_steps(stream: &[u8], count: usize, mode: Mode) -> Result<Vec<MacroStep>> {
    let is_xinput = mode == XINPUT;
    let mut result = Vec::with_capacity(count);

    for i in 0..count {
        let offset = i * tables::MACRO_STEP_RECORD_SIZE;

        // A record that does not fully fit terminates the walk.
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

// ---------------------------------------------------------------------------
// Encoder helpers (reverse lookups over the same tables the decoder reads).
// ---------------------------------------------------------------------------

/// Returns the 16-bit step-button bitmask for a canonical name.
///
/// # Errors
/// Returns [`Error::Validation`] if `name` is not in [`tables::STEP_BUTTONS`].
fn step_button_mask(name: &str) -> Result<u16> {
    tables::STEP_BUTTONS
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.mask)
        .ok_or_else(|| Error::Validation(format!("unknown step button: {name}")))
}

/// Returns the 32-bit `KeyMap` value for a canonical trigger name.
///
/// # Errors
/// Returns [`Error::Validation`] if `name` is not in [`tables::TRIGGERS`].
fn trigger_key_map(name: &str) -> Result<u32> {
    tables::TRIGGERS
        .iter()
        .find(|e| e.name == name)
        .map(|e| e.key_map)
        .ok_or_else(|| Error::Validation(format!("unknown macro trigger: {name}")))
}

/// Maps a [`Mode`] to the `gamepad_mode` descriptor byte.
fn macro_mode_byte(mode: Mode) -> u8 {
    if mode == XINPUT {
        tables::MACRO_GAMEPAD_MODE_XINPUT
    } else {
        0
    }
}

/// Encodes `steps` into the padded wire step stream.
///
/// Each step is encoded as a 10-byte `record_content_t` (all fields LE16).
/// The output is zero-padded to the next 32-byte boundary; empty input
/// produces an empty `Vec`.
///
/// # Errors
/// Returns [`Error::Validation`] if any `step.pressed_buttons` entry is unknown.
pub fn encode_macro_steps(steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
    if steps.is_empty() {
        return Ok(Vec::new());
    }

    let raw_len = steps.len() * tables::MACRO_STEP_RECORD_SIZE;
    let padded_len = if raw_len % 32 == 0 { raw_len } else { (raw_len / 32 + 1) * 32 };
    let mut out = vec![0u8; padded_len];

    let is_xinput = mode == XINPUT;

    for (i, step) in steps.iter().enumerate() {
        let base = i * tables::MACRO_STEP_RECORD_SIZE;

        // Build button bitmask from canonical names.
        let mut keys: u16 = 0;
        for name in &step.pressed_buttons {
            keys |= step_button_mask(name)?;
        }

        // Mode-specific L2/R2 routing.
        let trigger_value: u16 = if is_xinput {
            // XInput: trigger_value carries (L2<<8)|R2; clear bits 14–15 in keys.
            keys &= tables::STEP_BUTTON_BITS_MASK;
            (u16::from(step.trigger_left) << 8) | u16::from(step.trigger_right)
        } else {
            // Switch: encode L2/R2 as bits 14–15 in keys; trigger_value unused.
            if step.trigger_left > 0 {
                keys |= tables::STEP_SWITCH_L2_MASK;
            }
            if step.trigger_right > 0 {
                keys |= tables::STEP_SWITCH_R2_MASK;
            }
            0
        };

        let left_joy: u16 = (u16::from(step.left_stick_y) << 8) | u16::from(step.left_stick_x);
        let right_joy: u16 = (u16::from(step.right_stick_y) << 8) | u16::from(step.right_stick_x);

        put_u16_le(&mut out, base + tables::STEP_MS_TIME_OFFSET, step.duration_ms)?;
        put_u16_le(&mut out, base + tables::STEP_KEYS_OFFSET, keys)?;
        put_u16_le(&mut out, base + tables::STEP_TRIGGER_VALUE_OFFSET, trigger_value)?;
        put_u16_le(&mut out, base + tables::STEP_LEFT_JOY_OFFSET, left_joy)?;
        put_u16_le(&mut out, base + tables::STEP_RIGHT_JOY_OFFSET, right_joy)?;
    }

    Ok(out)
}

/// Encodes a macro's 52-byte Section-4 metadata descriptor (`record_macro_content_t`).
///
/// The descriptor is zero-initialized then individual fields are written at their
/// canonical offsets.
///
/// # Errors
/// Returns [`Error::Validation`] if `def.trigger` is unknown or if
/// `def.steps.len()` overflows `u16`, or if the `macro_slot` offset overflows `u16`.
pub fn encode_macro_metadata(def: &MacroDefinition, macro_slot: MacroSlot) -> Result<Vec<u8>> {
    let mut out = vec![0u8; tables::MACRO_DESCRIPTOR_SIZE];

    // [0..32] UTF-16BE name, truncated/padded to MACRO_NAME_BYTES.
    let name_bytes = encode_utf16be_name(&def.name, tables::MACRO_NAME_BYTES);
    put_slice(&mut out, 0, &name_bytes)?;

    // [32] gamepad_mode; [33] stays 0.
    out.get_mut(tables::MACRO_MODE_OFFSET)
        .map(|b| *b = macro_mode_byte(def.mode))
        .ok_or_else(|| Error::Decode("macro descriptor too small for mode byte".into()))?;

    // [34..36] max_steps LE16.
    let max_steps = u16::try_from(def.steps.len())
        .map_err(|_| Error::Validation("step count overflows u16".into()))?;
    put_u16_le(&mut out, tables::MACRO_MAX_STEPS_OFFSET, max_steps)?;

    // [36..38] offset LE16 = macro_slot_index * 4096.
    let offset_u32 = u32::from(macro_slot.get()) * 4096;
    let offset = u16::try_from(offset_u32)
        .map_err(|_| Error::Validation("macro slot offset overflows u16".into()))?;
    put_u16_le(&mut out, tables::MACRO_OFFSET_FIELD_OFFSET, offset)?;

    // [40..44] key_map LE32.
    let key_map = trigger_key_map(&def.trigger)?;
    put_u32_le(&mut out, tables::MACRO_KEY_MAP_OFFSET, key_map)?;

    // [44..48] cycles_num LE32.
    put_u32_le(&mut out, tables::MACRO_REPEAT_COUNT_OFFSET, def.repeat_count)?;

    // [48..52] interval_ms LE32.
    put_u32_le(&mut out, tables::MACRO_INTERVAL_MS_OFFSET, def.interval_ms)?;

    Ok(out)
}

/// Serializes a [`MacroDefinition`] to canonical Pro 3 macro JSON, matching
/// `schemas/macro-v1.schema.json`: [`crate::model::macros::macro_to_json`] with the
/// device `8bitdo-pro3`.
#[must_use]
pub fn macro_to_canonical_json(def: &MacroDefinition) -> Value {
    macro_to_json(def, "8bitdo-pro3")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::devices::pro3::{SWITCH, XINPUT};

    #[test]
    fn bitmask_decodes_buttons_in_bit_order() {
        // top face (bit 4) + r1 (bit 11) => 0x0810.
        assert_eq!(bitmask_to_step_button_names(0x0810), vec!["top face", "r1"]);
        assert_eq!(bitmask_to_step_button_names(0), Vec::<String>::new());
    }

    #[test]
    fn key_map_reverses_to_trigger_name() {
        assert_eq!(key_map_to_trigger_name(0x0000_0400), "l1");
        assert_eq!(key_map_to_trigger_name(0x4000_0000), "r4");
        assert_eq!(key_map_to_trigger_name(0xDEAD_BEEF), "");
    }

    #[test]
    fn gamepad_byte_maps_mode() {
        assert_eq!(gamepad_byte_to_mode(3), XINPUT);
        assert_eq!(gamepad_byte_to_mode(0), SWITCH);
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
        let steps = decode_macro_steps(&rec, 1, SWITCH).unwrap();
        assert_eq!(steps[0].trigger_left, 255);
        assert_eq!(steps[0].trigger_right, 0);
        assert_eq!(steps[0].pressed_buttons, Vec::<String>::new());
    }
}
