//! Integration tests of the 8BitDo Ultimate: golden vectors against `fixtures/ultimate/`,
//! its description against the codec, and the write orchestrator on a mock.
//!
//! Run one file's tests with its module name as the filter, such as
//! `cargo test -p controller-core --test ultimate golden_codec`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub, clippy::unwrap_used)]

mod description;
mod golden_codec;
mod golden_packets;
mod import_fixtures;
mod second_model;
mod wired;
mod write_orchestrator;

#[cfg(feature = "hardware")]
mod hardware_lock;
#[cfg(feature = "hardware")]
#[path = "../support/hw.rs"]
mod hw;
#[cfg(feature = "hardware")]
mod import_check;

use controller_core::description::ControllerDescription;
use controller_core::device::Model;

/// The registered Ultimate.
pub(crate) fn ultimate() -> &'static dyn Model {
    controller_core::devices::model_named("Ultimate (BT)").unwrap()
}

/// Its description.
pub(crate) fn d() -> &'static ControllerDescription {
    ultimate().description().unwrap()
}
