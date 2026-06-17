//! Device-readiness outcome shared by detection and the CLI.

use super::ids::Mode;

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
