//! Decoding a slot of an Ultimate image into a canonical profile, the default profile of
//! each mode, and writing a profile's settings and button map back into a section.

use crate::error::{Error, Result};
use crate::model::macros::macro_file_name;
use crate::model::profile::canonical_id;
use crate::model::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, MacroRef, Mode, RawProfilePayload,
    Slot,
};
use crate::protocol::bytes::{put_slice, put_u32_le, read_u32_le, read_u8, take};

use super::macros::decode_macros;
use super::settings::{Settings, Sticks, Triggers, Vibration};
use super::tables::{
    DPAD_FOUR_WAY, FLAG_LEN, INVERT_LEFT_X, INVERT_LEFT_Y, INVERT_RIGHT_X, INVERT_RIGHT_Y, IN_USE,
    MAP_LEN, MAP_OFFSET, NAME_LEN, NAME_OFFSET, NO_DEAD_ZONE, RANGE_LEN, STICKS_OFFSET,
    STICK_RAW_MAX, SWAP_DPAD_WITH_LEFT_STICK, SWAP_RIGHT_STICK_WITH_TRIGGERS, SWAP_STICKS,
    SWAP_TRIGGERS, SWITCH_TRIGGER_START, TOGGLES_LEN, TOGGLES_OFFSET, TRIGGERS_OFFSET,
    TRIGGER_RAW_MAX, VIBRATION_LEN, VIBRATION_LEVELS, VIBRATION_LEVEL_MAX, VIBRATION_OFFSET,
};
use super::{check_blob, description, read_flag, slot_index, SWITCH, XINPUT};
use crate::description::{Button, DISABLED_OUTPUT as DISABLED};

/// The device discriminator every Ultimate profile carries.
pub const DEVICE: &str = "8bitdo-ultimate";
/// The schema kind every Ultimate profile carries.
pub const KIND: &str = "8bitdo.ultimate.profile";

/// Every toggle bit a profile holds. The other bits go back as read.
const MODELLED_TOGGLES: u32 = INVERT_LEFT_X
    | INVERT_LEFT_Y
    | INVERT_RIGHT_X
    | INVERT_RIGHT_Y
    | SWAP_STICKS
    | SWAP_TRIGGERS
    | SWAP_DPAD_WITH_LEFT_STICK
    | SWAP_RIGHT_STICK_WITH_TRIGGERS
    | NO_DEAD_ZONE
    | DPAD_FOUR_WAY;

/// The buttons that have an entry in the button map, in map order.
fn map_buttons() -> Result<impl Iterator<Item = (usize, &'static Button)>> {
    Ok(description()?.buttons.iter().filter_map(|b| Some((usize::from(b.map_index?), b))))
}

/// The output `code` sends in `mode`. A code that names nothing reads as `disabled`, as
/// the vendor app shows it.
fn output_name(mode: Mode, code: u32) -> Result<&'static str> {
    let d = description()?;
    let button = d.buttons.iter().find(|b| b.map_codes.get(&mode) == Some(&code));
    let extra = d.mode(mode).and_then(|m| m.extra_outputs.iter().find(|o| o.code == Some(code)));
    let name = button.map(|b| b.id.as_str()).or_else(|| extra.map(|o| o.id.as_str()));
    Ok(name.unwrap_or(DISABLED))
}

/// The code of output `name` in `mode`.
fn output_code(mode: Mode, name: &str) -> Result<u32> {
    let d = description()?;
    let button = d.button(name).and_then(|b| b.map_codes.get(&mode).copied());
    let extra = d.mode(mode).and_then(|m| m.extra_outputs.iter().find(|o| o.id == name));
    (name == DISABLED)
        .then_some(0)
        .or(button)
        .or_else(|| extra.and_then(|o| o.code))
        .ok_or_else(|| Error::Validation(format!("unknown output '{name}' in {mode} mode")))
}

/// Converts a raw range end to 0..=100, rounding half up.
fn to_percent(raw: u8, raw_max: i32) -> i32 {
    (i32::from(raw) * 100 + raw_max / 2) / raw_max
}

/// Converts `pct` to a raw range end, rounding half up, but keeps `base` when `base`
/// already reads as `pct`: several raw values share one percent, and an untouched value
/// must go back as read.
fn keep_or_encode(base: u8, pct: i32, raw_max: i32) -> u8 {
    if to_percent(base, raw_max) == pct {
        return base;
    }
    let raw = (pct.clamp(0, 100) * raw_max + 50) / 100;
    u8::try_from(raw).unwrap_or(u8::MAX)
}

