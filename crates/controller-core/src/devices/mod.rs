//! Concrete controller backends, and the registry of every supported model.
//!
//! A new model gets its own folder next to `pro3/` and one entry in [`MODELS`].

pub mod pro3;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_pad;

use std::path::Path;

use crate::description::ControllerDescription;
use crate::detect::scan_sysfs_all;
use crate::device::{ConfigPort, Model};
use crate::transport::{DeviceIo, HidrawDevice};

/// Where sysfs lists the USB devices.
pub const SYSFS_USB: &str = "/sys/bus/usb/devices";

/// Every supported controller model.
static MODELS: [&dyn Model; 1] = [&pro3::Pro3];

/// Every supported controller model, in registry order.
#[must_use]
pub fn models() -> &'static [&'static dyn Model] {
    &MODELS
}

/// The description of every supported controller model.
pub fn descriptions() -> impl Iterator<Item = &'static ControllerDescription> {
    // A unit test loads every embedded description, so `ok()` drops nothing in a shipped build.
    models().iter().filter_map(|m| m.description().ok())
}

/// The model among `models` whose description lists `model_id`, the id a `START_CONFIG`
/// reply carries.
#[must_use]
pub fn find_model(models: &[&'static dyn Model], model_id: u16) -> Option<&'static dyn Model> {
    models.iter().copied().find(|m| m.description().is_ok_and(|d| d.model_ids.contains(&model_id)))
}

/// The config ports of every supported model. A USB id that two models share appears
/// once, with the port of the first model that lists it.
// ponytail: models that share a USB id must share its interface and framing; detection
// would have to try each candidate port before identifying the model otherwise.
#[must_use]
pub fn config_ports() -> Vec<ConfigPort> {
    ports_of(models())
}

/// The config ports of `models`, a USB id that two share once, from the first.
fn ports_of(models: &[&'static dyn Model]) -> Vec<ConfigPort> {
    let mut ports: Vec<ConfigPort> = Vec::new();
    let listed = models.iter().filter_map(|m| m.description().ok());
    for port in listed.flat_map(|d| d.config_ports.iter()) {
        if !ports.iter().any(|p| p.usb == port.usb) {
            ports.push(*port);
        }
    }
    ports
}

/// The transport for the controller on USB port path `port`, such as `8-5`, or for the
/// first supported controller when `port` is `None`. Nothing is opened yet.
///
/// The model that lists the controller's USB id picks it: its own
/// [`crate::device::ControllerSpec::transport`], or hidraw with the 8BitDo config
/// protocol. When no supported controller is attached, hidraw reports that on first use.
#[must_use]
pub fn open(port: Option<&str>) -> Box<dyn DeviceIo + Send> {
    own_transport(models(), Path::new(SYSFS_USB), port)
        .unwrap_or_else(|| Box::new(port.map_or_else(HidrawDevice::first, HidrawDevice::at)))
}

/// The transport of the model among `models` that lists the USB id of the controller
/// on `port` (or of the first one found) in sysfs at `root`, if that model has its own.
fn own_transport(
    models: &[&'static dyn Model],
    root: &Path,
    port: Option<&str>,
) -> Option<Box<dyn DeviceIo + Send>> {
    let found = scan_sysfs_all(root, &ports_of(models))
        .into_iter()
        .find(|d| port.is_none_or(|p| d.port_path() == p))?;
    let lists = |m: &&&dyn Model| {
        m.description().is_ok_and(|d| d.config_ports.iter().any(|p| p.usb == found.port.usb))
    };
    models.iter().find(lists)?.transport(found.port_path())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::devices::{pro3::Pro3, test_pad::TestPad};

    /// A sysfs tree with a Pro 3 in `XInput` on `3-1` and the test pad on `3-2`.
    fn sysfs() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (port, product) in [("3-1", "310b"), ("3-2", "7e57")] {
            let at = dir.path().join(port);
            std::fs::create_dir_all(&at).unwrap();
            std::fs::write(at.join("idVendor"), "2dc8\n").unwrap();
            std::fs::write(at.join("idProduct"), format!("{product}\n")).unwrap();
        }
        dir
    }

    #[test]
    fn every_registered_description_loads() {
        for model in models() {
            model.description().unwrap();
        }
    }

    #[test]
    fn a_model_with_its_own_transport_gets_it_by_its_usb_id() {
        let dir = sysfs();
        let models: [&'static dyn Model; 2] = [&Pro3, &TestPad];
        let pad = own_transport(&models, dir.path(), Some("3-2")).unwrap();
        let model = pad.model().unwrap();
        assert_eq!(model.description().unwrap().short_name, "Test Pad");
        // The Pro 3 has no transport of its own, so open() gives it hidraw. So does a
        // port with no supported controller, which then reports that on first use.
        assert!(own_transport(&models, dir.path(), Some("3-1")).is_none());
        assert!(own_transport(&models, dir.path(), Some("9-9")).is_none());
        // Without a port, the first controller by port path decides: the Pro 3 on 3-1.
        assert!(own_transport(&models, dir.path(), None).is_none());
        let pad_only: [&'static dyn Model; 1] = [&TestPad];
        assert!(own_transport(&pad_only, dir.path(), None).is_some());
    }
}
