//! Device-readiness outcome shared by detection and the CLI.

use super::ids::{Mode, Slot};
use crate::error::ErrorCategory;

/// Result of a device-readiness probe.
#[derive(Debug, Clone, Default)]
pub struct DeviceReadiness {
    /// Whether a supported device is connected.
    pub supported_device_connected: bool,
    /// Detected mode, if known.
    pub mode: Option<Mode>,
    /// Active slot marker (`"1"`/`"2"`/`"3"`/`"unknown"`).
    pub active_slot_marker: String,
    /// Whether the marker was verified against live hardware.
    pub active_slot_marker_verified: bool,
    /// Vendor id (lowercase hex).
    pub vendor_id: String,
    /// Product id (lowercase hex).
    pub product_id: String,
    /// Sysfs path of the device.
    pub sysfs_path: String,
    /// Human-readable status message.
    pub message: String,
}

/// Outcome of a write operation, including rollback details on failure.
///
/// Field names match the JSON the C++ CLI emits.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WriteResult {
    /// Whether the write completed.
    pub success: bool,
    /// Human-readable status or error message.
    pub message: String,
    /// Error classification (drives the CLI exit code).
    pub error_category: ErrorCategory,
    /// Target mode.
    pub mode: Mode,
    /// Target slot (1-3).
    pub slot: u8,
    /// Profile id, when the write came from a profile.
    pub profile_id: String,
    /// Whether a rollback was attempted after the failure.
    pub rollback_attempted: bool,
    /// Whether the rollback restored the previous state.
    pub rollback_succeeded: bool,
    /// Where the pre-write backup was saved, when rollback failed and saving worked.
    pub backup_file_path: Option<String>,
}

impl WriteResult {
    /// A failed result with no rollback.
    #[must_use]
    pub fn failure(
        mode: Mode,
        slot: Slot,
        error_category: ErrorCategory,
        message: impl Into<String>,
    ) -> Self {
        Self {
            success: false,
            message: message.into(),
            error_category,
            mode,
            slot: slot.get(),
            profile_id: String::new(),
            rollback_attempted: false,
            rollback_succeeded: false,
            backup_file_path: None,
        }
    }
}
