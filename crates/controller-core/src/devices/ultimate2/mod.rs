//! 8BitDo Ultimate 2 Wireless controller backend.
//!
//! The Ultimate 2 speaks the Pro 3 protocol and keeps the Pro 3 image layout, less the
//! Pro 3's 1176-byte legacy macro section and plus 420 bytes of motion and light
//! settings at the end. Its faces carry Xbox letters where the Pro 3's carry Nintendo
//! ones, which moves face entries and codes (see `translate`). The codec widens an
//! Ultimate 2 image to the Pro 3 layout, runs the Pro 3 codec on it, and narrows the
//! result back.

use std::sync::LazyLock;
use std::time::Duration;

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, ProtocolCodec, RawRequest};
use crate::devices::pro3::{self, Pro3, DINPUT, XINPUT};
use crate::error::Result;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::bytes::{
    put_slice, put_u16_le, put_u32_le, read_u16_le, read_u32_le, read_u8, take,
};
use crate::protocol::wire_write::PACKET_LEN;

mod lights;
mod motion;

/// Ultimate 2 image size in bytes (`0x0638`).
pub const BLOB_SIZE: usize = 0x0638;
/// End of the head both layouts share: flags, names, settings and the button map.
const SHARED_HEAD_END: usize = 0x01F4;
/// Length of the Pro 3's legacy macro section, which the Ultimate 2 lacks.
const LEGACY_LEN: usize = 0x0498;
/// Start of the macro records and motor ranges in a Pro 3 image.
const PRO3_TAIL_START: usize = SHARED_HEAD_END + LEGACY_LEN;
/// End of the macro records and motor ranges in an Ultimate 2 image. The motion and
/// light settings follow, up to [`BLOB_SIZE`].
const TAIL_END: usize = SHARED_HEAD_END + (pro3::tables::EXPECTED_PROFILE_SIZE - PRO3_TAIL_START);

/// The face-button map entries that trade places with the entry after them between
/// the two layouts. A swap is its own inverse, so it serves both directions.
const FACE_SWAPS: [usize; 2] = [0, 2];

/// Letter codes that trade places between the two pads: A with B, X with Y.
const LETTER_SWAPS: [(u32, u32); 2] = [(0x2000, 0x1000), (0x10, 0x20)];

/// The vendor app waits this long between the last chunk and the apply, on the
/// Ultimate 2 only.
const APPLY_DELAY: Duration = Duration::from_millis(100);

/// Offset of the mode the bank belongs to, a little-endian `u16`.
const MODE_OFFSET: usize = 0x0010;

/// Offset of the macro count in a slot's macro record: the number of macros with
/// steps, a byte. The Pro 3 code writes its macro slot count there instead.
const MACRO_COUNT_OFFSET: usize = 4;

/// Each per-slot section of the image: its first slot's offset, the stride between
/// slots and its length. Flag, name, vibration, sticks, triggers, options, button
/// map, macro record, motor range, motion, and the three light themes.
const SLOT_SECTIONS: [(usize, usize, usize); 13] = [
    (0x000, 4, 4),
    (0x014, 32, 32),
    (0x074, 12, 12),
    (0x098, 8, 8),
    (0x0B0, 8, 8),
    (0x0C8, 8, 8),
    (0x0E0, 0x5C, 0x5C),
    (0x1F4, 216, 216),
    (0x47C, 8, 8),
    (0x494, 12, 12),
    (0x4B8, 12, 12),
    (0x4DC, 16, 16),
    (0x50C, 100, 100),
];

/// `base` with every section of `slot` zeroed when the slot holds no profile. A bank
/// the pad never saved is erased flash, and the Pro 3 code keeps a base byte that
/// decodes to the value it would write, so `ff` would survive as a stick end of 255 and
/// as option bits nobody set.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `base` is a full image cut short.
fn clear_unused(base: &[u8], slot: Slot) -> Result<Vec<u8>> {
    let mut out = base.to_vec();
    if base.len() != BLOB_SIZE || Pro3.slot_active(base, slot)? {
        return Ok(out);
    }
    let index = usize::from(slot.get()) - 1;
    for (first, stride, len) in SLOT_SECTIONS {
        put_slice(&mut out, first + index * stride, &vec![0; len])?;
    }
    Ok(out)
}

/// The request that asks the pad its mode, which its USB id does not tell.
const MODE_REQUEST: [u8; 4] = [0x81, 0x04, 0x05, 0x01];
/// The first two bytes of the reply to [`MODE_REQUEST`].
const MODE_REPLY_HEAD: [u8; 2] = [0x02, 0x04];
/// Where the reply to [`MODE_REQUEST`] names the mode.
const MODE_REPLY_OFFSET: usize = 18;
/// The reply's value for `XInput`, seen on the pad.
const MODE_REPLY_XINPUT: u8 = 1;
/// The reply's value for `DInput`, seen on the pad.
const MODE_REPLY_DINPUT: u8 = 0;

