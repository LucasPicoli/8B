//! Canonical profile model matching `schemas/profile-v1.schema.json`.

use serde::{Deserialize, Serialize};

use super::ids::Mode;

/// A full canonical profile (export/validation shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CanonicalProfile {
    /// Deterministic profile id.
    pub id: String,
    /// Human-readable name (1–16 chars).
    pub name: String,
    /// Schema version (always 1).
    pub version: u8,
    /// Schema kind discriminator.
    pub kind: String,
    /// Device discriminator (`"8bitdo-pro3"`).
    pub device: String,
    /// Operating mode.
    pub mode: Mode,
    /// Preferred slot, if recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_slot: Option<u8>,
    /// Stick configuration.
    pub sticks: Sticks,
    /// Trigger configuration (analog or switch shape).
    pub triggers: Triggers,
    /// Vibration levels.
    pub vibration: Vibration,
    /// Button remaps.
    pub button_mappings: Vec<ButtonMapping>,
    /// Macro references (always empty for device readback).
    pub macro_refs: Vec<MacroRef>,
}

/// Stick configuration block.
// canonical wire/JSON shape — field set fixed by schema
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sticks {
    /// Left stick min deadzone percent.
    pub left_min_pct: i32,
    /// Left stick max range percent.
    pub left_max_pct: i32,
    /// Right stick min deadzone percent.
    pub right_min_pct: i32,
    /// Right stick max range percent.
    pub right_max_pct: i32,
    /// Invert left X.
    pub invert_left_x: bool,
    /// Invert left Y.
    pub invert_left_y: bool,
    /// Invert right X.
    pub invert_right_x: bool,
    /// Invert right Y.
    pub invert_right_y: bool,
    /// Swap left and right sticks.
    pub swap_sticks: bool,
    /// Swap D-pad with left stick.
    pub swap_dpad_with_left_stick: bool,
}

/// Trigger configuration; shape depends on mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Triggers {
    /// XInput/DInput analog ranges.
    Analog(TriggersAnalog),
    /// Switch threshold form.
    Switch(TriggersSwitch),
}

/// Analog trigger ranges (xinput/dinput).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggersAnalog {
    /// Left trigger min percent.
    pub left_min_pct: i32,
    /// Left trigger max percent.
    pub left_max_pct: i32,
    /// Right trigger min percent.
    pub right_min_pct: i32,
    /// Right trigger max percent.
    pub right_max_pct: i32,
    /// Swap triggers.
    pub swap_triggers: bool,
}

/// Switch trigger thresholds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TriggersSwitch {
    /// Left trigger threshold percent.
    pub left_threshold_pct: i32,
    /// Right trigger threshold percent.
    pub right_threshold_pct: i32,
    /// Swap triggers.
    pub swap_triggers: bool,
}

/// Vibration levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vibration {
    /// Left motor level (0–5).
    pub left_level: i32,
    /// Right motor level (0–5).
    pub right_level: i32,
}

/// A single button remap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ButtonMapping {
    /// Source control name.
    pub source: String,
    /// Target control name or `"disabled"`/`"screenshot"`.
    pub target: String,
}

/// A reference from a profile to a macro file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MacroRef {
    /// Trigger control name.
    pub trigger: String,
    /// Relative path to the macro JSON.
    pub path: String,
}

/// Raw profile payload from USB readback before mapping.
#[derive(Debug, Clone)]
pub struct RawProfilePayload {
    /// Raw 2348-byte profile blob.
    pub payload: Vec<u8>,
    /// 1-based slot index.
    pub source_slot: u8,
    /// 0-based profile index.
    pub source_profile_index: u8,
    /// Mode context for table selection.
    pub mode_hint: Mode,
}

/// A mapped profile plus provenance metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalProfileSummary {
    /// Profile id.
    pub id: String,
    /// Profile name.
    pub name: String,
    /// Mode.
    pub mode: Mode,
    /// 1-based slot.
    pub source_slot: u8,
    /// 0-based profile index.
    pub source_profile_index: u8,
    /// Full canonical profile.
    pub canonical: CanonicalProfile,
}

/// Result of reading all on-device profiles.
#[derive(Debug, Clone, Default)]
pub struct ProfileReadResult {
    /// Mapped canonical profiles.
    pub profiles: Vec<CanonicalProfileSummary>,
    /// Raw blobs for diagnostics/dump.
    pub raw_blobs: Vec<Vec<u8>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unwrap_used)]
    #[test]
    fn profile_json_round_trips() {
        let json = serde_json::json!({
            "id":"test","name":"Test","version":1,
            "kind":"8bitdo.pro3.profile","device":"8bitdo-pro3","mode":"xinput",
            "sticks":{"left_min_pct":0,"left_max_pct":100,"right_min_pct":0,"right_max_pct":100,
              "invert_left_x":false,"invert_left_y":false,"invert_right_x":false,"invert_right_y":false,
              "swap_sticks":false,"swap_dpad_with_left_stick":false},
            "triggers":{"left_min_pct":0,"left_max_pct":100,"right_min_pct":0,"right_max_pct":100,"swap_triggers":false},
            "vibration":{"left_level":3,"right_level":3},
            "button_mappings":[], "macro_refs":[]
        });
        let p: CanonicalProfile = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(p.mode, Mode::XInput);
        assert_eq!(serde_json::to_value(&p).unwrap(), json);
    }
}
