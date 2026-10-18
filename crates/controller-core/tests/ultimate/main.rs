//! Integration tests of the 8BitDo Ultimate: golden vectors against `fixtures/ultimate/`.
//!
//! Run one file's tests with its module name as the filter, such as
//! `cargo test -p controller-core --test ultimate golden_codec`.

// The modules share helpers through the crate, which exports nothing.
#![allow(unreachable_pub)]

mod golden_codec;
mod golden_packets;