/// The back paddles and extra shoulder buttons. None has an output of its own on the
/// Ultimate 2: the vendor app stores `0` for one left as it is, in every mode, and a
/// macro on it plays only then. The Pro 3 code writes its own code instead, which made
/// L4 send LB and not start its macro on the pad.
const PADDLES: [&str; 4] = ["rp", "lp", "l4", "r4"];

/// `profile` with each paddle that the Pro 3 code reads as `disabled`, or as its Pro 3
/// output, read as itself: on the Ultimate 2 all of them mean the paddle is left as it is.
fn paddles_as_themselves(profile: &mut CanonicalProfile) {
    for m in &mut profile.button_mappings {
        let src = m.source.as_str();
        let leftover = m.target == "disabled" || m.target.strip_suffix(" output") == Some(src);
        if PADDLES.contains(&src) && leftover {
            m.target.clone_from(&m.source);
        }
    }
}

/// `profile` for the Pro 3 code, with each paddle left as itself sent to `disabled`,
/// which it writes as `0`.
fn paddles_for_pro3(profile: &CanonicalProfile) -> CanonicalProfile {
    let mut out = profile.clone();
    for m in &mut out.button_mappings {
        if PADDLES.contains(&m.source.as_str()) && m.target == m.source {
            "disabled".clone_into(&mut m.target);
        }
    }
    out
}

/// The `device` every Ultimate 2 profile carries.
const DEVICE: &str = "8bitdo-ultimate-2";
/// The `kind` every Ultimate 2 profile carries.
const KIND: &str = "8bitdo.ultimate-2.profile";

/// The 8BitDo Ultimate 2 Wireless controller backend.
#[derive(Debug, Clone, Copy, Default)]
pub struct Ultimate2;

/// The Ultimate 2 controller description, parsed on first use.
static DESCRIPTION: LazyLock<Result<ControllerDescription>> = LazyLock::new(|| {
    ControllerDescription::parse(
        include_str!("../../../controllers/ultimate-2/description.json"),
        &[
            ("front.svg", include_str!("../../../controllers/ultimate-2/front.svg")),
            ("back.svg", include_str!("../../../controllers/ultimate-2/back.svg")),
        ],
    )
});

/// The mode whose bank `blob` is, from the mode it records.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `blob` is too short or records a mode the
/// description does not list. The pad serves the bank of the mode it is in, whatever
/// mode the slot select names, and keeps `2dc8:310b` in every mode, so only the bank
/// tells.
fn bank_mode(blob: &[u8]) -> Result<Mode> {
    let recorded = read_u16_le(blob, MODE_OFFSET)?;
    for mode in Ultimate2.description()?.modes.iter().map(|m| m.id) {
        if u16::from(Pro3.slot_select_value(mode)?) == recorded {
            return Ok(mode);
        }
    }
    Err(crate::Error::Decode(format!(
        "the pad is in a mode 8B does not support (mode byte {recorded}): switch it to XInput"
    )))
}

/// Refuses `blob`, a full image, unless it is the bank of `mode`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if it records another mode or one not listed.
fn check_bank(blob: &[u8], mode: Mode) -> Result<()> {
    match bank_mode(blob)? {
        found if blob.len() != BLOB_SIZE || found == mode => Ok(()),
        found => Err(crate::Error::Decode(format!(
            "the pad is in {found} mode: switch it to {mode} mode first"
        ))),
    }
}

/// `blob` in the Pro 3 layout. A blob of another size, such as the empty base of a new
/// write, passes through, and the Pro 3 codec treats it as it would its own.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `blob` records a mode the description does not list.
fn widen(blob: &[u8]) -> Result<Vec<u8>> {
    if blob.len() != BLOB_SIZE {
        return Ok(blob.to_vec());
    }
    let mut own = blob.to_vec();
    translate(&mut own)?;
    let (head, rest) = own.split_at(SHARED_HEAD_END);
    let tail = rest.get(..TAIL_END - SHARED_HEAD_END).unwrap_or_default();
    Ok([head, &[0; LEGACY_LEN], tail].concat())
}

/// `wide`, a Pro 3 layout image, back in the Ultimate 2 layout, with the motion and
/// light settings of `base` (zero when `base` is not a full image), CRC resealed.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `wide` is not a full Pro 3 layout image.
fn narrow(wide: &[u8], base: &[u8]) -> Result<Vec<u8>> {
    let short = || crate::Error::Decode(format!("expected a {}-byte image", wide.len()));
    let head = wide.get(..SHARED_HEAD_END).ok_or_else(short)?;
    let tail = wide.get(PRO3_TAIL_START..pro3::tables::EXPECTED_PROFILE_SIZE).ok_or_else(short)?;
    let extra = match base.get(TAIL_END..) {
        Some(extra) if base.len() == BLOB_SIZE => extra.to_vec(),
        _ => vec![0; BLOB_SIZE - TAIL_END],
    };
    let mut out = [head, tail, &extra].concat();
    translate(&mut out)?;
    count_macros(&mut out)?;
    pro3::profile::seal_crc(&mut out)?;
    Ok(out)
}

