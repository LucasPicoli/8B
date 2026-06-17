//! `Mode` and the validated `Slot`/`MacroSlot` newtypes.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// Operating mode of the controller.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// `XInput` mode (product 0x310b).
    XInput,
    /// Nintendo Switch mode (product 0x310b).
    Switch,
    /// `DInput` mode (product 0x6009).
    DInput,
}

impl Mode {
    /// Returns the canonical lowercase string for this mode.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::XInput => "xinput",
            Self::Switch => "switch",
            Self::DInput => "dinput",
        }
    }
}

impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Mode {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        match s {
            "xinput" => Ok(Self::XInput),
            "switch" => Ok(Self::Switch),
            "dinput" => Ok(Self::DInput),
            other => Err(Error::Validation(format!("unknown mode '{other}'"))),
        }
    }
}

/// A 1-based profile slot (1, 2, or 3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot(u8);

impl Slot {
    /// Creates a slot, validating the 1..=3 range.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `value` is not 1, 2, or 3.
    pub fn new(value: u8) -> Result<Self> {
        if (1..=3).contains(&value) {
            Ok(Self(value))
        } else {
            Err(Error::Validation(format!("slot {value} out of range (1-3)")))
        }
    }

    /// Returns the 1-based slot value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// A 0-based macro slot (0..=3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MacroSlot(u8);

impl MacroSlot {
    /// Creates a macro slot, validating the 0..=3 range.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `value` is greater than 3.
    pub fn new(value: u8) -> Result<Self> {
        if value <= 3 {
            Ok(Self(value))
        } else {
            Err(Error::Validation(format!("macro slot {value} out of range (0-3)")))
        }
    }

    /// Returns the 0-based macro slot value.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unwrap_used)]
    #[test]
    fn mode_parses_and_renders() {
        assert_eq!("xinput".parse::<Mode>().unwrap(), Mode::XInput);
        assert_eq!(Mode::Switch.as_str(), "switch");
        assert!("bogus".parse::<Mode>().is_err());
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn slot_range_is_validated() {
        assert!(Slot::new(0).is_err());
        assert_eq!(Slot::new(3).unwrap().get(), 3);
        assert!(MacroSlot::new(4).is_err());
        assert_eq!(MacroSlot::new(0).unwrap().get(), 0);
    }
}
