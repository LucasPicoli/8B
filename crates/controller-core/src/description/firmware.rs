//! Firmware gates: a description item that only some firmware versions have.

use serde::{Deserialize, Deserializer};

/// A firmware version, written `"1.03"` in a description: a major number, a point and
/// two digits, the way the pad reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct FirmwareVersion {
    /// The number before the point.
    pub major: u16,
    /// The two digits after the point.
    pub minor: u8,
}

/// Digits after the point in a firmware version.
const MINOR_DIGITS: usize = 2;

impl FirmwareVersion {
    /// Parses `"1.09"`, or the version at the start of `"1.09 beta 2"`.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (major, minor) = text.split_whitespace().next()?.split_once('.')?;
        let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
        if !digits(major) || !digits(minor) || minor.len() != MINOR_DIGITS {
            return None;
        }
        Some(Self { major: major.parse().ok()?, minor: minor.parse().ok()? })
    }
}

impl<'de> Deserialize<'de> for FirmwareVersion {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        Self::parse(&text).ok_or_else(|| {
            serde::de::Error::custom(format!("expected a version like \"1.03\", got \"{text}\""))
        })
    }
}

/// Whether an item gated at `min_firmware` shows on a controller that reports
/// `firmware`.
///
/// An item without a gate always shows. A gated item hides when the firmware is older
/// or cannot be read, so an unknown pad never gets a setting it may lack.
#[must_use]
pub fn firmware_allows(min_firmware: Option<FirmwareVersion>, firmware: &str) -> bool {
    min_firmware.is_none_or(|min| FirmwareVersion::parse(firmware).is_some_and(|v| v >= min))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_gate_passes_the_same_and_later_firmware_only() {
        let gate = FirmwareVersion::parse("1.03");
        assert_eq!(gate, Some(FirmwareVersion { major: 1, minor: 3 }));
        assert!(firmware_allows(gate, "1.09"));
        assert!(firmware_allows(gate, "1.03"));
        assert!(firmware_allows(gate, "2.00"));
        assert!(firmware_allows(gate, "1.04 beta 12"));
        assert!(!firmware_allows(gate, "1.02"));
        assert!(!firmware_allows(gate, "0.99"));
    }

    #[test]
    fn unknown_firmware_hides_a_gated_item_only() {
        let gate = FirmwareVersion::parse("1.03");
        for unknown in ["", "abc", "1", "1.3", "1.003", ".03", "1.x3", "-1.03"] {
            assert!(!firmware_allows(gate, unknown), "{unknown:?}");
        }
        assert!(firmware_allows(None, ""));
    }

    #[test]
    fn a_description_writes_a_version_as_a_string() {
        let v: FirmwareVersion = serde_json::from_str("\"1.03\"").unwrap();
        assert_eq!(v, FirmwareVersion { major: 1, minor: 3 });
        assert!(serde_json::from_str::<FirmwareVersion>("\"1.3\"").is_err());
        assert!(serde_json::from_str::<FirmwareVersion>("103").is_err());
    }
}