/// Turns an Ultimate 2 image into what the Pro 3 codec expects of the same bytes, or
/// back: each step is its own inverse.
///
/// 1. The button map lists the faces bottom, right, left, top, where the Pro 3 lists
///    right, bottom, top, left: [`FACE_SWAPS`].
/// 2. Macro triggers name a face by its printed letter, and A, B, X and Y sit on other
///    faces than on the Pro 3: [`swap_letters`]. In `DInput` the button map does too.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `blob` is short or records a mode not listed.
fn translate(blob: &mut [u8]) -> Result<()> {
    use pro3::tables::{
        BUTTON_ENTRY_BYTES, BUTTON_MAP_BASE_OFFSET, BUTTON_MAP_SLOT_STRIDE, MACRO_DESCRIPTOR_SIZE,
        MACRO_KEY_MAP_OFFSET, MACRO_SLOTS_PER_PROFILE, SECTION4_RECORD_HEADER_SIZE,
        SECTION4_SLOT_STRIDE, SOURCE_BUTTON_COUNT,
    };
    let letters_in_map = bank_mode(blob)? == DINPUT;
    for slot in 0..usize::from(pro3::tables::PROFILE_SLOT_COUNT) {
        let map = BUTTON_MAP_BASE_OFFSET + slot * BUTTON_MAP_SLOT_STRIDE;
        for index in FACE_SWAPS {
            let start = map + index * BUTTON_ENTRY_BYTES;
            let pair = blob
                .get_mut(start..start + 2 * BUTTON_ENTRY_BYTES)
                .ok_or_else(|| crate::Error::Decode("image too short".to_owned()))?;
            let (first, second) = pair.split_at_mut(BUTTON_ENTRY_BYTES);
            first.swap_with_slice(second);
        }
        if letters_in_map {
            for index in 0..SOURCE_BUTTON_COUNT {
                let at = map + index * BUTTON_ENTRY_BYTES;
                let code = swap_letters(read_u32_le(blob, at)?);
                put_u32_le(blob, at, code)?;
            }
        }
        let record = SHARED_HEAD_END + slot * SECTION4_SLOT_STRIDE + SECTION4_RECORD_HEADER_SIZE;
        for index in 0..MACRO_SLOTS_PER_PROFILE {
            let at = record + index * MACRO_DESCRIPTOR_SIZE + MACRO_KEY_MAP_OFFSET;
            let code = swap_letters(read_u32_le(blob, at)?);
            put_u32_le(blob, at, code)?;
        }
    }
    Ok(())
}

/// `code` with the A and B bits traded, and the X and Y bits. The codes follow the
/// printed letter, which sits on another face than on the Pro 3.
fn swap_letters(code: u32) -> u32 {
    LETTER_SWAPS.iter().fold(code, |code, &(a, b)| {
        let rest = code & !(a | b);
        let moved = if code & a != 0 { b } else { 0 } | if code & b != 0 { a } else { 0 };
        rest | moved
    })
}

/// [`swap_letters`] on each step's button bits in `stream`, a run of step records.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if a step record is cut short.
fn swap_step_letters(stream: &mut [u8]) -> Result<()> {
    use pro3::tables::{MACRO_STEP_RECORD_SIZE, STEP_KEYS_OFFSET};
    for start in (0..stream.len() / MACRO_STEP_RECORD_SIZE).map(|i| i * MACRO_STEP_RECORD_SIZE) {
        let at = start + STEP_KEYS_OFFSET;
        let keys = swap_letters(u32::from(read_u16_le(stream, at)?));
        put_u16_le(stream, at, u16::try_from(keys).unwrap_or_default())?;
    }
    Ok(())
}

/// Moves each step's triggers in `stream` from `trigger_value` into the `keys` bits, as
/// the vendor app stores them on the Ultimate 2: any pull of a trigger becomes a full one.
/// The pad plays the high byte of `trigger_value`, the Pro 3's left trigger, as RT and
/// ignores the low byte (`trigger_probe`), so no value there plays LT.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if a step record is cut short.
fn triggers_in_keys(stream: &mut [u8]) -> Result<()> {
    use pro3::tables::{
        MACRO_STEP_RECORD_SIZE, STEP_KEYS_OFFSET, STEP_SWITCH_L2_MASK, STEP_SWITCH_R2_MASK,
        STEP_TRIGGER_VALUE_OFFSET,
    };
    for start in (0..stream.len() / MACRO_STEP_RECORD_SIZE).map(|i| i * MACRO_STEP_RECORD_SIZE) {
        let value = read_u16_le(stream, start + STEP_TRIGGER_VALUE_OFFSET)?;
        let mut keys = read_u16_le(stream, start + STEP_KEYS_OFFSET)?;
        if value >> 8 != 0 {
            keys |= STEP_SWITCH_L2_MASK;
        }
        if value & 0xFF != 0 {
            keys |= STEP_SWITCH_R2_MASK;
        }
        put_u16_le(stream, start + STEP_KEYS_OFFSET, keys)?;
        put_u16_le(stream, start + STEP_TRIGGER_VALUE_OFFSET, 0)?;
    }
    Ok(())
}

