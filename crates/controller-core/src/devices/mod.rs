//! Concrete controller backends, and the registry of every supported model.
//!
//! A new model gets its own folder next to `pro3/` and one entry in [`MODELS`].

#[cfg(test)]
mod inline_pad;
pub mod pro3;
#[cfg(any(test, feature = "test-utils"))]
pub mod test_pad;
pub mod ultimate;

use std::path::{Path, PathBuf};

use crate::description::ControllerDescription;
use crate::detect::scan_sysfs_all;
use crate::device::{ConfigPort, Model, UsbId};
use crate::transport::{DeviceIo, HidrawDevice};

/// Where sysfs lists the USB devices.
pub const SYSFS_USB: &str = "/sys/bus/usb/devices";

/// Every supported controller model.
static MODELS: [&dyn Model; 3] =
    [&pro3::Pro3, &ultimate::model!("ultimate-bluetooth"), &ultimate::model!("ultimate-2-4g")];

/// Every supported controller model, in registry order.
#[must_use]
pub fn models() -> &'static [&'static dyn Model] {
    &MODELS
}

/// The registered model whose description has the short name `short_name`.
#[must_use]
pub fn model_named(short_name: &str) -> Option<&'static dyn Model> {
    models().iter().copied().find(|m| m.description().is_ok_and(|d| d.short_name == short_name))
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

