//! A USB device node, `/dev/bus/usb/BBB/DDD`, through the usbfs ioctls.
//!
//! The ioctls of `linux/usbdevice_fs.h` take an interface from its kernel driver,
//! claim it, write interrupt OUT transfers, release it and give it back.
//!
//! A locked controller shows no config port, only an interface a kernel driver such
//! as `xpad` holds, so its unlock sequence goes here. The workspace denies `unsafe`.
//! This module allows it for the ioctl calls, each with its contract stated.
#![allow(unsafe_code)]

use std::ffi::{c_int, c_uint, c_void};
use std::fs::{File, OpenOptions};
use std::path::Path;

use rustix::io::Errno;
use rustix::ioctl::{ioctl, opcode, Opcode, Setter, Updater};

use super::session::{errno_error, open_error};
use crate::error::Result;

/// The ioctl group of usbfs, `'U'`.
const USBFS_GROUP: u8 = b'U';
/// How long one interrupt OUT transfer may take, in milliseconds.
const TRANSFER_TIMEOUT_MS: c_uint = 1000;

/// `struct usbdevfs_bulktransfer`, which `USBDEVFS_BULK` also takes for an interrupt
/// endpoint.
#[repr(C)]
struct BulkTransfer {
    /// The endpoint address.
    ep: c_uint,
    /// The bytes at `data`.
    len: c_uint,
    /// In milliseconds.
    timeout: c_uint,
    /// The bytes to send.
    data: *mut c_void,
}

/// `struct usbdevfs_ioctl`: an ioctl for the driver of interface `ifno`.
#[repr(C)]
struct DriverIoctl {
    /// The interface number.
    ifno: c_int,
    /// [`DISCONNECT`] or [`CONNECT`].
    ioctl_code: c_int,
    /// Unused by both: null.
    data: *mut c_void,
}

/// `USBDEVFS_BULK`, `_IOWR('U', 2, struct usbdevfs_bulktransfer)`.
const BULK: Opcode = opcode::read_write::<BulkTransfer>(USBFS_GROUP, 2);
/// `USBDEVFS_CLAIMINTERFACE`, `_IOR('U', 15, unsigned int)`.
const CLAIM_INTERFACE: Opcode = opcode::read::<c_uint>(USBFS_GROUP, 15);
/// `USBDEVFS_RELEASEINTERFACE`, `_IOR('U', 16, unsigned int)`.
const RELEASE_INTERFACE: Opcode = opcode::read::<c_uint>(USBFS_GROUP, 16);
/// `USBDEVFS_IOCTL`, `_IOWR('U', 18, struct usbdevfs_ioctl)`.
const DRIVER_IOCTL: Opcode = opcode::read_write::<DriverIoctl>(USBFS_GROUP, 18);
/// `USBDEVFS_DISCONNECT`, `_IO('U', 22)`: detach the kernel driver.
const DISCONNECT: Opcode = opcode::none(USBFS_GROUP, 22);
/// `USBDEVFS_CONNECT`, `_IO('U', 23)`: attach the kernel driver again.
const CONNECT: Opcode = opcode::none(USBFS_GROUP, 23);

/// The steps of sending to one interface of a USB device. [`UsbNode`] does them on a
/// device node; a test fake records them.
pub trait UsbIo {
    /// Detaches the kernel driver from `interface`. An interface no driver holds is
    /// not an error.
    ///
    /// # Errors
    /// Returns [`crate::Error::Disconnected`] if the device is gone, and
    /// [`crate::Error::Usb`] on any other failure.
    fn disconnect(&mut self, interface: u8) -> Result<()>;
    /// Claims `interface` for this process.
    ///
    /// # Errors
    /// As [`Self::disconnect`].
    fn claim(&mut self, interface: u8) -> Result<()>;
    /// Writes `data` to interrupt OUT endpoint `endpoint`.
    ///
    /// # Errors
    /// As [`Self::disconnect`].
    fn interrupt_out(&mut self, endpoint: u8, data: &[u8]) -> Result<()>;
    /// Releases `interface`.
    ///
    /// # Errors
    /// As [`Self::disconnect`].
    fn release(&mut self, interface: u8) -> Result<()>;
    /// Attaches the kernel driver to `interface` again.
    ///
    /// # Errors
    /// As [`Self::disconnect`].
    fn connect(&mut self, interface: u8) -> Result<()>;
}

/// An open USB device node.
#[derive(Debug)]
pub struct UsbNode(File);

