//! Tests of the 8BitDo Ultimate 2: the files of its guided hardware check against the
//! codec, and, with the `hardware` feature, the guided check itself.
//!
//! Run the offline part with `cargo test -p controller-core --test ultimate2`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub, clippy::unwrap_used, clippy::indexing_slicing)]

// The presses and the write payload serve only the hardware check.
#[cfg_attr(not(feature = "hardware"), allow(dead_code))]
mod fixture;

#[cfg(feature = "hardware")]
mod guided;
#[cfg(feature = "hardware")]
#[path = "../support/hw.rs"]
mod hw;
#[cfg(feature = "hardware")]
mod input;
#[cfg(feature = "hardware")]
mod trigger_probe;

use controller_core::devices::model_named;
use controller_core::model::{RawProfilePayload, Slot};
use fixture::{differences, fixture, SLOT};

/// Each check file, written into a bank of its mode by the codec, reads back as itself:
/// the comparison the hardware check runs passes on what the codec makes.
#[test]
fn each_check_file_round_trips_through_the_codec() {
    let model = model_named("Ultimate 2").unwrap();
    let xinput = std::fs::read("../../fixtures/ultimate-2/xinput.blob").unwrap();
    // A DInput bank the pad never saved: erased flash with the mode byte.
    let mut dinput = vec![0xff; model.blob_size()];
    dinput[0x10] = 1;
    dinput[0x11] = 0;
    for (mode, base) in [("xinput", xinput), ("dinput", dinput)] {
        let mode = model
            .description()
            .unwrap()
            .modes
            .iter()
            .find(|m| m.id.to_string() == mode)
            .unwrap()
            .id;
        let (profile, macros) = fixture(mode);
        let slot = Slot::new(SLOT).unwrap();
        let bank = model.compile_profile(&profile, slot, &base, &macros).unwrap();
        let raw = RawProfilePayload {
            payload: bank.clone(),
            source_slot: SLOT,
            source_profile_index: SLOT - 1,
            mode_hint: mode,
        };
        let read = model.map_profile(&raw).unwrap().canonical;
        // The steps live on their own flash pages: encode and decode each macro's.
        let mut read_macros = model.decode_macro_metadata(&bank, slot).unwrap();
        for (got, want) in read_macros.iter_mut().zip(&macros) {
            let stream = model.encode_macro_steps(&want.steps, mode).unwrap();
            got.steps = model.decode_macro_steps(&stream, want.steps.len(), mode).unwrap();
        }
        assert_eq!(differences(&read, &read_macros), Vec::<String>::new(), "{mode}");
    }
}
