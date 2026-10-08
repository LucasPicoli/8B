//! Raw hidraw access for reverse engineering a new controller.
//!
//! Nothing here knows a protocol: [`list_hidraw`] lists every node the kernel has, USB and
//! Bluetooth alike, and [`exchange`] sends the bytes it is given and returns every report
//! that comes back. The supported-model path is [`super::HidrawDevice`].

use std::fs::OpenOptions;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::io::Errno;

use crate::error::{Error, Result};
use crate::transport::session::{errno_error, io_error, open_error};

/// Where the kernel lists hidraw nodes.
pub const SYSFS_HIDRAW: &str = "/sys/class/hidraw";
/// HID bus number of USB, as in the `HID_ID` uevent key.
pub const BUS_USB: u16 = 0x0003;
/// HID bus number of Bluetooth.
pub const BUS_BLUETOOTH: u16 = 0x0005;
/// Read buffer for one report. Larger than any report a gamepad sends.
const MAX_REPORT_LEN: usize = 4096;

/// One hidraw node and what sysfs says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HidrawNode {
    /// The device node, such as `/dev/hidraw4`.
    pub node: PathBuf,
    /// HID bus number: [`BUS_USB`], [`BUS_BLUETOOTH`] or another.
    pub bus: u16,
    /// Vendor id.
    pub vendor: u16,
    /// Product id.
    pub product: u16,
    /// USB interface number. `None` off USB.
    pub interface: Option<u8>,
    /// Device name the kernel reports.
    pub name: String,
    /// The HID report descriptor: it gives each report's id and size.
    pub report_descriptor: Vec<u8>,
}

impl HidrawNode {
    /// The bus as a word: `usb`, `bluetooth` or `bus 0x0019`.
    #[must_use]
    pub fn bus_name(&self) -> String {
        match self.bus {
            BUS_USB => "usb".to_owned(),
            BUS_BLUETOOTH => "bluetooth".to_owned(),
            other => format!("bus 0x{other:04x}"),
        }
    }
}

/// One report read back by [`exchange`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Time from the end of the send to the read.
    pub after: Duration,
    /// The report as read, report id first if the device uses ids.
    pub data: Vec<u8>,
}

/// Every hidraw node under `root`, in node-number order. The real root is
/// [`SYSFS_HIDRAW`]. A node whose uevent cannot be read is left out.
#[must_use]
pub fn list_hidraw(root: &Path) -> Vec<HidrawNode> {
    let Ok(dir) = std::fs::read_dir(root) else { return Vec::new() };
    let mut nodes: Vec<(u32, HidrawNode)> = dir
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = name.strip_prefix("hidraw")?.parse().ok()?;
            let device = entry.path().join("device");
            let uevent = std::fs::read_to_string(device.join("uevent")).ok()?;
            let mut node = parse_uevent(&uevent)?;
            node.node = Path::new("/dev").join(&name);
            node.report_descriptor =
                std::fs::read(device.join("report_descriptor")).unwrap_or_default();
            Some((number, node))
        })
        .collect();
    nodes.sort_by_key(|(number, _)| *number);
    nodes.into_iter().map(|(_, node)| node).collect()
}

/// Reads bus, ids, name and USB interface from a HID uevent. `None` without `HID_ID`.
fn parse_uevent(text: &str) -> Option<HidrawNode> {
    let value = |key: &str| {
        text.lines().find_map(|line| line.strip_prefix(key)?.strip_prefix('=')).map(str::trim)
    };
    // `HID_ID=0003:00002DC8:00006009`: bus, vendor, product.
    let mut id = value("HID_ID")?.split(':').map(|part| u32::from_str_radix(part, 16).ok());
    let mut next = || id.next().flatten().and_then(|n| u16::try_from(n).ok());
    let (bus, vendor, product) = (next()?, next()?, next()?);
    // `HID_PHYS=usb-0000:10:00.0-5/input2` ends in the USB interface number.
    let interface = (bus == BUS_USB)
        .then(|| value("HID_PHYS")?.rsplit_once("/input")?.1.parse().ok())
        .flatten();
    Some(HidrawNode {
        node: PathBuf::new(),
        bus,
        vendor,
        product,
        interface,
        name: value("HID_NAME").unwrap_or_default().to_owned(),
        report_descriptor: Vec::new(),
    })
}