impl UsbNode {
    /// Opens the device node `node`, such as `/dev/bus/usb/008/102`, to read and write.
    ///
    /// # Errors
    /// Returns [`crate::Error::PermissionDenied`] when the node may not be opened, as
    /// for a hidraw node, and [`crate::Error::Usb`] when it fails otherwise.
    pub fn open(node: &Path) -> Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open(node);
        file.map(Self).map_err(|e| open_error(node, &e))
    }

    /// Runs `ioctl_code` on the driver of `interface`.
    fn driver(&self, interface: u8, ioctl_code: Opcode) -> rustix::io::Result<()> {
        let mut request = DriverIoctl {
            ifno: c_int::from(interface),
            ioctl_code: c_int::try_from(ioctl_code).map_err(|_| Errno::INVAL)?,
            data: std::ptr::null_mut(),
        };
        // SAFETY: `DRIVER_IOCTL` is `USBDEVFS_IOCTL`, whose argument is a
        // `struct usbdevfs_ioctl`, which `DriverIoctl` lays out with `repr(C)`. The
        // kernel reads it for the length of the call, while `request` lives.
        // `DISCONNECT` and `CONNECT` take no data, so `data` is null.
        unsafe { ioctl(&self.0, Updater::<DRIVER_IOCTL, DriverIoctl>::new(&mut request)) }
    }

    /// Runs the interface ioctl `OPCODE` on `interface`.
    fn interface<const OPCODE: Opcode>(&self, interface: u8) -> Result<()> {
        // SAFETY: `OPCODE` is `USBDEVFS_CLAIMINTERFACE` or `USBDEVFS_RELEASEINTERFACE`,
        // whose argument is a pointer to an `unsigned int`, the interface number.
        // `Setter` passes a pointer to its own copy for the length of the call.
        unsafe { ioctl(&self.0, Setter::<OPCODE, c_uint>::new(c_uint::from(interface))) }
            .map_err(errno_error)
    }
}

impl UsbIo for UsbNode {
    fn disconnect(&mut self, interface: u8) -> Result<()> {
        match self.driver(interface, DISCONNECT) {
            // No driver holds the interface.
            Ok(()) | Err(Errno::NODATA) => Ok(()),
            Err(e) => Err(errno_error(e)),
        }
    }

    fn claim(&mut self, interface: u8) -> Result<()> {
        self.interface::<CLAIM_INTERFACE>(interface)
    }

    fn interrupt_out(&mut self, endpoint: u8, data: &[u8]) -> Result<()> {
        let mut bytes = data.to_vec();
        let mut transfer = BulkTransfer {
            ep: c_uint::from(endpoint),
            len: c_uint::try_from(bytes.len()).map_err(|_| errno_error(Errno::INVAL))?,
            timeout: TRANSFER_TIMEOUT_MS,
            data: bytes.as_mut_ptr().cast(),
        };
        // SAFETY: `BULK` is `USBDEVFS_BULK`, whose argument is a
        // `struct usbdevfs_bulktransfer`, which `BulkTransfer` lays out with `repr(C)`.
        // For an OUT endpoint the kernel reads `len` bytes at `data`, which points at
        // `bytes`, `len` long, alive and unmoved until the call returns.
        unsafe { ioctl(&self.0, Updater::<BULK, BulkTransfer>::new(&mut transfer)) }
            .map_err(errno_error)
    }

    fn release(&mut self, interface: u8) -> Result<()> {
        self.interface::<RELEASE_INTERFACE>(interface)
    }

    fn connect(&mut self, interface: u8) -> Result<()> {
        self.driver(interface, CONNECT).map_err(errno_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The opcodes as the C macros of `linux/usbdevice_fs.h` expand them on a 64-bit
    /// target.
    #[cfg(target_pointer_width = "64")]
    #[test]
    fn the_opcodes_match_linux_usbdevice_fs_h() {
        assert_eq!(u64::from(BULK), 0xC018_5502);
        assert_eq!(u64::from(CLAIM_INTERFACE), 0x8004_550F);
        assert_eq!(u64::from(RELEASE_INTERFACE), 0x8004_5510);
        assert_eq!(u64::from(DRIVER_IOCTL), 0xC010_5512);
        assert_eq!(u64::from(DISCONNECT), 0x5516);
        assert_eq!(u64::from(CONNECT), 0x5517);
    }

    #[test]
    fn a_node_that_is_not_usb_takes_no_ioctl() {
        let Ok(null) = File::open("/dev/null") else { return };
        let mut node = UsbNode(null);
        assert!(node.claim(0).is_err());
        assert!(node.interrupt_out(0x02, &[0; 8]).is_err());
        assert!(node.disconnect(0).is_err());
    }
}
