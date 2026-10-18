//! Integration tests of the 8BitDo Ultimate: golden vectors against `fixtures/ultimate/`,
//! its description against the codec, and the write orchestrator on a mock.
//!
//! Run one file's tests with its module name as the filter, such as
//! `cargo test -p controller-core --test ultimate golden_codec`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub)]

mod description;
mod golden_codec;
mod golden_packets;
mod write_orchestrator;

#[cfg(feature = "hardware")]
mod hardware_lock;
