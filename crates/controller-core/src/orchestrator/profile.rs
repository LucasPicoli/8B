//! Profile orchestrator: detect → read pipeline and raw-blob dump helper.
//!
//! Ports `ProfileOrchestrator::detectAndReadAll` and the read step of
//! `runDump` from `src/core/profile_orchestrator.cpp`.

use crate::devices::pro3::Pro3;
use crate::error::{Error, ErrorCategory, Result};
use crate::model::{CanonicalProfileSummary, DeviceReadiness, Mode};
use crate::service::read;
use crate::transport::device_io::DeviceIo;

/// Result of a full detect → read pipeline.
///
/// Mirrors the C++ `core::DetectAndReadResult` struct field-for-field.
/// The CLI (T14) hand-builds JSON from this; do **not** derive `Serialize`.
#[derive(Debug, Clone)]
pub struct DetectAndReadResult {
    /// Whether a supported device was found and profiles were read successfully.
    pub success: bool,
    /// Human-readable status message suitable for display.
    pub message: String,
    /// Error category for deterministic exit-code selection.
    pub error_category: ErrorCategory,
    /// Device mode detected during readiness probe (e.g., `XInput`, `Switch`).
    pub mode: Option<Mode>,
    /// Device product id as lowercase hex string (e.g., `"310b"`).
    pub product_id: String,
    /// Active slot marker value from hardware probe (e.g., `"1"`, `"2"`, `"3"`).
    pub active_slot_marker: String,
    /// Whether the active slot marker was verified against live hardware.
    pub active_slot_marker_verified: bool,
    /// Canonical profile summaries from all detected modes.
    pub profiles: Vec<CanonicalProfileSummary>,
    /// Raw profile blobs for diagnostic dump.
    pub raw_blobs: Vec<Vec<u8>>,
}

/// Builds a failure [`DetectAndReadResult`] from an [`Error`], with empty device fields.
fn failure_from_err(e: &Error) -> DetectAndReadResult {
    DetectAndReadResult {
        success: false,
        message: e.to_string(),
        error_category: e.category(),
        mode: None,
        product_id: String::new(),
        active_slot_marker: String::new(),
        active_slot_marker_verified: false,
        profiles: vec![],
        raw_blobs: vec![],
    }
}

/// Builds a failure [`DetectAndReadResult`] carrying readiness metadata.
fn failure_with_readiness(
    category: ErrorCategory,
    message: String,
    readiness: &DeviceReadiness,
) -> DetectAndReadResult {
    DetectAndReadResult {
        success: false,
        message,
        error_category: category,
        mode: readiness.mode,
        product_id: readiness.product_id.clone(),
        active_slot_marker: readiness.active_slot_marker.clone(),
        active_slot_marker_verified: readiness.active_slot_marker_verified,
        profiles: vec![],
        raw_blobs: vec![],
    }
}

/// Detects a connected device and reads all on-device profiles.
///
/// Infallible at the signature level — failures are captured in the returned
/// struct (`success = false`, `error_category` set). Ports
/// `ProfileOrchestrator::detectAndReadAll` from the C++ reference.
///
/// # Steps
/// 1. Probe device readiness via [`DeviceIo::detect_readiness`].
/// 2. Reject unsupported or undetected devices.
/// 3. Require a known mode.
/// 4. Read all profiles via [`read::read_profiles`].
#[must_use]
pub fn detect_and_read_all(dev: &dyn DeviceIo, _codec: &Pro3) -> DetectAndReadResult {
    // 1. Probe readiness.
    let readiness = match dev.detect_readiness() {
        Ok(r) => r,
        Err(e) => return failure_from_err(&e),
    };

    // 2. Reject unsupported/disconnected devices.
    if !readiness.supported_device_connected {
        return failure_with_readiness(
            ErrorCategory::ConnectionFailure,
            readiness.message.clone(),
            &readiness,
        );
    }

    // 3. Require a known mode.
    let Some(mode) = readiness.mode else {
        return failure_with_readiness(
            ErrorCategory::ConnectionFailure,
            "device mode unknown".into(),
            &readiness,
        );
    };

    // 4. Read profiles.
    match read::read_profiles(dev, mode) {
        Ok(rr) => DetectAndReadResult {
            success: true,
            message: "Profiles read successfully.".into(),
            error_category: ErrorCategory::None,
            mode: Some(mode),
            product_id: readiness.product_id,
            active_slot_marker: readiness.active_slot_marker,
            active_slot_marker_verified: readiness.active_slot_marker_verified,
            profiles: rr.profiles,
            raw_blobs: rr.raw_blobs,
        },
        Err(e) => failure_with_readiness(e.category(), e.to_string(), &readiness),
    }
}

