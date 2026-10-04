//! Device detection: sysfs enumeration, hidraw node lookup, active-slot marker check.

use std::path::{Path, PathBuf};

use crate::device::ConfigPort;
use crate::error::Result;
use crate::model::Slot;
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

/// A supported controller found in the sysfs USB device tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectedUsb {
    /// Vendor id, lowercase hex (e.g. `"2dc8"`).
    pub vendor_id: String,
    /// Product id, lowercase hex (e.g. `"310b"`).
    pub product_id: String,
    /// Sysfs directory path for this device.
    pub sysfs_path: String,
    /// The config port matched by the USB id. Its `mode` is the current mode.
    pub port: ConfigPort,
}

/// Scans the sysfs USB device tree under `root` for a device whose USB id matches
/// one of `ports`. Returns the first match. The real root is `/sys/bus/usb/devices`.
#[must_use]
pub fn scan_sysfs(root: &Path, ports: &[ConfigPort]) -> Option<DetectedUsb> {
    let dir = std::fs::read_dir(root).ok()?;
    for entry in dir.flatten() {
        let base = entry.path();
        let (Some(vendor), Some(product)) =
            (read_trimmed(&base.join("idVendor")), read_trimmed(&base.join("idProduct")))
        else {
            continue;
        };
        let usb = (u16::from_str_radix(&vendor, 16), u16::from_str_radix(&product, 16));
        let Some(port) = ports.iter().find(|p| usb == (Ok(p.usb.vendor), Ok(p.usb.product))) else {
            continue;
        };
        return Some(DetectedUsb {
            vendor_id: vendor,
            product_id: product,
            sysfs_path: base.to_string_lossy().into_owned(),
            port: *port,
        });
    }
    None
}

/// Returns the `/dev/hidrawN` node of USB `interface` of the device at `device_dir`.
///
/// Walks `<device_dir>/<if-dir>/<hid-dir>/hidraw/hidrawN`, where the interface
/// directory carries `bInterfaceNumber` in hex.
#[must_use]
pub fn config_hidraw(device_dir: &Path, interface: u8) -> Option<PathBuf> {
    let iface_dir = std::fs::read_dir(device_dir).ok()?.flatten().map(|e| e.path()).find(|p| {
        read_trimmed(&p.join("bInterfaceNumber")).and_then(|n| u8::from_str_radix(&n, 16).ok())
            == Some(interface)
    })?;
    std::fs::read_dir(iface_dir)
        .ok()?
        .flatten()
        .filter_map(|hid| std::fs::read_dir(hid.path().join("hidraw")).ok())
        .flat_map(Iterator::flatten)
        .map(|node| Path::new("/dev").join(node.file_name()))
        .next()
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_lowercase())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::ControllerSpec as _;
    use crate::devices::pro3::Pro3;
    use crate::model::{Mode, Slot};
    use crate::protocol::framing::Framing;

    #[test]
    fn slot1_marker_detected() {
        let mut blob = vec![0u8; PROFILE_SIZE];
        blob[0..4].copy_from_slice(&ACTIVE_SLOT_MARKER);
        assert!(is_slot_active(&blob, Slot::new(1).unwrap()).unwrap());
        assert!(!is_slot_active(&blob, Slot::new(2).unwrap()).unwrap());
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn sysfs_scan_maps_usb_id_to_current_mode() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("7-1/idVendor"), "2dc8\n");
        write(&dir.path().join("7-1/idProduct"), "3109\n"); // receiver: not a config port
        write(&dir.path().join("8-5/idVendor"), "057e\n");
        write(&dir.path().join("8-5/idProduct"), "2009\n");
        let found = scan_sysfs(dir.path(), &Pro3.description().unwrap().config_ports).unwrap();
        assert_eq!(found.port.mode, Mode::Switch);
        assert_eq!(found.port.framing, Framing::Wrapped);
        assert_eq!(found.product_id, "2009");
    }

    #[test]
    fn config_hidraw_picks_the_config_interface() {
        let dir = tempfile::tempdir().unwrap();
        let dev = dir.path();
        write(&dev.join("8-5:1.0/bInterfaceNumber"), "00\n");
        write(&dev.join("8-5:1.2/bInterfaceNumber"), "02\n");
        write(&dev.join("8-5:1.0/0003:2DC8:310B.0001/hidraw/hidraw4/dev"), "");
        write(&dev.join("8-5:1.2/0003:2DC8:310B.0003/hidraw/hidraw7/dev"), "");
        assert_eq!(config_hidraw(dev, 2), Some(PathBuf::from("/dev/hidraw7")));
        assert_eq!(config_hidraw(dev, 1), None);
    }
}
