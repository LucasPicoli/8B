//! The Pro 3's hardware seed: an attached Pro 3, and its banks filled with the fixture
//! profiles through the guard of `support/hw.rs`, which puts them back after the test.
//!
//! Every slot of every bank gets a profile: `XInput` and Switch slots 1 to 3 from
//! `fixtures/pro3/remap/`, and `DInput` slots 1 to 3 converted from the `XInput` ones.
//! l4 on `DInput` slot 3 is `disabled`, where the remap test starts.
//! A test can then assume occupied slots on any Pro 3, even one with every slot empty.
#![allow(clippy::unwrap_used)]

use controller_core::convert::convert_profile;
use controller_core::device::{ControllerSpec, ProtocolCodec};
use controller_core::devices::pro3::{Pro3, DINPUT, XINPUT};
use controller_core::model::profile::canonical_id;
use controller_core::model::{CanonicalProfile, Mode, Slot};
use controller_core::transport::HidrawDevice;

use crate::hw::{self, Banks};

/// Where the fixture profiles live, from the crate root.
const REMAP: &str = "../../fixtures/pro3/remap";

/// The fixture profile for `slot` of `mode`.
fn fixture(mode: Mode, slot: u8) -> CanonicalProfile {
    let file = |m: Mode| format!("{REMAP}/{m}-slot{slot}.profile.json");
    let read = |m: Mode| -> CanonicalProfile {
        serde_json::from_slice(&std::fs::read(file(m)).unwrap()).unwrap()
    };
    if mode != DINPUT {
        return read(mode);
    }
    let description = Pro3.description().unwrap();
    let xinput = read(XINPUT);
    let (mut profile, losses) = convert_profile(
        description,
        &xinput,
        &Pro3.default_profile(XINPUT),
        &Pro3.default_profile(DINPUT),
    )
    .unwrap();
    assert_eq!(losses, [], "XInput converts to DInput unchanged");
    profile.id = canonical_id(DINPUT, slot, slot - 1);
    profile.name = format!("RemapD{slot}");
    // XInput's l4 default is `disabled`, so the conversion gives DInput's default, its
    // own output. The remap test needs it unassigned.
    if slot == 3 {
        let l4 = profile.button_mappings.iter_mut().find(|m| m.source == "l4").unwrap();
        "disabled".clone_into(&mut l4.target);
    }
    profile
}

/// The first attached Pro 3. Panics before any write when none is attached.
pub fn pad() -> HidrawDevice {
    hw::pad(&Pro3)
}

/// Fills every slot of every bank of `dev`, a Pro 3 from [`pad`], with its fixture
/// profile.
pub fn seed(dev: &HidrawDevice) -> Banks<'_> {
    let banks = hw::seed(dev, &Pro3, |mode, held| {
        let mut blob = held.to_vec();
        for n in 1..=3 {
            let slot = Slot::new(n).unwrap();
            blob = Pro3.compile_profile(&fixture(mode, n), slot, &blob, &[]).unwrap();
        }
        blob
    });
    let full = banks.fixtures.profiles.iter().all(|p| !p.id.is_empty());
    assert!(full, "every slot holds a profile");
    banks
}
