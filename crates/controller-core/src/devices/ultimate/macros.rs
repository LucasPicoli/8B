//! The Ultimate's macros: up to four records per slot, inside the profile blob.
//!
//! Every code in a record names a printed button, in both modes: `0x2000` is printed
//! A, the right face button. The button map names the output instead, so in `XInput`
//! the same code means Xbox A, the bottom face button. A step holds its buttons and one
//! of 8 directions per stick at full travel, never an analog value.

use crate::error::{Error, Result};
use crate::model::{MacroDefinition, MacroStep, Mode, Slot};
use crate::protocol::bytes::{
    put_slice, put_u16_le, put_u32_le, read_u16_le, read_u32_le, read_u8, take,
};

use super::tables::{
    IN_USE, LEFT_STICK_BITS, MACROS_LEN, MACROS_OFFSET, MACRO_COUNT_AT, MACRO_PADDING_AT,
    MACRO_RECORDS_AT, MACRO_RECORD_LEN, MACRO_SLOTS, MACRO_STEPS, RECORD_CODES_AT, RECORD_COUNT_AT,
    RECORD_PADDING_AT, RECORD_STICKS_AT, RECORD_TIMES_AT, RECORD_TRIGGER_AT, RIGHT_STICK_BITS,
    UNUSED_STEP_MS,
};
use super::{check_blob, description, image_mode, read_flag, slot_in_use, slot_index};
use crate::description::Button;

/// A stick axis at rest, as [`MacroStep::default`] holds it.
const CENTRE: u8 = 127;
/// The other value a centred axis may hold, the true middle of 0 to 255.
const CENTRE_HIGH: u8 = 128;
/// An axis pushed all the way up or left.
const LOW: u8 = 0;
/// An axis pushed all the way down or right.
const HIGH: u8 = 255;

/// The buttons `allowed` picks that a record can hold, with their record codes.
fn coded(allowed: fn(&Button) -> bool) -> Result<Vec<(&'static str, u32)>> {
    let buttons = &description()?.buttons;
    Ok(buttons
        .iter()
        .filter(|b| allowed(b))
        .filter_map(|b| Some((b.id.as_str(), b.macro_code?)))
        .collect())
}

/// Whether a button may start a macro.
const fn starts(b: &Button) -> bool {
    b.can_start_macro
}

/// Whether a button may be pressed in a macro step.
const fn in_step(b: &Button) -> bool {
    b.can_be_in_macro_step
}

/// The macros of `slot` in `blob`, in record order, with their steps. A slot or a macro
/// section not in use has none.
///
/// # Errors
/// Returns [`Error::Decode`] if `blob` is not a full image or its mode is unknown, and
/// [`Error::Validation`] for a slot past the third.
pub fn decode_macros(blob: &[u8], slot: Slot) -> Result<Vec<MacroDefinition>> {
    check_blob(blob)?;
    let mode = image_mode(blob)?;
    let at = MACROS_OFFSET + slot_index(slot)? * MACROS_LEN;
    if !slot_in_use(blob, slot)? || read_flag(blob, at)? != IN_USE {
        return Ok(Vec::new());
    }
    let count = usize::from(read_u8(blob, at + MACRO_COUNT_AT)?).min(MACRO_SLOTS);
    (0..count)
        .map(|m| {
            let record =
                take(blob, at + MACRO_RECORDS_AT + m * MACRO_RECORD_LEN, MACRO_RECORD_LEN)?;
            decode_record(record, u8::try_from(m).unwrap_or_default(), mode)
        })
        .collect()
}

/// Decodes one 96-byte record as the macro in macro slot `m`.
fn decode_record(record: &[u8], m: u8, mode: Mode) -> Result<MacroDefinition> {
    let trigger = read_u32_le(record, RECORD_TRIGGER_AT)?;
    let named = coded(starts)?.into_iter().find(|&(_, code)| code == trigger);
    let (trigger_buttons, unknown_trigger_bits) = match named {
        Some((name, _)) => (vec![name.to_owned()], 0),
        None => (Vec::new(), trigger),
    };
    let count = usize::from(read_u8(record, RECORD_COUNT_AT)?).min(MACRO_STEPS);
    let step_codes = coded(in_step)?;
    let steps = (0..count)
        .map(|i| {
            let code = u32::from(read_u16_le(record, RECORD_CODES_AT + 2 * i)?);
            let stick = read_u8(record, RECORD_STICKS_AT + i)?;
            let [left_stick_x, left_stick_y] = axes(stick, LEFT_STICK_BITS);
            let [right_stick_x, right_stick_y] = axes(stick, RIGHT_STICK_BITS);
            Ok(MacroStep {
                duration_ms: read_u16_le(record, RECORD_TIMES_AT + 2 * i)?,
                pressed_buttons: step_codes
                    .iter()
                    .filter(|&&(_, bit)| code & bit != 0)
                    .map(|&(name, _)| name.to_owned())
                    .collect(),
                left_stick_x,
                left_stick_y,
                right_stick_x,
                right_stick_y,
                ..MacroStep::default()
            })
        })
        .collect::<Result<_>>()?;
    Ok(MacroDefinition {
        name: String::new(),
        mode,
        trigger_buttons,
        unknown_trigger_bits,
        repeat_count: 1,
        interval_ms: 0,
        steps,
        macro_slot: Some(m),
    })
}

