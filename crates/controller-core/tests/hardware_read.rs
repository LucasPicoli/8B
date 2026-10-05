//! Hardware integration tests: require a physical 8BitDo Pro 3 on USB, in any
//! current mode (`XInput`, Switch or `DInput`).
//!
//! Gated behind `--features hardware` and marked `#[ignore]` so they never run
//! in CI without an attached device. Run with:
//!   `cargo test -p controller-core --features hardware --test hardware_read -- --ignored`
#![cfg(feature = "hardware")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use controller_core::model::Mode;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn reads_every_bank_from_the_current_mode() {
    let dev = HidrawDevice::open().unwrap();
    let read = dev.read_all_profiles().unwrap();
    assert_eq!(read.raw_blobs.len(), Mode::ALL.len());
    assert!(read.raw_blobs.iter().all(|b| b.len() == 0x092C));
    let modes: Vec<Mode> = read.profiles.iter().map(|p| p.mode).collect();
    assert_eq!(modes, Mode::ALL.iter().flat_map(|&m| [m; 3]).collect::<Vec<_>>());
    assert!(read.profiles.iter().any(|p| !p.name.is_empty())); // at least one active mapped profile
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn detects_connected_device() {
    let dev = HidrawDevice::open().unwrap();
    let rd = dev.detect_readiness().unwrap();
    assert!(rd.supported_device_connected);
    assert!(rd.mode.is_some());
    assert!(rd.active_slot_marker_verified, "{}", rd.message);
}