/// Writes the macro count of each slot whose macro record is in use, the number of
/// its macros with steps, as the vendor app counts it before a write. An erased
/// record keeps its `ff`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if `blob` is shorter than its macro records.
fn count_macros(blob: &mut [u8]) -> Result<()> {
    use pro3::tables::{
        MACRO_DESCRIPTOR_SIZE, MACRO_MAX_STEPS_OFFSET, MACRO_SLOTS_PER_PROFILE,
        SECTION4_RECORD_HEADER_SIZE, SECTION4_SLOT_STRIDE,
    };
    for slot in 0..usize::from(pro3::tables::PROFILE_SLOT_COUNT) {
        let record = SHARED_HEAD_END + slot * SECTION4_SLOT_STRIDE;
        if take(blob, record, pro3::tables::SLOT_MARKER.len())? != pro3::tables::SLOT_MARKER {
            continue;
        }
        let mut count = 0u8;
        for i in 0..MACRO_SLOTS_PER_PROFILE {
            let at = record + SECTION4_RECORD_HEADER_SIZE + i * MACRO_DESCRIPTOR_SIZE;
            if read_u16_le(blob, at + MACRO_MAX_STEPS_OFFSET)? > 0 {
                count += 1;
            }
        }
        put_slice(blob, record + MACRO_COUNT_OFFSET, &[count])?;
    }
    Ok(())
}

/// `image` with the motion and light settings of `profile` in `slot`, CRC resealed. A
/// group the profile lacks leaves its bytes as they are.
///
/// # Errors
/// Returns [`crate::Error::Validation`] if a value of a group is missing or out of
/// range, or [`crate::Error::Decode`] if `image` is not a full image.
fn with_settings(mut image: Vec<u8>, profile: &CanonicalProfile, slot: Slot) -> Result<Vec<u8>> {
    if let Some(group) = profile.settings.get(motion::GROUP) {
        motion::write(&mut image, slot, group)?;
    }
    if let Some(group) = profile.settings.get(lights::GROUP) {
        lights::write(&mut image, slot, group)?;
    }
    pro3::profile::seal_crc(&mut image)?;
    Ok(image)
}

/// `profile` with this model's `device` and `kind`.
fn own(mut profile: CanonicalProfile) -> CanonicalProfile {
    DEVICE.clone_into(&mut profile.device);
    KIND.clone_into(&mut profile.kind);
    profile
}

/// `mode`, if the Ultimate 2 has it. The Pro 3 bytes serve only the modes both share.
///
/// # Errors
/// Returns [`crate::Error::Validation`] for a mode the Ultimate 2 does not have.
fn has(mode: Mode) -> Result<Mode> {
    match Ultimate2.description()?.mode(mode) {
        Some(_) => Ok(mode),
        None => Err(crate::Error::Validation(format!("the Ultimate 2 has no {mode} mode"))),
    }
}

impl ControllerSpec for Ultimate2 {
    fn description(&self) -> Result<&'static ControllerDescription> {
        DESCRIPTION.as_ref().map_err(Clone::clone)
    }
    fn blob_size(&self) -> usize {
        BLOB_SIZE
    }
    fn apply_argument(&self) -> u16 {
        Pro3.apply_argument()
    }
    fn apply_delay(&self) -> Duration {
        APPLY_DELAY
    }
    fn mode_request(&self) -> Option<RawRequest> {
        let mut wire = [0; PACKET_LEN];
        for (dst, b) in wire.iter_mut().zip(MODE_REQUEST) {
            *dst = b;
        }
        Some(RawRequest { wire, reply_head: MODE_REPLY_HEAD })
    }
    fn mode_in_reply(&self, reply: &[u8]) -> Result<Mode> {
        match read_u8(reply, MODE_REPLY_OFFSET)? {
            MODE_REPLY_XINPUT => Ok(XINPUT),
            MODE_REPLY_DINPUT => Ok(DINPUT),
            other => Err(crate::Error::Decode(format!(
                "the pad names an unknown mode {other}: switch it to XInput or DInput"
            ))),
        }
    }
    fn slot_select_value(&self, mode: Mode) -> Result<u8> {
        Pro3.slot_select_value(has(mode)?)
    }
    fn macro_gamepad_mode(&self, mode: Mode) -> Result<u8> {
        Pro3.macro_gamepad_mode(has(mode)?)
    }
}

