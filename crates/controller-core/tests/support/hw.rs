//! A hardware test's hold on a real pad, for any model.
//!
//! [`pad`] finds an attached pad of the model a test is for, and refuses any other: a
//! test compiles its model's blobs, and they must never reach another pad's flash.
//! [`seed`] fills the banks through the model's own fill function and returns a
//! [`Banks`] guard, which writes the banks back byte for byte when the test ends, pass
//! or fail. Only the profile banks: a test that writes other flash restores it itself.
// Each model's test binary uses part of this module, and none exports it.
#![allow(dead_code, unreachable_pub, clippy::unwrap_used, clippy::panic)]

use std::path::Path;

use controller_core::description::ControllerDescription;
use controller_core::detect::scan_sysfs_all;
use controller_core::device::Model;
use controller_core::devices::SYSFS_USB;
use controller_core::model::{Mode, ProfileReadResult};
use controller_core::transport::{DeviceIo, HidrawDevice};

/// The description of `model`.
fn description(model: &dyn Model) -> &'static ControllerDescription {
    model.description().unwrap()
}

/// The first attached pad that answers as `model`.
///
/// # Panics
/// When no attached pad is a `model`, before anything is written.
pub fn pad(model: &'static dyn Model) -> HidrawDevice {
    let wanted = description(model);
    let found = scan_sysfs_all(Path::new(SYSFS_USB), &wanted.config_ports);
    let mut seen = Vec::new();
    for usb in &found {
        let dev = HidrawDevice::at(usb.port_path());
        match dev.model() {
            Ok(m) if std::ptr::eq(description(m), wanted) => return dev,
            Ok(m) => seen.push(format!("{} on {}", description(m).short_name, usb.port_path())),
            Err(e) => seen.push(format!("{} ({e})", usb.port_path())),
        }
    }
    panic!("no {} is attached; found: {seen:?}", wanted.short_name);
}

/// The banks of a pad as a test found them and as it seeded them.
pub struct Banks<'a> {
    dev: &'a HidrawDevice,
    modes: Vec<Mode>,
    /// The banks before the seed. Dropping the guard writes them back.
    pub original: ProfileReadResult,
    /// The banks right after the seed.
    pub fixtures: ProfileReadResult,
}

/// Writes `blob` into the bank of `mode`, flipping the pad first if its mode asks.
fn write_bank(dev: &HidrawDevice, mode: Mode, blob: &[u8]) {
    let back = dev.begin_write().unwrap();
    let sent = (|| {
        dev.send_slot_select(mode)?;
        dev.write_full_profile(mode, blob)?;
        dev.send_apply(mode)
    })();
    if let Some(back) = back {
        dev.end_write(back).unwrap();
    }
    sent.unwrap();
}

/// Writes each bank of `target` that differs from what the pad holds now.
fn put(dev: &HidrawDevice, modes: &[Mode], target: &ProfileReadResult) {
    let now = dev.read_all_profiles().unwrap();
    for ((mode, held), want) in modes.iter().zip(&now.raw_blobs).zip(&target.raw_blobs) {
        if held != want {
            write_bank(dev, *mode, want);
        }
    }
}

/// Writes into each bank of `dev`, a pad of `model` from [`pad`], what `fill` makes of
/// it from the mode and the bank as read. The guard exists before the first write, so a
/// seed that fails halfway is undone too.
pub fn seed<'a>(
    dev: &'a HidrawDevice,
    model: &'static dyn Model,
    fill: impl Fn(Mode, &[u8]) -> Vec<u8>,
) -> Banks<'a> {
    let order: Vec<Mode> = description(model).modes.iter().map(|m| m.id).collect();
    let original = dev.read_all_profiles().unwrap();
    let mut banks = Banks { dev, modes: order, fixtures: original.clone(), original };
    for (mode, held) in banks.modes.iter().zip(&banks.original.raw_blobs) {
        write_bank(dev, *mode, &fill(*mode, held));
    }
    banks.fixtures = dev.read_all_profiles().unwrap();
    banks
}

impl Banks<'_> {
    /// Writes the seeded banks again, for a test that writes twice from one start.
    pub fn reseed(&self) {
        put(self.dev, &self.modes, &self.fixtures);
    }
}

impl Drop for Banks<'_> {
    fn drop(&mut self) {
        let failing = std::thread::panicking();
        let restore = std::panic::AssertUnwindSafe(|| {
            put(self.dev, &self.modes, &self.original);
            self.dev.read_all_profiles().unwrap().raw_blobs
        });
        match std::panic::catch_unwind(restore) {
            Ok(now) if now == self.original.raw_blobs => {}
            // A panic in a drop that runs for a panic aborts the run: only report it.
            _ if failing => eprintln!("the banks could not be put back; restore them from a dump"),
            Ok(_) => panic!("the banks differ from how the test found them"),
            Err(cause) => std::panic::resume_unwind(cause),
        }
    }
}
