//! Device detection: sysfs enumeration + active-slot marker check.

use std::path::Path;

use crate::error::Result;
use crate::model::{Mode, Slot};
use crate::protocol::bytes::take;

/// The 4-byte marker written by the 8BitDo app to flag an active profile slot.
pub const ACTIVE_SLOT_MARKER: [u8; 4] = [0x11, 0x09, 0x20, 0x20];

const PROFILE_SIZE: usize = 0x092C;
const FLAG_STRIDE: usize = 4;

/// Returns `true` if `blob` contains the active-slot marker at the position for `slot`.
///
/// Ports `PreWriteReadbackService::isSlotActive`: `flag_offset = (slot-1) * 4`;
/// compares 4 bytes against [`ACTIVE_SLOT_MARKER`].
///
/// Returns `Ok(false)` — not an error — if `blob.len() != 0x092C`.
///
/// # Errors
/// Returns [`crate::Error::Decode`] if the byte-range accessor fails (unreachable
/// when the length check passes, but required by the return type).
pub fn is_slot_active(blob: &[u8], slot: Slot) -> Result<bool> {
    if blob.len() != PROFILE_SIZE {
        return Ok(false);
    }
    let flag_offset = (usize::from(slot.get()) - 1) * FLAG_STRIDE;
    Ok(take(blob, flag_offset, 4)? == ACTIVE_SLOT_MARKER)
}

/// A supported 8BitDo Pro 3 found in the sysfs USB device tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedUsb {
    /// Vendor id, lowercase hex (e.g. `"2dc8"`).
    pub vendor_id: String,
    /// Product id, lowercase hex (e.g. `"310b"`).
    pub product_id: String,
    /// Sysfs directory path for this device.
    pub sysfs_path: String,
    /// Detected operating mode.
    pub mode: Mode,
}

/// Scans the sysfs USB device tree under `root` for a supported 8BitDo Pro 3.
///
/// Reads `idVendor` and `idProduct` from each child directory (trimmed,
/// lowercased). Returns the first entry with vendor `"2dc8"` and product `"310b"`
/// (`XInput`) or `"6009"` (`DInput`). The real root is `/sys/bus/usb/devices`.
#[must_use]
pub fn scan_sysfs(root: &Path) -> Option<DetectedUsb> {
    let dir = std::fs::read_dir(root).ok()?;
    for entry in dir.flatten() {
        let base = entry.path();
        let Some(vendor) = read_trimmed(&base.join("idVendor")) else {
            continue;
        };
        if vendor != "2dc8" {
            continue;
        }
        let Some(product) = read_trimmed(&base.join("idProduct")) else {
            continue;
        };
        let mode = match product.as_str() {
            "310b" => Mode::XInput,
            "6009" => Mode::DInput,
            _ => continue,
        };
        return Some(DetectedUsb {
            vendor_id: vendor,
            product_id: product,
            sysfs_path: base.to_string_lossy().into_owned(),
            mode,
        });
    }
    None
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_lowercase())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::Slot;

    #[test]
    fn slot1_marker_detected() {
        let mut blob = vec![0u8; PROFILE_SIZE];
        blob[0..4].copy_from_slice(&ACTIVE_SLOT_MARKER);
        assert!(is_slot_active(&blob, Slot::new(1).unwrap()).unwrap());
        assert!(!is_slot_active(&blob, Slot::new(2).unwrap()).unwrap());
    }

    #[test]
    fn sysfs_scan_finds_supported_device() {
        let dir = tempfile::tempdir().unwrap();
        let dev_dir = dir.path().join("usb1");
        std::fs::create_dir_all(&dev_dir).unwrap();
        std::fs::write(dev_dir.join("idVendor"), "2dc8\n").unwrap();
        std::fs::write(dev_dir.join("idProduct"), "310b\n").unwrap();
        let found = scan_sysfs(dir.path()).unwrap();
        assert_eq!(found.mode, Mode::XInput);
        assert_eq!(found.product_id, "310b");
    }
}
