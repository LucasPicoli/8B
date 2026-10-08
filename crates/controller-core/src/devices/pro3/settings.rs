//! The Pro 3's settings groups as typed structs. A [`CanonicalProfile`] holds them as
//! JSON objects; [`Settings::of`] reads them and [`Settings::into_map`] writes them back.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::CanonicalProfile;

/// The three settings groups of a Pro 3 profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Stick configuration.
    pub sticks: Sticks,
    /// Trigger configuration (analog or switch shape).
    pub triggers: Triggers,
    /// Vibration levels.
    pub vibration: Vibration,
}

impl Settings {
    /// Reads the three groups of `profile`.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if a group is missing or does not have the Pro 3's
    /// shape.
    pub fn of(profile: &CanonicalProfile) -> Result<Self> {
        Ok(Self {
            sticks: group(profile, "sticks")?,
            triggers: group(profile, "triggers")?,
            vibration: group(profile, "vibration")?,
        })
    }

    /// The groups as the JSON objects a [`CanonicalProfile`] holds.
    #[must_use]
    pub fn into_map(self) -> Map<String, Value> {
        let json = |v: std::result::Result<Value, serde_json::Error>| v.unwrap_or(Value::Null);
        Map::from_iter([
            ("sticks".to_owned(), json(serde_json::to_value(self.sticks))),
            ("triggers".to_owned(), json(serde_json::to_value(self.triggers))),
            ("vibration".to_owned(), json(serde_json::to_value(self.vibration))),
        ])
    }
}

/// The settings group `name` of `profile` as `T`.
fn group<T: for<'de> Deserialize<'de>>(profile: &CanonicalProfile, name: &str) -> Result<T> {
    let value = profile
        .settings
        .get(name)
        .ok_or_else(|| Error::Validation(format!("the profile has no {name}")))?;
    serde_json::from_value(value.clone())
        .map_err(|e| Error::Validation(format!("the profile's {name} are not a Pro 3's: {e}")))
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