/// The x and y of one stick from a step's stick byte, whose `bits` are up, down, left
/// and right.
fn axes(stick: u8, [up, down, left, right]: [u8; 4]) -> [u8; 2] {
    let axis = |low: u8, high: u8| {
        if stick & low != 0 {
            LOW
        } else if stick & high != 0 {
            HIGH
        } else {
            CENTRE
        }
    };
    [axis(left, right), axis(up, down)]
}

/// The bits of one stick at `x` and `y`, whose `bits` are up, down, left and right.
fn stick_bits([x, y]: [u8; 2], [up, down, left, right]: [u8; 4]) -> Result<u8> {
    let axis = |v: u8, low: u8, high: u8| match v {
        LOW => Ok(low),
        CENTRE | CENTRE_HIGH => Ok(0),
        HIGH => Ok(high),
        other => Err(Error::Validation(format!(
            "this controller moves a stick in a macro all the way or not at all, not to {other}"
        ))),
    };
    Ok(axis(x, left, right)? | axis(y, up, down)?)
}

/// The 96-byte record of `def`.
///
/// # Errors
/// Returns [`Error::Validation`] for more than one trigger button, more than 18 steps,
/// a button a record cannot hold, a stick between centre and full travel, or a trigger
/// held at an analog value.
pub fn encode_record(def: &MacroDefinition) -> Result<[u8; MACRO_RECORD_LEN]> {
    let lookup =
        |name: &str, allowed: fn(&Button) -> bool, what: &str| {
            coded(allowed)?.into_iter().find(|&(n, _)| n == name).map(|(_, code)| code).ok_or_else(
                || Error::Validation(format!("{name} cannot {what} on this controller")),
            )
        };
    let trigger = match def.trigger_buttons.as_slice() {
        [] => 0,
        [one] => lookup(one, starts, "start a macro")?,
        _ => return Err(Error::Validation("a macro here starts from one button".to_owned())),
    };
    if def.steps.len() > MACRO_STEPS {
        return Err(Error::Validation(format!("a macro here holds at most {MACRO_STEPS} steps")));
    }
    let mut record = [0u8; MACRO_RECORD_LEN];
    put_u32_le(&mut record, RECORD_TRIGGER_AT, trigger)?;
    for i in 0..MACRO_STEPS {
        let (time, code, stick) = match def.steps.get(i) {
            None => (UNUSED_STEP_MS, 0, 0),
            Some(step) => {
                if step.trigger_left != 0 || step.trigger_right != 0 {
                    return Err(Error::Validation(
                        "a macro here presses L2 and R2 as buttons, never part way".to_owned(),
                    ));
                }
                let mut code = 0;
                for name in &step.pressed_buttons {
                    code |= lookup(name, in_step, "be pressed in a macro")?;
                }
                let left = stick_bits([step.left_stick_x, step.left_stick_y], LEFT_STICK_BITS)?;
                let right = stick_bits([step.right_stick_x, step.right_stick_y], RIGHT_STICK_BITS)?;
                (step.duration_ms, u16::try_from(code).unwrap_or_default(), left | right)
            }
        };
        put_u16_le(&mut record, RECORD_TIMES_AT + 2 * i, time)?;
        put_u16_le(&mut record, RECORD_CODES_AT + 2 * i, code)?;
        put_slice(&mut record, RECORD_STICKS_AT + i, &[stick])?;
    }
    let count = u8::try_from(def.steps.len()).unwrap_or_default();
    put_slice(&mut record, RECORD_COUNT_AT, &[count])?;
    Ok(record)
}