/// The level of a stored motor strength, 0 for a value that is not a number.
fn level(strength: f32) -> i32 {
    let level = (strength * VIBRATION_LEVELS).round();
    if level.is_finite() {
        // Safe: clamped into 0..=5 first.
        #[allow(clippy::cast_possible_truncation)]
        let level = level.clamp(0.0, VIBRATION_LEVELS) as i32;
        level
    } else {
        0
    }
}

/// The section of `len` bytes at `base + slot * len`, or `None` while its flag is not
/// in use.
fn section(blob: &[u8], base: usize, len: usize, s: usize) -> Result<Option<&[u8]>> {
    let at = base + s * len;
    (read_flag(blob, at)? == IN_USE).then(|| take(blob, at, len)).transpose()
}

/// The profile of slot `slot` in `blob`, an image of `mode`. Each section whose flag is
/// not in use reads as the mode's default for that section.
fn decode(blob: &[u8], slot: Slot, mode: Mode) -> Result<CanonicalProfile> {
    check_blob(blob)?;
    let s = slot_index(slot)?;
    let default = default_profile(mode);
    let mut settings = Settings::of(&default)?;
    let bits = match section(blob, TOGGLES_OFFSET, TOGGLES_LEN, s)? {
        Some(t) => read_u32_le(t, FLAG_LEN)?,
        None => 0,
    };
    let on = |bit: u32| bits & bit != 0;
    if let Some(r) = section(blob, STICKS_OFFSET, RANGE_LEN, s)? {
        let pct = |i: usize| Ok::<_, Error>(to_percent(read_u8(r, FLAG_LEN + i)?, STICK_RAW_MAX));
        settings.sticks.left_min_pct = pct(0)?;
        settings.sticks.left_max_pct = pct(1)?;
        settings.sticks.right_min_pct = pct(2)?;
        settings.sticks.right_max_pct = pct(3)?;
    }
    settings.sticks = Sticks {
        invert_left_x: on(INVERT_LEFT_X),
        invert_left_y: on(INVERT_LEFT_Y),
        invert_right_x: on(INVERT_RIGHT_X),
        invert_right_y: on(INVERT_RIGHT_Y),
        swap_sticks: on(SWAP_STICKS),
        swap_dpad_with_left_stick: on(SWAP_DPAD_WITH_LEFT_STICK),
        swap_right_stick_with_triggers: on(SWAP_RIGHT_STICK_WITH_TRIGGERS),
        no_dead_zone: on(NO_DEAD_ZONE),
        dpad_four_way: on(DPAD_FOUR_WAY),
        ..settings.sticks
    };
    if let Some(r) = section(blob, TRIGGERS_OFFSET, RANGE_LEN, s)? {
        let pct = |i: usize| Ok::<_, Error>(to_percent(read_u8(r, FLAG_LEN + i)?, TRIGGER_RAW_MAX));
        settings.triggers = Triggers {
            left_min_pct: pct(0)?,
            left_max_pct: pct(1)?,
            right_min_pct: pct(2)?,
            right_max_pct: pct(3)?,
            ..settings.triggers
        };
    }
    settings.triggers.swap_triggers = on(SWAP_TRIGGERS);
    if let Some(v) = section(blob, VIBRATION_OFFSET, VIBRATION_LEN, s)? {
        let strength = |at: usize| Ok::<_, Error>(f32::from_bits(read_u32_le(v, at)?));
        settings.vibration = Vibration {
            left_level: level(strength(FLAG_LEN)?),
            right_level: level(strength(FLAG_LEN + 4)?),
        };
    }
    let button_mappings = match section(blob, MAP_OFFSET, MAP_LEN, s)? {
        None => default.button_mappings,
        Some(map) => map_buttons()?
            .map(|(i, button)| {
                let code = read_u32_le(map, FLAG_LEN + 4 * i)?;
                Ok(ButtonMapping {
                    source: button.id.clone(),
                    target: output_name(mode, code)?.to_owned(),
                })
            })
            .collect::<Result<_>>()?,
    };
    let macro_refs = decode_macros(blob, slot)?
        .into_iter()
        .map(|def| MacroRef {
            path: macro_file_name(mode, slot.get(), &def),
            trigger_buttons: def.trigger_buttons,
        })
        .collect();
    Ok(CanonicalProfile {
        name: decode_name(take(blob, NAME_OFFSET + s * NAME_LEN, NAME_LEN)?),
        settings: settings.into_map(),
        button_mappings,
        macro_refs,
        ..default
    })
}

