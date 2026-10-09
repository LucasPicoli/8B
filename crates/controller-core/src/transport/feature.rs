//! HID feature reports on a hidraw node: the `HIDIOCGFEATURE` and `HIDIOCSFEATURE`
//! ioctls of `linux/hidraw.h`.
//!
//! The 8BitDo protocol uses output and input reports and never calls these. They are
//! here for a model whose own transport talks through feature reports.
//!
//! The workspace denies `unsafe`. This module allows it for the one ioctl call, whose
//! contract the `Ioctl` impl below states.
#![allow(unsafe_code)]

use std::ffi::c_void;
use std::os::fd::AsFd;

use rustix::ioctl::{ioctl, opcode, Direction, Ioctl, IoctlOutput, Opcode};

use crate::error::{Error, Result};

/// The ioctl group of hidraw, `'H'`.
const HIDRAW_GROUP: u8 = b'H';
/// The ioctl number of `HIDIOCSFEATURE`.
const SET_FEATURE: u8 = 0x06;
/// The ioctl number of `HIDIOCGFEATURE`.
const GET_FEATURE: u8 = 0x07;
/// The largest report the ioctl size field holds: it has 14 bits.
pub const MAX_FEATURE_LEN: usize = (1 << 14) - 1;

/// One feature-report ioctl over `buf`, whose first byte is the report id.
struct Feature<'a> {
    /// [`SET_FEATURE`] or [`GET_FEATURE`].
    number: u8,
    /// The report, the id first. The kernel reads it, and for a get writes into it.
    buf: &'a mut [u8],
}

// SAFETY: `opcode` encodes `buf.len()` as the size, as `HIDIOCSFEATURE(len)` and
// `HIDIOCGFEATURE(len)` do in `linux/hidraw.h`, so the kernel reads and writes at most
// `buf.len()` bytes at `as_ptr`, which points at the start of `buf` for the whole call.
// `IS_MUTATING` is true because a get writes into `buf`. `output_from_ptr` reads only
// the return value, never the pointer.
unsafe impl Ioctl for Feature<'_> {
    type Output = usize;
    const IS_MUTATING: bool = true;

    fn opcode(&self) -> Opcode {
        opcode::from_components(Direction::ReadWrite, HIDRAW_GROUP, self.number, self.buf.len())
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.buf.as_mut_ptr().cast()
    }

    unsafe fn output_from_ptr(out: IoctlOutput, _: *mut c_void) -> rustix::io::Result<usize> {
        usize::try_from(out).map_err(|_| rustix::io::Errno::INVAL)
    }
}

/// Runs one feature ioctl on `fd`.
fn call(fd: impl AsFd, number: u8, buf: &mut [u8]) -> Result<usize> {
    if buf.is_empty() || buf.len() > MAX_FEATURE_LEN {
        return Err(Error::Usb(format!(
            "a feature report is 1 to {MAX_FEATURE_LEN} bytes, got {}",
            buf.len()
        )));
    }
    // SAFETY: `Feature` keeps the `Ioctl` contract, as its impl states.
    unsafe { ioctl(fd, Feature { number, buf }) }
        .map_err(|e| Error::Usb(format!("feature report ioctl failed: {e}")))
}

/// Reads feature report `report_id` into `buf`, which sets the largest length to read.
/// The first byte of `buf` comes back as the report id. Returns the bytes the device
/// sent, the id included.
///
/// # Errors
/// Returns [`Error::Usb`] if `buf` is empty or longer than [`MAX_FEATURE_LEN`], or if
/// the ioctl fails, such as on a node that is not hidraw.
pub fn get_feature(fd: impl AsFd, report_id: u8, buf: &mut [u8]) -> Result<usize> {
    if let Some(first) = buf.first_mut() {
        *first = report_id;
    }
    call(fd, GET_FEATURE, buf)
}

/// Sends feature report `report`, whose first byte is the report id. Returns the bytes
/// the kernel sent.
///
/// # Errors
/// Returns [`Error::Usb`] if `report` is empty or longer than [`MAX_FEATURE_LEN`], or
/// if the ioctl fails.
pub fn set_feature(fd: impl AsFd, report: &[u8]) -> Result<usize> {
    call(fd, SET_FEATURE, &mut report.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_opcodes_match_linux_hidraw_h() {
        let mut buf = [0u8; 64];
        let get = Feature { number: GET_FEATURE, buf: &mut buf };
        // HIDIOCGFEATURE(64) and HIDIOCSFEATURE(64), as the C macros expand them.
        assert_eq!(u64::from(get.opcode()), 0xC040_4807);
        let set = Feature { number: SET_FEATURE, buf: &mut buf };
        assert_eq!(u64::from(set.opcode()), 0xC040_4806);
    }

    #[test]
    fn a_bad_length_or_a_node_that_is_not_hidraw_is_refused() {
        let null = std::fs::File::open("/dev/null");
        let Ok(null) = null else { return };
        assert!(get_feature(&null, 1, &mut []).is_err());
        assert!(set_feature(&null, &vec![0; MAX_FEATURE_LEN + 1]).is_err());
        assert!(get_feature(&null, 1, &mut [0; 8]).is_err(), "/dev/null takes no ioctl");
        assert!(set_feature(&null, &[1, 2, 3]).is_err());
    }
}
