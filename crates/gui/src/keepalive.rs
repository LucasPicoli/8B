//! Holds the `xpad` event node of a controller open while 8B runs. A Pro 3 in `XInput`
//! mode resets about half a second after nothing polls it, and `xpad` polls a wired pad
//! only while its event node is open. With the node held, the pad stays put while the
//! user edits it, with no udev keepalive installed.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::PathBuf;

use controller_core::detect::{xpad_event, DetectedUsb};

/// One controller whose event node should be held: its port path, its USB device
/// number and the node to open.
type Wanted = (String, Option<u16>, PathBuf);

/// The controllers in `found` whose port sets `needs_keepalive` and whose event node
/// exists yet. A replug gets a new device number, so it is a new entry.
fn wanted(found: &[DetectedUsb], devnum: impl Fn(&DetectedUsb) -> Option<u16>) -> Vec<Wanted> {
    found
        .iter()
        .filter(|usb| usb.port.needs_keepalive)
        .filter_map(|usb| {
            let node = xpad_event(std::path::Path::new(&usb.sysfs_path))?;
            Some((usb.port_path().to_owned(), devnum(usb), node))
        })
        .collect()
}

/// The event nodes held open, by port path. A failed open is kept as `None`, so it is
/// logged once and not tried again until the controller is replugged.
#[derive(Debug, Default)]
pub struct Keepalive {
    held: BTreeMap<String, (Option<u16>, Option<File>)>,
}

impl Keepalive {
    /// Opens the event node of each new controller in `found` and drops the handle of
    /// each one that is gone or replugged. Call it on every poll.
    pub fn sync(&mut self, found: &[DetectedUsb], devnum: impl Fn(&DetectedUsb) -> Option<u16>) {
        let wanted = wanted(found, devnum);
        self.held.retain(|port, (num, _)| wanted.iter().any(|(p, n, _)| p == port && n == num));
        for (port, num, node) in wanted {
            if self.held.contains_key(&port) {
                continue;
            }
            let file = File::open(&node)
                .map_err(|e| eprintln!("8b: cannot hold {}: {e}", node.display()))
                .ok();
            self.held.insert(port, (num, file));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;

    use controller_core::detect::scan_sysfs_all;
    use controller_core::device::ControllerSpec as _;
    use controller_core::devices::pro3::Pro3;

    use super::*;

    /// Plugs a device with USB id `vendor:product` at `port`, with an `xpad` event node.
    fn plug(root: &std::path::Path, port: &str, vendor: &str, product: &str) {
        let dev = root.join(port);
        fs::create_dir_all(dev.join(format!("{port}:1.0/input/input9/event9"))).unwrap();
        fs::write(dev.join("idVendor"), vendor).unwrap();
        fs::write(dev.join("idProduct"), product).unwrap();
        symlink("../../bus/usb/drivers/xpad", dev.join(format!("{port}:1.0/driver"))).unwrap();
    }

    #[test]
    fn only_ports_with_the_flag_are_held() {
        let root = tempfile::tempdir().unwrap();
        plug(root.path(), "3-1", "2dc8", "310b"); // XInput: needs the keepalive
        plug(root.path(), "3-2", "2dc8", "6009"); // DInput: does not
        let ports = &Pro3.description().unwrap().config_ports;
        let found = scan_sysfs_all(root.path(), ports);
        let held = wanted(&found, |_| Some(7));
        assert_eq!(held, [("3-1".to_owned(), Some(7), PathBuf::from("/dev/input/event9"))]);
    }

    #[test]
    fn a_node_that_is_not_there_yet_is_skipped() {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("3-1")).unwrap();
        fs::write(root.path().join("3-1/idVendor"), "2dc8").unwrap();
        fs::write(root.path().join("3-1/idProduct"), "310b").unwrap();
        let ports = &Pro3.description().unwrap().config_ports;
        let found = scan_sysfs_all(root.path(), ports);
        assert_eq!(wanted(&found, |_| None).len(), 0);
    }
}