/// Writes `defs` into the macro `section` of one slot, packed in macro-slot order.
///
/// The pad plays the first `count` records. A record that held a macro and now holds
/// none is zeroed, as the vendor app does; a record past both counts stays as it is.
///
/// # Errors
/// Returns [`Error::Validation`] for more than four macros or a macro
/// [`encode_record`] refuses.
pub fn write_macros(section: &mut [u8], defs: &[MacroDefinition]) -> Result<()> {
    if defs.len() > MACRO_SLOTS {
        return Err(Error::Validation(format!(
            "a profile here holds at most {MACRO_SLOTS} macros"
        )));
    }
    let mut sorted: Vec<&MacroDefinition> = defs.iter().collect();
    sorted.sort_by_key(|d| d.macro_slot.unwrap_or(u8::MAX));
    let old = usize::from(read_u8(section, MACRO_COUNT_AT)?).min(MACRO_SLOTS);
    for m in 0..MACRO_SLOTS {
        let at = MACRO_RECORDS_AT + m * MACRO_RECORD_LEN;
        match sorted.get(m) {
            Some(def) => put_slice(section, at, &encode_record(def)?)?,
            None if m < old => put_slice(section, at, &[0; MACRO_RECORD_LEN])?,
            None => {}
        }
        put_slice(section, at + RECORD_PADDING_AT, &[0])?;
    }
    put_slice(section, MACRO_COUNT_AT, &[u8::try_from(sorted.len()).unwrap_or_default()])?;
    put_slice(section, MACRO_PADDING_AT, &[0; MACRO_RECORDS_AT - MACRO_PADDING_AT])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::devices::ultimate::XINPUT;

    fn step(buttons: &[&str], left: [u8; 2], right: [u8; 2]) -> MacroStep {
        MacroStep {
            duration_ms: 100,
            pressed_buttons: buttons.iter().map(|&b| b.to_owned()).collect(),
            left_stick_x: left[0],
            left_stick_y: left[1],
            right_stick_x: right[0],
            right_stick_y: right[1],
            ..MacroStep::default()
        }
    }

    fn def(trigger: &str, steps: Vec<MacroStep>) -> MacroDefinition {
        MacroDefinition {
            name: String::new(),
            mode: XINPUT,
            trigger_buttons: vec![trigger.to_owned()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps,
            macro_slot: Some(0),
        }
    }

    #[test]
    fn a_record_names_printed_buttons() {
        let record =
            encode_record(&def("right face", vec![step(&["top face"], [127; 2], [127; 2])]))
                .unwrap();
        assert_eq!(read_u32_le(&record, RECORD_TRIGGER_AT).unwrap(), 0x2000, "printed A");
        assert_eq!(read_u16_le(&record, RECORD_CODES_AT).unwrap(), 0x10, "printed X");
        assert_eq!(read_u16_le(&record, RECORD_TIMES_AT + 2).unwrap(), UNUSED_STEP_MS);
        assert_eq!(record[RECORD_COUNT_AT], 1);
        let star = encode_record(&def("star", Vec::new())).unwrap();
        assert_eq!(read_u32_le(&star, RECORD_TRIGGER_AT).unwrap(), 0x1_0000);
    }

    #[test]
    fn each_stick_direction_has_its_bit_and_comes_back() {
        let directions: [([u8; 2], u8); 8] = [
            ([127, 0], 0x01),
            ([127, 255], 0x02),
            ([0, 127], 0x04),
            ([255, 127], 0x08),
            ([0, 0], 0x05),
            ([255, 0], 0x09),
            ([0, 255], 0x06),
            ([255, 255], 0x0A),
        ];
        for (at, bits) in directions {
            let steps = vec![step(&[], at, [127; 2]), step(&[], [127; 2], at)];
            let record = encode_record(&def("p1", steps.clone())).unwrap();
            assert_eq!(record[RECORD_STICKS_AT], bits, "left {at:?}");
            assert_eq!(record[RECORD_STICKS_AT + 1], bits << 4, "right {at:?}");
            assert_eq!(decode_record(&record, 0, XINPUT).unwrap().steps, steps);
        }
        // The true middle reads back as the model's centre.
        let middle = encode_record(&def("p1", vec![step(&[], [128; 2], [128; 2])])).unwrap();
        assert_eq!(middle[RECORD_STICKS_AT], 0);
    }

    #[test]
    fn a_record_refuses_what_the_pad_cannot_play() {
        let half = def("p1", vec![step(&[], [64, 127], [127; 2])]);
        assert!(encode_record(&half).is_err(), "a stick part way");
        assert!(encode_record(&def("select/back", Vec::new())).is_err(), "no macro on View");
        assert!(encode_record(&def("p1", vec![step(&["star"], [127; 2], [127; 2])])).is_err());
        let long = def("p1", vec![step(&[], [127; 2], [127; 2]); MACRO_STEPS + 1]);
        assert!(encode_record(&long).is_err(), "19 steps");
        let pulled = MacroStep { trigger_left: 200, ..MacroStep::default() };
        assert!(encode_record(&def("p1", vec![pulled])).is_err(), "an analog trigger");
        let two = MacroDefinition {
            trigger_buttons: vec!["p1".into(), "p2".into()],
            ..def("p1", vec![])
        };
        assert!(encode_record(&two).is_err(), "two triggers");
    }

    #[test]
    fn a_dpad_diagonal_is_two_buttons() {
        let steps = vec![step(&["d-pad up", "d-pad right"], [127; 2], [127; 2])];
        let record = encode_record(&def("p2", steps.clone())).unwrap();
        assert_eq!(read_u16_le(&record, RECORD_CODES_AT).unwrap(), 0x240);
        assert_eq!(decode_record(&record, 0, XINPUT).unwrap().steps, steps);
    }
}