/// A UTF-16LE name up to its first NUL. A blank field, all `00` or all `ff`, is empty.
fn decode_name(field: &[u8]) -> String {
    if field.iter().all(|&b| b == 0xFF) {
        return String::new();
    }
    let units: Vec<u16> = field
        .chunks_exact(2)
        .map(|p| {
            u16::from_le_bytes([p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0)])
        })
        .take_while(|&u| u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// Decodes slot `raw.source_slot` of the image `raw.payload`, read in `raw.mode_hint`.
///
/// # Errors
/// Returns [`Error::Decode`] if the payload is not a full image, and
/// [`Error::Validation`] for a slot past the third.
pub fn map_profile(raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
    let slot = Slot::new(raw.source_slot)?;
    let mode = raw.mode_hint;
    let id = canonical_id(mode, raw.source_slot, raw.source_profile_index);
    let canonical = CanonicalProfile { id: id.clone(), ..decode(&raw.payload, slot, mode)? };
    Ok(CanonicalProfileSummary {
        id,
        name: canonical.name.clone(),
        mode,
        source_slot: raw.source_slot,
        source_profile_index: raw.source_profile_index,
        canonical,
    })
}

/// The profile a new slot of `mode` starts from, with an empty name: the vendor app's
/// defaults.
///
/// Every button sends its own output, except Star (Turbo in `XInput`, Screenshot in
/// Switch) and P1 and P2 (nothing). Full stick and trigger ranges, except that a Switch
/// trigger starts at about 30%; both motors at full strength.
#[must_use]
pub fn default_profile(mode: Mode) -> CanonicalProfile {
    let star = if mode == SWITCH { "screenshot" } else { "turbo" };
    let button_mappings = map_buttons()
        .into_iter()
        .flatten()
        .map(|(_, button)| {
            let target = match button.id.as_str() {
                "star" => star,
                "p1" | "p2" => DISABLED,
                own => own,
            };
            ButtonMapping { source: button.id.clone(), target: target.to_owned() }
        })
        .collect();
    let start = if mode == XINPUT { 0 } else { SWITCH_TRIGGER_START };
    let settings = Settings {
        sticks: Sticks {
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
            swap_right_stick_with_triggers: false,
            no_dead_zone: false,
            dpad_four_way: false,
        },
        triggers: Triggers {
            left_min_pct: start,
            left_max_pct: 100,
            right_min_pct: start,
            right_max_pct: 100,
            swap_triggers: false,
        },
        vibration: Vibration { left_level: VIBRATION_LEVEL_MAX, right_level: VIBRATION_LEVEL_MAX },
    };
    CanonicalProfile {
        id: String::new(),
        name: String::new(),
        version: 1,
        kind: KIND.to_owned(),
        device: DEVICE.to_owned(),
        mode,
        preferred_slot: None,
        settings: settings.into_map(),
        button_mappings,
        macro_refs: Vec::new(),
        macros: None,
    }
}

/// One settings section of a slot.
#[derive(Debug, Clone, Copy)]
pub(super) enum Section {
    /// Motor strengths.
    Vibration,
    /// Stick ranges.
    Sticks,
    /// Trigger ranges.
    Triggers,
    /// Toggle bits.
    Toggles,
    /// Button map.
    Map,
}

impl Section {
    /// Every settings section.
    pub(super) const ALL: [Self; 5] =
        [Self::Vibration, Self::Sticks, Self::Triggers, Self::Toggles, Self::Map];

    /// Where this section of the first slot starts, and its length, which is also the
    /// stride between slots.
    pub(super) const fn place(self) -> (usize, usize) {
        match self {
            Self::Vibration => (VIBRATION_OFFSET, VIBRATION_LEN),
            Self::Sticks => (STICKS_OFFSET, RANGE_LEN),
            Self::Triggers => (TRIGGERS_OFFSET, RANGE_LEN),
            Self::Toggles => (TOGGLES_OFFSET, TOGGLES_LEN),
            Self::Map => (MAP_OFFSET, MAP_LEN),
        }
    }

    /// Writes `profile`'s values for this section into `bytes`, the section with its
    /// flag. A value that `bytes` already holds stays byte for byte, and so do toggle
    /// bits no profile holds and map entries the profile leaves out.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if the profile's settings do not have the
    /// Ultimate's shape or a button sends an unknown output.
    pub(super) fn write(self, bytes: &mut [u8], profile: &CanonicalProfile) -> Result<()> {
        let s = Settings::of(profile)?;
        let ranges = |bytes: &mut [u8], pcts: [i32; 4], raw_max: i32| -> Result<()> {
            for (i, pct) in pcts.into_iter().enumerate() {
                let at = FLAG_LEN + i;
                let raw = keep_or_encode(read_u8(bytes, at)?, pct, raw_max);
                put_slice(bytes, at, &[raw])?;
            }
            Ok(())
        };
        match self {
            Self::Vibration => {
                for (at, wanted) in
                    [(FLAG_LEN, s.vibration.left_level), (FLAG_LEN + 4, s.vibration.right_level)]
                {
                    let old = f32::from_bits(read_u32_le(bytes, at)?);
                    if level(old) != wanted || !old.is_finite() {
                        let strength = f32::from(
                            u8::try_from(wanted.clamp(0, VIBRATION_LEVEL_MAX)).unwrap_or(0),
                        ) / VIBRATION_LEVELS;
                        put_u32_le(bytes, at, strength.to_bits())?;
                    }
                }
            }
            Self::Sticks => {
                let t = &s.sticks;
                ranges(
                    bytes,
                    [t.left_min_pct, t.left_max_pct, t.right_min_pct, t.right_max_pct],
                    STICK_RAW_MAX,
                )?;
            }
            Self::Triggers => {
                let t = &s.triggers;
                ranges(
                    bytes,
                    [t.left_min_pct, t.left_max_pct, t.right_min_pct, t.right_max_pct],
                    TRIGGER_RAW_MAX,
                )?;
            }
            Self::Toggles => {
                let (t, swap) = (&s.sticks, s.triggers.swap_triggers);
                let bits = [
                    (t.invert_left_x, INVERT_LEFT_X),
                    (t.invert_left_y, INVERT_LEFT_Y),
                    (t.invert_right_x, INVERT_RIGHT_X),
                    (t.invert_right_y, INVERT_RIGHT_Y),
                    (t.swap_sticks, SWAP_STICKS),
                    (swap, SWAP_TRIGGERS),
                    (t.swap_dpad_with_left_stick, SWAP_DPAD_WITH_LEFT_STICK),
                    (t.swap_right_stick_with_triggers, SWAP_RIGHT_STICK_WITH_TRIGGERS),
                    (t.no_dead_zone, NO_DEAD_ZONE),
                    (t.dpad_four_way, DPAD_FOUR_WAY),
                ];
                let set = bits.iter().filter(|(on, _)| *on).fold(0, |acc, (_, bit)| acc | bit);
                let kept = read_u32_le(bytes, FLAG_LEN)? & !MODELLED_TOGGLES;
                put_u32_le(bytes, FLAG_LEN, kept | set)?;
            }
            Self::Map => {
                for (i, button) in map_buttons()? {
                    let Some(m) = profile.button_mappings.iter().find(|m| m.source == button.id)
                    else {
                        continue;
                    };
                    put_u32_le(bytes, FLAG_LEN + 4 * i, output_code(profile.mode, &m.target)?)?;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn percent_rounds_half_up_and_keeps_a_read_value() {
        assert_eq!(to_percent(63, STICK_RAW_MAX), 49);
        assert_eq!(to_percent(64, STICK_RAW_MAX), 50);
        assert_eq!(to_percent(77, TRIGGER_RAW_MAX), 30);
        assert_eq!(keep_or_encode(0, 30, TRIGGER_RAW_MAX), 77);
        assert_eq!(keep_or_encode(76, 30, TRIGGER_RAW_MAX), 76, "76 also reads as 30%");
        assert_eq!(keep_or_encode(0, 100, STICK_RAW_MAX), 128);
    }

    #[test]
    fn levels_are_fifths_and_nan_is_off() {
        assert_eq!(level(0.4), 2);
        assert_eq!(level(1.0), 5);
        assert_eq!(level(f32::NAN), 0);
        assert_eq!(
            (2.0f32 / VIBRATION_LEVELS).to_bits(),
            0x3ECC_CCCD,
            "0.4 as the vendor app stores it"
        );
    }

    #[test]
    fn face_outputs_differ_by_mode_and_unknown_codes_read_as_disabled() {
        assert_eq!(output_name(XINPUT, 0x2000).unwrap(), "bottom face");
        assert_eq!(output_name(SWITCH, 0x2000).unwrap(), "right face");
        assert_eq!(output_name(XINPUT, 0xFFFF_FFFF).unwrap(), DISABLED);
        assert_eq!(output_code(SWITCH, "screenshot").unwrap(), 0x40_0000);
        assert!(output_code(XINPUT, "star").is_err(), "Star is never an output");
    }
}
