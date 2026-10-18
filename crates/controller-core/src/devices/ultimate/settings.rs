//! The Ultimate's settings groups as typed structs. A [`CanonicalProfile`] holds them as
//! JSON objects; [`Settings::of`] reads them and [`Settings::into_map`] writes them back.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::{Error, Result};
use crate::model::CanonicalProfile;

/// The three settings groups of an Ultimate profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    /// Stick ranges and toggles.
    pub sticks: Sticks,
    /// Trigger ranges and the trigger swap.
    pub triggers: Triggers,
    /// Vibration levels.
    pub vibration: Vibration,
}

impl Settings {
    /// Reads the three groups of `profile`.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if a group is missing or does not have the
    /// Ultimate's shape.
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
        .map_err(|e| Error::Validation(format!("the profile's {name} are not an Ultimate's: {e}")))
}

/// Stick ranges, 0 to 100 each, and the stick toggles.
#[allow(clippy::struct_excessive_bools)] // one bool per toggle the pad stores
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sticks {
    /// Left stick: the travel below which it reads as centred.
    pub left_min_pct: i32,
    /// Left stick: the travel past which it reads as pushed all the way.
    pub left_max_pct: i32,
    /// Right stick: the travel below which it reads as centred.
    pub right_min_pct: i32,
    /// Right stick: the travel past which it reads as pushed all the way.
    pub right_max_pct: i32,
    /// Invert the left stick's X axis.
    pub invert_left_x: bool,
    /// Invert the left stick's Y axis.
    pub invert_left_y: bool,
    /// Invert the right stick's X axis.
    pub invert_right_x: bool,
    /// Invert the right stick's Y axis.
    pub invert_right_y: bool,
    /// Swap the left and right sticks.
    pub swap_sticks: bool,
    /// Swap the D-pad and the left stick.
    pub swap_dpad_with_left_stick: bool,
    /// Swap the right stick and the triggers.
    pub swap_right_stick_with_triggers: bool,
    /// Turn the stick dead zone off.
    pub no_dead_zone: bool,
    /// The D-pad gives four directions only.
    pub dpad_four_way: bool,
}

/// Trigger ranges, 0 to 100 each, and the trigger swap.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Triggers {
    /// Left trigger: the travel below which it reads as released.
    pub left_min_pct: i32,
    /// Left trigger: the travel past which it reads as pulled all the way.
    pub left_max_pct: i32,
    /// Right trigger: the travel below which it reads as released.
    pub right_min_pct: i32,
    /// Right trigger: the travel past which it reads as pulled all the way.
    pub right_max_pct: i32,
    /// Swap the left and right triggers.
    pub swap_triggers: bool,
}

/// Vibration levels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vibration {
    /// Left motor level, 0 to 5.
    pub left_level: i32,
    /// Right motor level, 0 to 5.
    pub right_level: i32,
}
