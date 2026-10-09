//! Hardware write test: requires a physical 8BitDo Pro 3 on USB in `DInput` mode
//! (`2dc8:6009`). It remaps l4 on `DInput` slot 3, reads the bank back, then puts l4
//! back. Run with:
//!   `cargo test -p controller-core --features hardware --test hardware_write -- --ignored`
#![cfg(feature = "hardware")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use controller_core::devices::pro3::{Pro3, DINPUT};
use controller_core::model::Slot;
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::transport::{DeviceIo, HidrawDevice};
use serial_test::serial;

/// Button entry 20 (l4) of slot 3.
const L4_SLOT3: usize = 0x00E4 + 2 * 0x5C + 20 * 4;
/// The struct CRC, which the firmware does not check.
const CRC: std::ops::Range<usize> = 0x0C..0x10;

fn dinput_bank(dev: &HidrawDevice) -> Vec<u8> {
    dev.read_all_profiles().unwrap().raw_blobs.remove(2)
}

/// Offsets where `a` and `b` differ, outside the CRC.
fn changed(a: &[u8], b: &[u8]) -> Vec<usize> {
    (0..a.len()).filter(|&i| a[i] != b[i] && !CRC.contains(&i)).collect()
}

#[test]
#[ignore = "requires attached 8BitDo Pro 3 in DInput mode"]
#[serial]
fn dinput_remap_lands_on_slot3_l4_and_reverts() {
    let dev = HidrawDevice::first();
    let dir = tempfile::tempdir().unwrap();
    let orch = ProfileWriteOrchestrator::new(&dev, &Pro3, dir.path());
    let slot = Slot::new(3).unwrap();
    let before = dinput_bank(&dev);
    assert_eq!(&before[L4_SLOT3..L4_SLOT3 + 4], &[0; 4], "l4 on slot 3 must start unassigned");

    let r = orch.remap_button(DINPUT, slot, "l4", "bottom face");
    assert!(r.success, "{}", r.message);
    let remapped = dinput_bank(&dev);
    let diff = changed(&before, &remapped);
    println!("remap changed {diff:x?}");
    assert_eq!(&remapped[L4_SLOT3..L4_SLOT3 + 4], &[0x00, 0x20, 0x00, 0x00]);
    // Sticks and triggers must not drift: only the l4 entry may change.
    assert!(diff.iter().all(|i| (L4_SLOT3..L4_SLOT3 + 4).contains(i)), "{diff:x?}");

    let r = orch.remap_button(DINPUT, slot, "l4", "disabled");
    assert!(r.success, "{}", r.message);
    let reverted = dinput_bank(&dev);
    let left = changed(&before, &reverted);
    println!("revert left {left:x?}");
    assert!(left.is_empty(), "{left:x?}");
}
