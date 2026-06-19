//! Device-neutral engine for 8BitDo controller configuration.
//!
//! The only controller implemented today is the 8BitDo Pro 3 (`devices::pro3`).
//! See each module for its scope.

pub mod error;
pub use error::{Error, ErrorCategory, Result};

pub mod detect;
pub mod device;
pub mod devices;
pub mod model;
pub mod protocol;
pub mod transport;

/// Returns the crate version string.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
