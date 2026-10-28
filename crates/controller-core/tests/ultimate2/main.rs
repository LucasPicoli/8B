//! Tests of the 8BitDo Ultimate 2: the files of its guided hardware check and of its GUI
//! check against the codec, and, with the `hardware` feature, both checks themselves.
//!
//! Run the offline part with `cargo test -p controller-core --test ultimate2`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub, clippy::unwrap_used, clippy::indexing_slicing)]

// The presses and the write payload serve only the hardware check.
#[cfg_attr(not(feature = "hardware"), allow(dead_code))]
mod fixture;

#[cfg(feature = "hardware")]
mod erase_probe;
#[cfg(feature = "hardware")]
mod gui_check;
#[cfg(feature = "hardware")]
mod guided;
#[cfg(feature = "hardware")]
#[path = "../support/hw.rs"]
mod hw;
#[cfg(feature = "hardware")]
mod input;
mod step_limit;
#[cfg(feature = "hardware")]
mod step_probe;
#[cfg(feature = "hardware")]
mod sweep_probe;
#[cfg(feature = "hardware")]
mod trigger_probe;

use controller_core::devices::model_named;
use controller_core::model::glide::{expand, fold};
use controller_core::model::{CanonicalProfile, MacroDefinition, Mode, RawProfilePayload, Slot};
use fixture::{differences_from, fixture, load, SLOT};

/// The slots and files of the GUI check: each mode's slots 2 and 3.
pub const GUI_CHECK: [(&str, u8); 4] = [("xinput", 2), ("xinput", 3), ("dinput", 2), ("dinput", 3)];

/// The GUI check's file of `mode`'s slot `slot`.
#[must_use]
pub fn gui_check_file(mode: &str, slot: u8) -> String {
    format!("../../fixtures/ultimate-2/gui-check/{mode}-slot{slot}.json")
}

/// The mode named `name`.
fn mode_named(name: &str) -> Mode {
    let model = model_named("Ultimate 2").unwrap();
    model.description().unwrap().modes.iter().find(|m| m.id.to_string() == name).unwrap().id
}

/// Every way `profile` and `macros` differ from themselves once the codec has written
/// them into slot `slot` of a bank of their mode and read them back.
fn round_trip(profile: &CanonicalProfile, macros: &[MacroDefinition], slot: u8) -> Vec<String> {
    let model = model_named("Ultimate 2").unwrap();
    let mode = profile.mode;
    let base = if mode.to_string() == "xinput" {
        std::fs::read("../../fixtures/ultimate-2/xinput.blob").unwrap()
    } else {
        // A DInput bank the pad never saved: erased flash with the mode byte.
        let mut dinput = vec![0xff; model.blob_size()];
        dinput[0x10] = 1;
        dinput[0x11] = 0;
        dinput
    };
    let at = Slot::new(slot).unwrap();
    let bank = model.compile_profile(profile, at, &base, macros).unwrap();
    let raw = RawProfilePayload {
        payload: bank.clone(),
        source_slot: slot,
        source_profile_index: slot - 1,
        mode_hint: mode,
    };
    let read = model.map_profile(&raw).unwrap().canonical;
    // The steps live on their own flash pages: encode and decode each macro's, with its
    // glides as short steps on the pad, as a write and a read of the app make them.
    let glide_ms = model.description().unwrap().limits.glide_step_ms;
    let mut read_macros = model.decode_macro_metadata(&bank, at).unwrap();
    for (got, want) in read_macros.iter_mut().zip(macros) {
        let on_pad = expand(want, glide_ms);
        let stream = model.encode_macro_steps(&on_pad.steps, mode).unwrap();
        got.steps = model.decode_macro_steps(&stream, on_pad.steps.len(), mode).unwrap();
        got.steps = fold(got, glide_ms).steps;
    }
    differences_from(profile, macros, &read, &read_macros)
}

/// Each check file, written into a bank of its mode by the codec, reads back as itself:
/// the comparison the hardware check runs passes on what the codec makes.
#[test]
fn each_check_file_round_trips_through_the_codec() {
    for mode in ["xinput", "dinput"] {
        let (profile, macros) = fixture(mode_named(mode));
        assert_eq!(round_trip(&profile, &macros, SLOT), Vec::<String>::new(), "{mode}");
    }
}

/// Each file of the GUI check is one the codec writes and reads back unchanged, so a
/// difference the GUI check finds comes from what the window wrote, not from the file.
#[test]
fn each_gui_check_file_round_trips_through_the_codec() {
    for (mode, slot) in GUI_CHECK {
        let (profile, macros) = load(&gui_check_file(mode, slot));
        assert_eq!(profile.mode, mode_named(mode), "{mode} slot {slot}");
        assert_eq!(round_trip(&profile, &macros, slot), Vec::<String>::new(), "{mode} slot {slot}");
    }
}
