//! Canonical macro model matching `schemas/macro-v1.schema.json`.

use serde_json::{json, Map, Value};

use super::ids::Mode;

/// A single macro step, in memory.
///
/// Canonical macro JSON (per `schemas/macro-v1.schema.json`, with its nested
/// `repeat` and `actions.buttons.press/release` shape) is produced and consumed
/// via dedicated converters, not via direct serde derive, which is why this type
/// intentionally does not derive `Serialize`/`Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroStep {
    /// Delay before this step (ms).
    pub duration_ms: u16,
    /// Canonical names of pressed buttons.
    pub pressed_buttons: Vec<String>,
    /// Left stick X (0–255, center 127).
    pub left_stick_x: u8,
    /// Left stick Y.
    pub left_stick_y: u8,
    /// Right stick X.
    pub right_stick_x: u8,
    /// Right stick Y.
    pub right_stick_y: u8,
    /// L2 analog (0–255).
    pub trigger_left: u8,
    /// R2 analog (0–255).
    pub trigger_right: u8,
}

impl Default for MacroStep {
    fn default() -> Self {
        Self {
            duration_ms: 0,
            pressed_buttons: Vec::new(),
            left_stick_x: 127,
            left_stick_y: 127,
            right_stick_x: 127,
            right_stick_y: 127,
            trigger_left: 0,
            trigger_right: 0,
        }
    }
}

/// A complete macro definition, in memory.
///
/// Canonical macro JSON (per `schemas/macro-v1.schema.json`, with its nested
/// `repeat` and `actions.buttons.press/release` shape) is produced and consumed
/// via dedicated converters, not via direct serde derive, which is why this type
/// intentionally does not derive `Serialize`/`Deserialize`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MacroDefinition {
    /// Display name (1–15 chars).
    pub name: String,
    /// Mode (`xinput` or `switch`).
    pub mode: Mode,
    /// Canonical trigger name.
    pub trigger: String,
    /// Repeat count (`0xFFFF_FFFF` = continuous).
    pub repeat_count: u32,
    /// Interval between repeats (ms).
    pub interval_ms: u32,
    /// Ordered steps.
    pub steps: Vec<MacroStep>,
    /// Original macro slot (0–3) or `None`.
    pub macro_slot: Option<u8>,
}

/// The file name of an exported macro, and the `path` a read puts in `macro_refs`.
///
/// The shape is `<mode>-slot<slot>-macro<m>-<name>.json`. Each character of the name
/// other than a letter, a digit, `-` or `_` becomes `_`.
#[must_use]
pub fn macro_file_name(mode: Mode, profile_slot: u8, def: &MacroDefinition) -> String {
    let safe_name: String = def
        .name
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    let m_slot = def.macro_slot.unwrap_or(0);
    format!("{mode}-slot{profile_slot}-macro{m_slot}-{safe_name}.json")
}

/// The macro slot (0–3) and the name in a [`macro_file_name`] path, or `None` when
/// `path` has another shape.
#[must_use]
pub fn parse_macro_file_name(path: &str) -> Option<(u8, &str)> {
    let mut parts = path.strip_suffix(".json")?.splitn(4, '-');
    let (_mode, _slot) = (parts.next()?, parts.next()?);
    let macro_slot = parts.next()?.strip_prefix("macro")?.parse().ok()?;
    Some((macro_slot, parts.next()?))
}

/// A stick axis at rest, the value a step omits.
pub const STICK_CENTER: u8 = 127;

/// Serializes a [`MacroDefinition`] to canonical macro JSON for `device`, the device
/// string of the model's profiles.
///
/// Top-level: `version:1, device, mode, name, trigger,
/// repeat:{count,interval_ms}, steps:[...]`. Per step, `actions.buttons` is
/// ALWAYS emitted (with `press`+`release` arrays; the wire format only tracks
/// the currently-pressed set, so `release` is always empty); `left_stick`,
/// `right_stick` and `triggers` are OMITTED when at their defaults (stick
/// `127/127`, triggers `0/0`).
#[must_use]
pub fn macro_to_json(def: &MacroDefinition, device: &str) -> Value {
    let steps: Vec<Value> = def.steps.iter().map(step_to_json).collect();

    json!({
        "version": 1,
        "device": device,
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

    // Buttons: always emitted; `release` is always empty.
    let press: Vec<Value> =
        step.pressed_buttons.iter().map(|name| Value::String(name.clone())).collect();
    actions.insert("buttons".to_owned(), json!({ "press": press, "release": Vec::<Value>::new() }));

    // Left stick — omitted when centered.
    if step.left_stick_x != STICK_CENTER || step.left_stick_y != STICK_CENTER {
        actions.insert(
            "left_stick".to_owned(),
            json!({ "x": step.left_stick_x, "y": step.left_stick_y }),
        );
    }

    // Right stick — omitted when centered.
    if step.right_stick_x != STICK_CENTER || step.right_stick_y != STICK_CENTER {
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
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::devices::pro3::XINPUT;

    #[test]
    fn a_macro_file_name_parses_back_to_its_slot_and_name() {
        let def = MacroDefinition {
            name: "My-macro 1".into(),
            mode: XINPUT,
            trigger: "l1".into(),
            repeat_count: 1,
            interval_ms: 0,
            steps: Vec::new(),
            macro_slot: Some(2),
        };
        let path = macro_file_name(XINPUT, 1, &def);
        assert_eq!(parse_macro_file_name(&path), Some((2, "My-macro_1")));
        assert_eq!(parse_macro_file_name("m.json"), None);
        assert_eq!(parse_macro_file_name("xinput-slot1-macroX-a.json"), None);
    }
}
