//! Integration tests of the 8BitDo Pro 3: golden vectors against `fixtures/pro3/`, the
//! validators, the write orchestrators on a mock, and, behind the `hardware` feature,
//! tests on an attached Pro 3.
//!
//! Each model keeps its tests in `tests/<model>/`, entered from that folder's
//! `main.rs`, so one model's tests never mix with another's. `tests/support/` holds
//! what every model's tests share.
//!
//! Run one file's tests with its module name as the filter, such as
//! `cargo test -p controller-core --test pro3 golden_profile_compile`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub)]

mod fixture_macros;
mod fixture_profiles;
mod golden_macro_decode;
mod golden_macro_encode;
mod golden_profile_compile;
mod golden_profile_decode;
mod validation_macro;
mod validation_profile;
mod write_batch;
mod write_orchestrator;

#[cfg(feature = "hardware")]
#[path = "../support/hw.rs"]
mod hw;
#[cfg(feature = "hardware")]
mod seed;

#[cfg(feature = "hardware")]
mod hardware_batch;
#[cfg(feature = "hardware")]
mod hardware_macro;
#[cfg(feature = "hardware")]
mod hardware_read;
#[cfg(feature = "hardware")]
mod hardware_write;
