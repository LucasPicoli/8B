//! Canonical macro model matching `schemas/macro-v1.schema.json`.

use super::ids::Mode;

/// A single macro step — in-memory representation mirroring the C++ model.
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

/// A complete macro definition — the in-memory representation mirroring the C++ model.
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
