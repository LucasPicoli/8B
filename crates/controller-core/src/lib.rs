//! Device-neutral engine for 8BitDo controller configuration.
//!
//! The only controller implemented today is the 8BitDo Pro 3 (`devices::pro3`).
//! Modules are added task by task; Task 1 ships only the crate version accessor.

pub mod error;
pub use error::{Error, ErrorCategory, Result};

pub mod protocol;

/// Returns the crate version string.
#[must_use]
pub const fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
