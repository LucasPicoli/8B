//! `Mode` and the validated `Slot`/`MacroSlot` newtypes.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

/// Longest mode id, in bytes.
pub const MODE_ID_MAX: usize = 15;

/// An operating mode, by the id its controller description gives it, such as `xinput`.
///
/// Each model's description lists its modes. This type only holds the id: 1 to
/// [`MODE_ID_MAX`] bytes of lowercase ASCII letters, digits, `-` or `_`. It stays `Copy`
/// and needs no global table, because the id is stored inline.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Mode {
    /// The id bytes, zero padded. Comes first so the order is the id's order.
    id: [u8; MODE_ID_MAX],
    /// How many bytes of `id` are used.
    len: u8,
}

impl Mode {
    /// Makes a mode from an id known when the program is built, for a driver's own
    /// constants. An id past [`MODE_ID_MAX`] bytes is cut short, so each driver tests
    /// that its constants read back whole. Use [`Mode::new`] for any other text.
    #[must_use]
    pub const fn from_static(id: &'static str) -> Self {
        let mut out = [0u8; MODE_ID_MAX];
        let mut len = 0;
        let mut src = id.as_bytes();
        let mut dst: &mut [u8] = &mut out;
        while let (Some((&b, s)), Some((d, rest))) = (src.split_first(), dst.split_first_mut()) {
            *d = b;
            src = s;
            dst = rest;
            len += 1;
        }
        Self { id: out, len }
    }

    /// Makes a mode from `id`, such as a `-m` argument or the `mode` of a profile file.
    /// Whether a model has the mode is up to its description.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `id` is empty, longer than [`MODE_ID_MAX`]
    /// bytes, or holds a byte other than a lowercase letter, a digit, `-` or `_`.
    pub fn new(id: &str) -> Result<Self> {
        let fits = (1..=MODE_ID_MAX).contains(&id.len());
        let plain =
            id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-_".contains(&b));
        if fits && plain {
            let mut out = [0u8; MODE_ID_MAX];
            out.iter_mut().zip(id.bytes()).for_each(|(d, b)| *d = b);
            let len = u8::try_from(id.len()).map_err(|e| Error::Validation(e.to_string()))?;
            Ok(Self { id: out, len })
        } else {
            Err(Error::Validation(format!(
                "unknown mode '{id}' (a mode id is 1 to {MODE_ID_MAX} lowercase letters, digits, - or _)"
            )))
        }
    }

    /// The id, such as `xinput`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.id.get(..usize::from(self.len)).and_then(|b| std::str::from_utf8(b).ok()).unwrap_or("")
    }
}

impl fmt::Debug for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Mode({:?})", self.as_str())
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
        Self::new(s)
    }
}

impl serde::Serialize for Mode {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for Mode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let id = std::borrow::Cow::<'de, str>::deserialize(d)?;
        Self::new(&id).map_err(serde::de::Error::custom)
    }
}

/// A 1-based profile slot. The model's `slot_count` is the upper bound: the write
/// orchestrator refuses a slot past it, and so does each codec.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot(u8);

impl Slot {
    /// Creates a slot, refusing 0.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] if `value` is 0.
    pub fn new(value: u8) -> Result<Self> {
        if value == 0 {
            Err(Error::Validation("slot 0 out of range (slots start at 1)".to_owned()))
        } else {
            Ok(Self(value))
        }
    }

    /// Refuses a slot past `count`, the model's slot count.
    ///
    /// # Errors
    /// Returns [`Error::Validation`] naming the model's range.
    pub fn check(self, count: u8) -> Result<Self> {
        if self.0 <= count {
            Ok(self)
        } else {
            Err(Error::Validation(format!("slot {} out of range (1-{count})", self.0)))
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
        let xinput = Mode::new("xinput").unwrap();
        assert_eq!(xinput, Mode::from_static("xinput"));
        assert_eq!(xinput.to_string(), "xinput");
        assert_eq!(serde_json::to_value(xinput).unwrap(), "xinput");
        assert_eq!(serde_json::from_value::<Mode>("xinput".into()).unwrap(), xinput);
        assert_eq!(Mode::new("a-much-longer_1").unwrap().as_str(), "a-much-longer_1");
        for bad in ["", "XInput", "x input", "sixteen-letters!", "much-too-long-id"] {
            assert!(bad.parse::<Mode>().is_err(), "{bad}");
        }
    }

    #[test]
    fn modes_sort_by_id() {
        let ids = ["xinput", "switch", "dinput", "d"];
        let mut modes: Vec<Mode> = ids.iter().map(|m| Mode::from_static(m)).collect();
        modes.sort();
        let sorted: Vec<&str> = modes.iter().map(Mode::as_str).collect();
        assert_eq!(sorted, ["d", "dinput", "switch", "xinput"]);
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn slot_range_is_validated() {
        assert!(Slot::new(0).is_err());
        assert_eq!(Slot::new(3).unwrap().get(), 3);
        assert!(Slot::new(3).unwrap().check(3).is_ok());
        assert!(Slot::new(4).unwrap().check(3).is_err());
        assert!(MacroSlot::new(4).is_err());
        assert_eq!(MacroSlot::new(0).unwrap().get(), 0);
    }
}
