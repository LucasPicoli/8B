//! Hardware integration tests — require a physical 8BitDo Pro 3 (`XInput`, pid `310b`).
//!
//! Gated behind `--features hardware` and marked `#[ignore]` so they never run
//! in CI without an attached device. Run with:
//!   `cargo test -p controller-core --features hardware --test hardware_read -- --ignored`
#![cfg(feature = "hardware")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use controller_core::model::Mode;
use controller_core::transport::{DeviceIo, NusbDevice};
use serial_test::serial;

#[test]
#[ignore = "requires attached 8BitDo Pro 3 (XInput, pid 310b)"]
#[serial]
fn reads_and_maps_a_real_profile() {
    let dev = NusbDevice::open().unwrap();
    let r = dev.read_all_profiles(Mode::XInput).unwrap();
    assert_eq!(r.raw_blobs.len(), 2); // xinput + switch
    assert!(r.raw_blobs.iter().all(|b| b.len() == 0x092C));
    assert!(!r.profiles.is_empty());
    assert!(r.profiles.iter().any(|p| !p.name.is_empty())); // at least one active mapped profile
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3"]
#[serial]
fn detects_connected_device() {
    let dev = NusbDevice::open().unwrap();
    let rd = dev.detect_readiness().unwrap();
    assert!(rd.supported_device_connected);
    assert_eq!(rd.product_id, "310b");
    assert_eq!(rd.mode, Some(Mode::XInput));
}