impl ProtocolCodec for Ultimate2 {
    /// Refuses the bank of another mode, so that a read shows no empty slots there.
    fn slot_active(&self, blob: &[u8], slot: Slot) -> Result<bool> {
        Pro3.slot_active(&widen(blob)?, slot)
    }

    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        check_bank(&raw.payload, raw.mode_hint)?;
        let wide = RawProfilePayload { payload: widen(&raw.payload)?, ..raw.clone() };
        let mut summary = Pro3.map_profile(&wide)?;
        summary.canonical = own(summary.canonical);
        paddles_as_themselves(&mut summary.canonical);
        let slot = Slot::new(raw.source_slot)?;
        let settings = &mut summary.canonical.settings;
        settings.insert(motion::GROUP.to_owned(), motion::read(&raw.payload, slot)?);
        settings.insert(lights::GROUP.to_owned(), lights::read(&raw.payload, slot)?);
        Ok(summary)
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        let mut profile = own(Pro3.default_profile(mode));
        paddles_as_themselves(&mut profile);
        profile.settings.insert(motion::GROUP.to_owned(), motion::default());
        profile.settings.insert(lights::GROUP.to_owned(), lights::default());
        profile
    }

    fn decode_macro_metadata(
        &self,
        blob: &[u8],
        profile_slot: Slot,
    ) -> Result<Vec<MacroDefinition>> {
        // A record the pad never wrote is erased flash, whose `ff` descriptors the Pro 3
        // code would read as macros. Only a record with the marker holds any.
        let index = usize::from(profile_slot.get()).saturating_sub(1);
        let record = SHARED_HEAD_END + index * pro3::tables::SECTION4_SLOT_STRIDE;
        if blob.len() == BLOB_SIZE
            && take(blob, record, pro3::tables::SLOT_MARKER.len())? != pro3::tables::SLOT_MARKER
        {
            return Ok(Vec::new());
        }
        Pro3.decode_macro_metadata(&widen(blob)?, profile_slot)
    }

    fn decode_macro_steps(
        &self,
        stream: &[u8],
        step_count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>> {
        let mut stream = stream.to_vec();
        swap_step_letters(&mut stream)?;
        Pro3.decode_macro_steps(&stream, step_count, mode)
    }

    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
        let mut stream = Pro3.encode_macro_steps(steps, mode)?;
        triggers_in_keys(&mut stream)?;
        swap_step_letters(&mut stream)?;
        Ok(stream)
    }

    fn encode_macro_metadata(
        &self,
        def: &MacroDefinition,
        macro_slot: MacroSlot,
    ) -> Result<Vec<u8>> {
        let mut descriptor = Pro3.encode_macro_metadata(def, macro_slot)?;
        let at = pro3::tables::MACRO_KEY_MAP_OFFSET;
        let code = swap_letters(read_u32_le(&descriptor, at)?);
        put_u32_le(&mut descriptor, at, code)?;
        Ok(descriptor)
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        check_bank(base_blob, profile.mode)?;
        let base = clear_unused(base_blob, target_slot)?;
        let pro3 = paddles_for_pro3(profile);
        let wide = Pro3.compile_profile(&pro3, target_slot, &widen(&base)?, macros)?;
        with_settings(narrow(&wide, &base)?, profile, target_slot)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        check_bank(base_blob, profile.mode)?;
        let base = clear_unused(base_blob, target_slot)?;
        let pro3 = paddles_for_pro3(profile);
        let wide = Pro3.compile_profile_keep_macros(&pro3, target_slot, &widen(&base)?)?;
        with_settings(narrow(&wide, &base)?, profile, target_slot)
    }

    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        narrow(&Pro3.drop_macros(&widen(blob)?, slot, triggers)?, blob)
    }

    fn set_macro(
        &self,
        blob: &[u8],
        slot: Slot,
        macro_slot: MacroSlot,
        def: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        narrow(&Pro3.set_macro(&widen(blob)?, slot, macro_slot, def)?, blob)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        narrow(&Pro3.deactivate_profile(&widen(base_blob)?, slot)?, base_blob)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        self.description()?.validate_remap(mode, source, target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    /// Bank read of a real Ultimate 2 in `XInput`: slot 1 "Crimson" with two macros.
    const XINPUT_BLOB: &[u8] = include_bytes!("../../../../../fixtures/ultimate-2/xinput.blob");
    /// Where the CRC sits. The pad does not check it, and its own differs from ours.
    const CRC: std::ops::Range<usize> = 0x0C..0x10;

    /// The motion sensor's button note hides while the sensor moves nothing, and a note
    /// that names an option its choice lacks is refused.
    #[test]
    fn the_button_note_names_an_option_of_a_choice() {
        let raw = include_str!("../../../controllers/ultimate-2/description.json");
        let svgs = [
            ("front.svg", include_str!("../../../controllers/ultimate-2/front.svg")),
            ("back.svg", include_str!("../../../controllers/ultimate-2/back.svg")),
        ];
        let note = Ultimate2.description().unwrap().choice(XINPUT, "/motion/button").unwrap();
        let unless = note.button_note.as_ref().unwrap().unless.as_ref().unwrap();
        assert_eq!((unless.field.as_str(), unless.is.as_str()), ("/motion/map_to", "off"));
        let broken = raw.replacen(r#""is": "off""#, r#""is": "sideways""#, 1);
        assert_ne!(broken, raw);
        assert!(ControllerDescription::parse(&broken, &svgs).is_err());
    }

    /// Points picked from the art: each button's centre lands on that button only.
    #[test]
    fn ultimate_2_hotspots_sit_on_their_buttons() {
        let views = &Ultimate2.description().unwrap().views;
        let hit = |view: usize, x: f64, y: f64| -> Vec<&str> {
            let hotspots = views[view].hotspots.iter();
            hotspots.filter(|h| h.contains(x, y)).map(|h| h.button.as_str()).collect()
        };
        let none = Vec::<&str>::new();
        let front = [
            (380.0, 84.0, "top face"),
            (348.0, 117.0, "left face"),
            (413.0, 117.0, "right face"),
            (380.0, 149.0, "bottom face"),
            (188.0, 163.0, "d-pad up"),
            (188.0, 215.0, "d-pad down"),
            (162.0, 189.0, "d-pad left"),
            (214.0, 189.0, "d-pad right"),
            (120.0, 117.0, "l3"),
            (312.0, 189.0, "r3"),
            (191.0, 78.0, "select/back"),
            (309.0, 78.0, "start/menu"),
            (250.0, 78.0, "home/guide"),
            (281.0, 112.0, "turbo"),
            (122.0, 33.0, "l1"),
            (378.0, 33.0, "r1"),
            (149.0, 25.0, "l4"),
            (351.0, 25.0, "r4"),
        ];
        for (x, y, button) in front {
            assert_eq!(hit(0, x, y), [button], "front ({x}, {y})");
        }
        assert_eq!(hit(0, 188.0, 189.0), none, "d-pad centre");
        assert_eq!(hit(0, 90.0, 60.0), none, "body below L1");
        assert_eq!(hit(0, 219.0, 112.0), none, "the square button has no id");
        assert_eq!(hit(0, 250.0, 150.0), none, "the profile button has no id");
        // Seen from behind, the right hand is on the left.
        let back = [
            (102.0, 40.0, "r2"),
            (398.0, 40.0, "l2"),
            (151.0, 21.0, "r4"),
            (349.0, 21.0, "l4"),
            (165.0, 165.0, "rp"),
            (335.0, 165.0, "lp"),
        ];
        for (x, y, button) in back {
            assert_eq!(hit(1, x, y), [button], "back ({x}, {y})");
        }
        assert_eq!(hit(1, 250.0, 120.0), none, "the back between the paddles");
    }

    /// `blob` with its CRC zeroed.
    fn sans_crc(blob: &[u8]) -> Vec<u8> {
        let mut out = blob.to_vec();
        out[CRC].fill(0);
        out
    }

    fn slot1() -> CanonicalProfile {
        let raw = RawProfilePayload {
            payload: XINPUT_BLOB.to_vec(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: XINPUT,
        };
        Ultimate2.map_profile(&raw).unwrap().canonical
    }

    #[test]
    fn an_erased_macro_record_holds_no_macros() {
        // Slots 2 and 3 of the capture are erased flash, `ff` but a few bytes.
        for n in [2, 3] {
            let slot = Slot::new(n).unwrap();
            assert_eq!(Ultimate2.decode_macro_metadata(XINPUT_BLOB, slot).unwrap(), []);
        }
    }

    #[test]
    fn a_paddle_left_as_itself_is_stored_as_zero_in_both_modes() {
        let slot = Slot::new(1).unwrap();
        // Entry 20 of slot 1's map: L4.
        let entry = pro3::tables::BUTTON_MAP_BASE_OFFSET + 20 * 4;
        for (mode, base) in [(XINPUT, XINPUT_BLOB.to_vec()), (DINPUT, erased_dinput())] {
            let profile = Ultimate2.default_profile(mode);
            assert!(profile.button_mappings.iter().any(|m| m.source == "l4" && m.target == "l4"));
            let bank = Ultimate2.compile_profile(&profile, slot, &base, &[]).unwrap();
            assert_eq!(bank[entry..entry + 4], [0, 0, 0, 0], "{mode}");
        }
        // The capture's L4 and R4 start macros and hold 0: they read as themselves.
        let p = slot1();
        for paddle in ["l4", "r4"] {
            assert!(p.button_mappings.iter().any(|m| m.source == paddle && m.target == paddle));
        }
    }

    #[test]
    fn a_dinput_paddle_sent_to_itself_reads_back_as_itself() {
        let slot = Slot::new(1).unwrap();
        let mut profile = Ultimate2.default_profile(DINPUT);
        let m = profile.button_mappings.iter_mut().find(|m| m.source == "l4").unwrap();
        "l4".clone_into(&mut m.target);
        let bank = Ultimate2.compile_profile(&profile, slot, &erased_dinput(), &[]).unwrap();
        let raw = RawProfilePayload {
            payload: bank,
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: DINPUT,
        };
        let read = Ultimate2.map_profile(&raw).unwrap().canonical;
        assert_eq!(read.button_mappings, profile.button_mappings);
    }

    #[test]
    fn layout_sizes_add_up() {
        assert_eq!(PRO3_TAIL_START, 0x068C);
        assert_eq!(TAIL_END, 0x0494);
        assert_eq!(XINPUT_BLOB.len(), BLOB_SIZE);
        assert_eq!(widen(XINPUT_BLOB).unwrap().len(), pro3::tables::EXPECTED_PROFILE_SIZE);
    }

    #[test]
    fn narrow_undoes_widen() {
        let back = narrow(&widen(XINPUT_BLOB).unwrap(), XINPUT_BLOB).unwrap();
        assert_eq!(sans_crc(&back), sans_crc(XINPUT_BLOB));
    }

    #[test]
    fn reads_the_captured_slot() {
        let p = slot1();
        assert_eq!(
            (p.name.as_str(), p.device.as_str(), p.kind.as_str()),
            ("Crimson", DEVICE, KIND)
        );
        for face in ["right face", "bottom face", "top face", "left face"] {
            let m = p.button_mappings.iter().find(|m| m.source == face).unwrap();
            assert_eq!(m.target, face, "the capture remaps no face button");
        }
        let macros = Ultimate2.decode_macro_metadata(XINPUT_BLOB, Slot::new(1).unwrap()).unwrap();
        let found: Vec<_> =
            macros.iter().map(|m| (m.name.as_str(), m.trigger_buttons.clone())).collect();
        assert_eq!(found, [("L1L2", vec!["l4".to_owned()]), ("R1R2", vec!["r4".to_owned()])]);
    }

    #[test]
    fn letters_trade_places_and_back() {
        assert_eq!(swap_letters(0x2000), 0x1000);
        assert_eq!(swap_letters(0x1000 | 0x10 | 0x400), 0x2000 | 0x20 | 0x400);
        assert_eq!(swap_letters(swap_letters(0xFFFF_FFFF)), 0xFFFF_FFFF);
        assert_eq!(swap_letters(0x0020_0000), 0x0020_0000, "L4 is no letter");
    }

    #[test]
    fn a_macro_started_by_b_reads_as_the_right_face() {
        // On the pad, the trigger code 0x1000 (B) fired from the right face.
        let mut blob = XINPUT_BLOB.to_vec();
        let key_map = SHARED_HEAD_END + 8 + pro3::tables::MACRO_KEY_MAP_OFFSET;
        blob[key_map..key_map + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        let macros = Ultimate2.decode_macro_metadata(&blob, Slot::new(1).unwrap()).unwrap();
        assert_eq!(macros[0].trigger_buttons, ["right face"]);
    }

    #[test]
    fn a_step_of_b_presses_the_right_face() {
        let steps =
            Ultimate2.decode_macro_steps(&[0, 0, 0x00, 0x10, 0, 0, 0, 0, 0, 0], 1, XINPUT).unwrap();
        let back = Ultimate2.encode_macro_steps(&steps, XINPUT).unwrap();
        assert_eq!(back[2..4], [0x00, 0x10]);
        assert_eq!(steps[0].pressed_buttons, ["right face"]);
    }

    #[test]
    fn a_step_trigger_is_stored_as_the_vendor_app_stores_it() {
        // The vendor app's save of LT, RT, then both, to an Ultimate 2 in XInput
        // (`fixtures/ultimate-2/xinput-vendor-triggers.steps.bin`): L2 as keys 0x4000, R2
        // as 0x8000, trigger_value 0, sticks centred. The pad played 8B's trigger_value
        // 0xFF00 as RT.
        let vendor =
            include_bytes!("../../../../../fixtures/ultimate-2/xinput-vendor-triggers.steps.bin");
        let step = |duration_ms, trigger_left, trigger_right| MacroStep {
            duration_ms,
            trigger_left,
            trigger_right,
            ..MacroStep::default()
        };
        let steps = [
            step(500, 255, 0),
            step(500, 0, 0),
            step(500, 0, 255),
            step(500, 0, 0),
            step(500, 255, 255),
            step(500, 0, 255),
            step(0, 0, 0),
        ];
        for mode in [XINPUT, DINPUT] {
            let stream = Ultimate2.encode_macro_steps(&steps, mode).unwrap();
            assert_eq!(stream[..vendor.len()], vendor[..], "{mode}");
            let back = Ultimate2.decode_macro_steps(&stream, steps.len(), mode).unwrap();
            assert_eq!(back, steps, "{mode}");
        }
    }

    /// Each step limit keeps its padded write off what the pad keeps in `DInput` slot 3's
    /// page 0. The controller's limit ends before its byte at `0x0FF0`, and is the longest
    /// every other macro slot kept across mode switches (`sweep_probe`). That slot's macro
    /// 1 limit ends by `0x0C00`, where the pad's own region starts: a longer macro 1 was
    /// erased at the next `DInput` start-up (`step_probe::record_probe`).
    #[test]
    fn each_step_limit_keeps_its_write_off_the_pad_s_own_bytes() {
        const PAD_BYTE: usize = 0x0FF0;
        const PAD_REGION: usize = 0x0C00;
        let d = Ultimate2.description().unwrap();
        let written = |mode, steps: i32| {
            let steps = vec![MacroStep::default(); usize::try_from(steps).unwrap()];
            Ultimate2.encode_macro_steps(&steps, mode).unwrap().len()
        };
        for mode in [XINPUT, DINPUT] {
            assert!(written(mode, d.limits.macro_steps.max) <= PAD_BYTE, "{mode}");
        }
        assert!(written(DINPUT, d.macro_steps_max(DINPUT, 3, 0)) <= PAD_REGION);
    }

    /// What the pad served in `DInput`: never saved, erased flash with the mode byte.
    fn erased_dinput() -> Vec<u8> {
        let mut bank = vec![0xff; BLOB_SIZE];
        bank[MODE_OFFSET..MODE_OFFSET + 2].copy_from_slice(&[1, 0]);
        bank
    }

    #[test]
    fn a_bank_of_another_mode_is_refused() {
        let raw = RawProfilePayload {
            payload: erased_dinput(),
            source_slot: 1,
            source_profile_index: 0,
            mode_hint: XINPUT,
        };
        assert!(Ultimate2.map_profile(&raw).is_err());
        let slot = Slot::new(1).unwrap();
        assert!(Ultimate2.compile_profile(&slot1(), slot, &erased_dinput(), &[]).is_err());
        let mut switch = erased_dinput();
        switch[MODE_OFFSET] = 0;
        assert!(Ultimate2.slot_active(&switch, slot).is_err(), "Switch is no mode of the pad");
    }

    #[test]
    fn a_dinput_profile_round_trips_through_the_erased_bank() {
        let slot = Slot::new(2).unwrap();
        assert!(!Ultimate2.slot_active(&erased_dinput(), slot).unwrap());
        let mut profile = Ultimate2.default_profile(DINPUT);
        "Blue".clone_into(&mut profile.name);
        let m = profile.button_mappings.iter_mut().find(|m| m.source == "bottom face").unwrap();
        "right face".clone_into(&mut m.target);
        let bank = Ultimate2.compile_profile(&profile, slot, &erased_dinput(), &[]).unwrap();
        // No erased byte survives in the slot: full stick range, no option bits.
        assert_eq!(bank[0x098 + 8 + 4..0x098 + 8 + 8], [0x11, 0x80, 0x11, 0x80]);
        assert_eq!(bank[0x0C8 + 8 + 4..0x0C8 + 8 + 8], [0, 0, 0, 0]);
        // The other slots stay erased.
        assert!(bank[0x098 + 4..0x098 + 8].iter().all(|&b| b == 0xff));
        // The bottom face (entry 0) now sends B, the right face's code.
        let entry = pro3::tables::BUTTON_MAP_BASE_OFFSET + pro3::tables::BUTTON_MAP_SLOT_STRIDE;
        assert_eq!(bank[entry..entry + 4], 0x1000u32.to_le_bytes());
        let raw = RawProfilePayload {
            payload: bank,
            source_slot: 2,
            source_profile_index: 1,
            mode_hint: DINPUT,
        };
        let read = Ultimate2.map_profile(&raw).unwrap().canonical;
        assert_eq!(read.button_mappings, profile.button_mappings);
        assert_eq!(read.name, "Blue");
    }

    #[test]
    fn the_mode_reply_names_the_mode() {
        let mut reply = [0u8; 64];
        reply[..2].copy_from_slice(&MODE_REPLY_HEAD);
        reply[MODE_REPLY_OFFSET] = 1;
        assert_eq!(Ultimate2.mode_in_reply(&reply).unwrap(), XINPUT);
        reply[MODE_REPLY_OFFSET] = 0;
        assert_eq!(Ultimate2.mode_in_reply(&reply).unwrap(), DINPUT);
        reply[MODE_REPLY_OFFSET] = 2;
        assert!(Ultimate2.mode_in_reply(&reply).is_err());
    }

    #[test]
    fn writing_the_read_profile_back_changes_no_byte() {
        let out = Ultimate2
            .compile_profile_keep_macros(&slot1(), Slot::new(1).unwrap(), XINPUT_BLOB)
            .unwrap();
        let diff: Vec<usize> =
            (0..BLOB_SIZE).filter(|&i| out[i] != XINPUT_BLOB[i] && !CRC.contains(&i)).collect();
        assert!(diff.is_empty(), "bytes differ at {diff:x?}");
    }
}