/// Writes `packet` to `node` as one report, then collects every report that arrives in
/// the next `listen`. Input reports are kept too: the caller sees the wire as it is.
///
/// Nothing is added to `packet`. A device that uses report ids needs the id as its first
/// byte.
///
/// # Errors
/// Returns [`Error::PermissionDenied`] if the node may not be opened, [`Error::Usb`] if it
/// fails to open otherwise or the write fails, and [`Error::Disconnected`] if the device
/// goes away.
pub fn exchange(node: &Path, packet: &[u8], listen: Duration) -> Result<Vec<Report>> {
    let mut file =
        OpenOptions::new().read(true).write(true).open(node).map_err(|e| open_error(node, &e))?;
    file.write_all(packet).map_err(|e| io_error(&e))?;
    let start = Instant::now();
    let mut reports = Vec::new();
    let mut buf = vec![0u8; MAX_REPORT_LEN];
    loop {
        let left = listen.saturating_sub(start.elapsed());
        if left.is_zero() {
            return Ok(reports);
        }
        let ts = Timespec::try_from(left).map_err(|_| Error::Timeout)?;
        match poll(&mut [PollFd::new(&file, PollFlags::IN)], Some(&ts)) {
            Ok(0) | Err(Errno::INTR) => continue,
            Ok(_) => {}
            Err(e) => return Err(errno_error(e)),
        }
        let n = file.read(&mut buf).map_err(|e| io_error(&e))?;
        let data = buf.get(..n).unwrap_or_default().to_vec();
        reports.push(Report { after: start.elapsed(), data });
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    const PRO3_DINPUT: &str = "DRIVER=hid-generic\nHID_ID=0003:00002DC8:00006009\n\
        HID_NAME=8BitDo 8BitDo Pro 3\nHID_PHYS=usb-0000:10:00.0-5/input2\nHID_UNIQ=\n";

    #[test]
    fn uevent_gives_ids_name_and_interface() {
        let node = parse_uevent(PRO3_DINPUT).unwrap();
        assert_eq!((node.bus, node.vendor, node.product), (BUS_USB, 0x2dc8, 0x6009));
        assert_eq!(node.interface, Some(2));
        assert_eq!(node.name, "8BitDo 8BitDo Pro 3");
        assert_eq!(node.bus_name(), "usb");
    }

    #[test]
    fn bluetooth_node_has_no_interface() {
        let text = "HID_ID=0005:0000057E:00002009\nHID_NAME=Pro Controller\n\
            HID_PHYS=aa:bb:cc:dd:ee:ff\n";
        let node = parse_uevent(text).unwrap();
        assert_eq!((node.bus_name(), node.interface), ("bluetooth".to_owned(), None));
        assert!(parse_uevent("HID_NAME=no id\n").is_none());
    }

    #[test]
    fn list_reads_sysfs_in_node_order() {
        let root = tempfile::tempdir().unwrap();
        for (name, uevent) in [("hidraw10", PRO3_DINPUT), ("hidraw4", PRO3_DINPUT)] {
            let device = root.path().join(name).join("device");
            std::fs::create_dir_all(&device).unwrap();
            std::fs::write(device.join("uevent"), uevent).unwrap();
            std::fs::write(device.join("report_descriptor"), [0x05, 0x01]).unwrap();
        }
        let nodes = list_hidraw(root.path());
        let paths: Vec<_> = nodes.iter().map(|n| n.node.clone()).collect();
        assert_eq!(paths, [PathBuf::from("/dev/hidraw4"), PathBuf::from("/dev/hidraw10")]);
        assert_eq!(nodes[0].report_descriptor, [0x05, 0x01]);
    }
}