/// The model among `models` that USB id `usb` names on its own: its description lists
/// no model ids, and no other description lists `usb`.
#[must_use]
pub fn find_model_by_port(models: &[&'static dyn Model], usb: UsbId) -> Option<&'static dyn Model> {
    let lists = |m: &&&dyn Model| {
        m.description().is_ok_and(|d| d.config_ports.iter().any(|p| p.usb == usb))
    };
    match models.iter().filter(lists).collect::<Vec<_>>()[..] {
        [model] if model.description().is_ok_and(|d| d.model_ids.is_empty()) => Some(*model),
        _ => None,
    }
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
pub(crate) fn ports_of(models: &[&'static dyn Model]) -> Vec<ConfigPort> {
    let mut ports: Vec<ConfigPort> = Vec::new();
    let listed = models.iter().filter_map(|m| m.description().ok());
    for port in listed.flat_map(|d| d.config_ports.iter()) {
        if !ports.iter().any(|p| p.usb == port.usb) {
            ports.push(*port);
        }
    }
    ports
}

/// The environment variable that lets a model through that no one has checked on a real
/// pad yet. Set to `1`, the app then saves a backup file before every write.
pub const UNVALIDATED_ENV: &str = "EIGHTB_UNVALIDATED";

/// Whether the user set [`UNVALIDATED_ENV`] to `1`.
#[must_use]
pub fn unvalidated_allowed() -> bool {
    std::env::var(UNVALIDATED_ENV).is_ok_and(|v| v == "1")
}

/// Whether the app refuses the model `description` describes: it is not validated, and
/// `allowed` (see [`unvalidated_allowed`]) is false.
#[must_use]
pub const fn refused_with(description: &ControllerDescription, allowed: bool) -> bool {
    !description.validated && !allowed
}

/// [`refused_with`] as the user's environment sets it.
#[must_use]
pub fn refused(description: &ControllerDescription) -> bool {
    refused_with(description, unvalidated_allowed())
}

/// The class, subclass and protocol of the vendor interface a locked pad shows, which
/// is what the kernel's `xpad` driver binds: `ff/5d/01`. A pad with the same USB id and
/// string that shows only HID is not unlocked.
const LOCKED_INTERFACE: [&str; 3] = ["ff", "5d", "01"];

/// Whether the device at sysfs directory `base` has an interface of class
/// [`LOCKED_INTERFACE`].
fn has_locked_interface(base: &Path) -> bool {
    let Ok(dir) = std::fs::read_dir(base) else { return false };
    let prefix = base.file_name().and_then(|n| n.to_str()).map(|n| format!("{n}:"));
    dir.flatten().any(|e| {
        let name = e.file_name();
        prefix.as_deref().is_some_and(|p| name.to_string_lossy().starts_with(p))
            && ["bInterfaceClass", "bInterfaceSubClass", "bInterfaceProtocol"]
                .iter()
                .zip(LOCKED_INTERFACE)
                .all(|(file, want)| {
                    std::fs::read_to_string(e.path().join(file))
                        .is_ok_and(|v| v.trim().eq_ignore_ascii_case(want))
                })
    })
}

/// A locked controller on the USB bus: one a model's lock declaration names by its USB
/// id and product string, and whose interface class is `ff/5d/01`.
#[derive(Debug, Clone)]
pub struct LockedController {
    /// The model whose lock declaration matched.
    pub model: &'static dyn Model,
    /// The USB port path, such as `8-5`. The config port shows on the same one.
    pub port_path: String,
    /// The usbfs node of the device, such as `/dev/bus/usb/008/102`.
    pub usb_node: PathBuf,
}

/// Every locked controller attached, sorted by port path.
#[must_use]
pub fn locked_ports() -> Vec<LockedController> {
    locked_in(models(), Path::new(SYSFS_USB))
}

/// The locked controllers of `models` in sysfs at `root`, sorted by port path.
fn locked_in(models: &[&'static dyn Model], root: &Path) -> Vec<LockedController> {
    let Ok(dir) = std::fs::read_dir(root) else { return Vec::new() };
    let mut found: Vec<LockedController> =
        dir.flatten()
            .filter_map(|entry| {
                let base = entry.path();
                let attr = |name: &str| {
                    std::fs::read_to_string(base.join(name)).ok().map(|s| s.trim().to_owned())
                };
                let usb = crate::detect::usb_id(&base)?;
                let product = attr("product")?;
                has_locked_interface(&base).then_some(())?;
                let model = models.iter().copied().find(|m| {
                    m.description().ok().and_then(|d| d.lock.as_ref()).is_some_and(|lock| {
                        lock.locked.usb == usb && lock.locked.product == product
                    })
                })?;
                let number = |name: &str| attr(name)?.parse::<u16>().ok();
                let usb_node = PathBuf::from(format!(
                    "/dev/bus/usb/{:03}/{:03}",
                    number("busnum")?,
                    number("devnum")?
                ));
                let port_path = entry.file_name().to_str()?.to_owned();
                Some(LockedController { model, port_path, usb_node })
            })
            .collect();
    found.sort_by(|a, b| a.port_path.cmp(&b.port_path));
    found
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
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::ControllerSpec as _;
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

    /// Adds a USB device on `port` to the sysfs tree at `root`.
    fn device(root: &Path, port: &str, product_id: &str, product: &str, devnum: &str) {
        let at = root.join(port);
        std::fs::create_dir_all(&at).unwrap();
        for (name, value) in [
            ("idVendor", "2dc8"),
            ("idProduct", product_id),
            ("product", product),
            ("busnum", "8"),
            ("devnum", devnum),
        ] {
            std::fs::write(at.join(name), format!("{value}\n")).unwrap();
        }
    }

    /// Gives the device on `port` an interface with class `class`, as `ff/5d/01`.
    fn interface(root: &Path, port: &str, class: [&str; 3]) {
        let at = root.join(port).join(format!("{port}:1.0"));
        std::fs::create_dir_all(&at).unwrap();
        for (name, value) in [
            ("bInterfaceClass", class[0]),
            ("bInterfaceSubClass", class[1]),
            ("bInterfaceProtocol", class[2]),
        ] {
            std::fs::write(at.join(name), format!("{value}\n")).unwrap();
        }
    }

    #[test]
    fn a_locked_controller_is_found_by_its_usb_id_and_product_string() {
        let dir = tempfile::tempdir().unwrap();
        device(dir.path(), "8-5", "3106", "8BitDo Ultimate Controller", "102");
        // The Pro 2 wired shares the locked USB id under another product string.
        device(dir.path(), "8-6", "3106", "8BitDo Pro 2 Wired Controller", "7");
        device(dir.path(), "8-1", "6007", "8BitDo Ultimate Controller", "9");
        for port in ["8-5", "8-6", "8-1"] {
            interface(dir.path(), port, ["ff", "5d", "01"]);
        }
        let found = locked_in(models(), dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].port_path, "8-5");
        assert_eq!(found[0].usb_node, Path::new("/dev/bus/usb/008/102"));
        assert_eq!(found[0].model.description().unwrap().short_name, "Ultimate");
        let no_lock: [&'static dyn Model; 2] = [&Pro3, &TestPad];
        assert!(locked_in(&no_lock, dir.path()).is_empty());
    }

    #[test]
    fn a_locked_id_without_the_vendor_interface_is_never_listed() {
        let dir = tempfile::tempdir().unwrap();
        // The same USB id and string, as HID only, and with the class of another pad.
        device(dir.path(), "8-5", "3106", "8BitDo Ultimate Controller", "102");
        interface(dir.path(), "8-5", ["03", "00", "00"]);
        device(dir.path(), "8-6", "3106", "8BitDo Ultimate Controller", "103");
        device(dir.path(), "8-7", "3106", "8BitDo Ultimate Controller", "104");
        interface(dir.path(), "8-7", ["ff", "5d", "02"]);
        assert!(locked_in(models(), dir.path()).is_empty());
        interface(dir.path(), "8-6", ["ff", "5d", "01"]);
        let found = locked_in(models(), dir.path());
        assert_eq!(found.iter().map(|f| f.port_path.as_str()).collect::<Vec<_>>(), ["8-6"]);
    }

    #[test]
    fn a_model_that_is_not_validated_is_refused_unless_the_user_allows_it() {
        let unchecked = test_pad::UnvalidatedPad.description().unwrap();
        assert!(!unchecked.validated);
        assert!(refused_with(unchecked, false));
        assert!(!refused_with(unchecked, true), "EIGHTB_UNVALIDATED=1 lets it through");
        // The models checked on a real pad. A model no one has checked yet stays out of this list.
        for name in ["Pro 3", "Ultimate"] {
            let checked = model_named(name).unwrap().description().unwrap();
            assert!(!refused_with(checked, false), "{name}");
        }
        assert!(!refused_with(TestPad.description().unwrap(), false));
    }

    #[test]
    fn every_registered_description_loads() {
        for model in models() {
            model.description().unwrap();
        }
    }

    #[test]
    fn a_model_named_by_its_port_must_be_alone_on_it() {
        let pad = TestPad.description().unwrap().config_ports[0].usb;
        let pro3 = Pro3.description().unwrap().config_ports[0].usb;
        let both: [&'static dyn Model; 2] = [&Pro3, &TestPad];
        assert!(find_model_by_port(&both, pad).is_some());
        // The Pro 3 lists model ids, so its port names no model.
        assert!(find_model_by_port(&both, pro3).is_none());
        // Two descriptions that list the same port: the port names neither.
        let twice: [&'static dyn Model; 2] = [&TestPad, &TestPad];
        assert!(find_model_by_port(&twice, pad).is_none());
        // Every registered model without model ids is found by each of its ports.
        for model in models() {
            let d = model.description().unwrap();
            if d.model_ids.is_empty() {
                for port in &d.config_ports {
                    assert!(find_model_by_port(models(), port.usb).is_some(), "{}", d.short_name);
                }
            }
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