/// Detects a connected device, reads all profiles, and returns the raw blobs.
///
/// Ports the read step of `runDump` from `src/main.cpp`.
/// Returns `Err` if detection or reading fails, preserving the error category.
///
/// # Errors
/// - [`Error::Usb`] for connection or general failures.
/// - [`Error::Timeout`] if the device timed out.
/// - [`Error::Validation`] if validation failed.
pub fn dump_blobs(dev: &dyn DeviceIo, codec: &Pro3) -> Result<Vec<Vec<u8>>> {
    let result = detect_and_read_all(dev, codec);
    if !result.success {
        let msg = result.message;
        return Err(match result.error_category {
            ErrorCategory::Timeout => Error::Timeout,
            ErrorCategory::ValidationFailure => Error::Validation(msg),
            _ => Error::Usb(msg),
        });
    }
    Ok(result.raw_blobs)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::{DeviceReadiness, Mode, ProfileReadResult};
    use crate::transport::mock::MockDevice;

    /// A minimal [`CanonicalProfileSummary`] for test use.
    fn dummy_summary() -> crate::model::CanonicalProfileSummary {
        use crate::model::{
            ButtonMapping, CanonicalProfile, CanonicalProfileSummary, MacroRef, Sticks, Triggers,
            TriggersAnalog, Vibration,
        };
        CanonicalProfileSummary {
            id: "test-id".into(),
            name: "Test Profile".into(),
            mode: Mode::XInput,
            source_slot: 1,
            source_profile_index: 0,
            canonical: CanonicalProfile {
                id: "test-id".into(),
                name: "Test Profile".into(),
                version: 1,
                kind: "8bitdo.pro3.profile".into(),
                device: "8bitdo-pro3".into(),
                mode: Mode::XInput,
                preferred_slot: None,
                sticks: Sticks {
                    left_min_pct: 0,
                    left_max_pct: 100,
                    right_min_pct: 0,
                    right_max_pct: 100,
                    invert_left_x: false,
                    invert_left_y: false,
                    invert_right_x: false,
                    invert_right_y: false,
                    swap_sticks: false,
                    swap_dpad_with_left_stick: false,
                },
                triggers: Triggers::Analog(TriggersAnalog {
                    left_min_pct: 0,
                    left_max_pct: 100,
                    right_min_pct: 0,
                    right_max_pct: 100,
                    swap_triggers: false,
                }),
                vibration: Vibration { left_level: 3, right_level: 3 },
                button_mappings: Vec::<ButtonMapping>::new(),
                macro_refs: Vec::<MacroRef>::new(),
            },
        }
    }

    #[test]
    fn detect_and_read_reports_no_device() {
        let dev = MockDevice::new();
        let out = detect_and_read_all(&dev, &Pro3);
        assert!(!out.success, "expected failure without device");
        assert_eq!(out.error_category, ErrorCategory::ConnectionFailure);
    }

    #[test]
    fn detect_and_read_succeeds_with_mock() {
        let readiness = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(Mode::XInput),
            product_id: "310b".into(),
            active_slot_marker: "1".into(),
            active_slot_marker_verified: true,
            ..Default::default()
        };
        let profiles_result = ProfileReadResult {
            profiles: vec![dummy_summary()],
            raw_blobs: vec![vec![0u8; 0x092C], vec![0u8; 0x092C]],
        };
        let dev = MockDevice::new()
            .with_readiness(readiness)
            .with_profiles(Mode::XInput, profiles_result);

        let out = detect_and_read_all(&dev, &Pro3);
        assert!(out.success, "expected success with mock device");
        assert_eq!(out.mode, Some(Mode::XInput));
        assert_eq!(out.product_id, "310b");
        assert_eq!(out.profiles.len(), 1);
        assert_eq!(out.raw_blobs.len(), 2);
    }

    #[test]
    fn dump_blobs_returns_blobs() {
        let readiness = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(Mode::XInput),
            product_id: "310b".into(),
            active_slot_marker: "1".into(),
            active_slot_marker_verified: true,
            ..Default::default()
        };
        let profiles_result = ProfileReadResult {
            profiles: vec![dummy_summary()],
            raw_blobs: vec![vec![0u8; 0x092C], vec![0u8; 0x092C]],
        };
        let dev = MockDevice::new()
            .with_readiness(readiness)
            .with_profiles(Mode::XInput, profiles_result);

        let blobs = dump_blobs(&dev, &Pro3).expect("dump_blobs should succeed");
        assert_eq!(blobs.len(), 2);
    }

    #[test]
    fn dump_blobs_errors_without_device() {
        let dev = MockDevice::new();
        let result = dump_blobs(&dev, &Pro3);
        assert!(result.is_err(), "expected Err without device");
    }
}
