//! High-level read/write services that orchestrate device I/O and codec calls.
//!
//! Each submodule exposes thin, testable functions that accept a [`crate::transport::device_io::DeviceIo`]
//! trait object, keeping hardware access behind an interface seam.

pub mod read;
pub mod readback;
pub mod rollback;
pub mod validation;

pub use readback::{bank_of, confirm_slot, readback_and_confirm, ConfirmPolicy, ReadbackResult};
pub use rollback::{attempt_rollback, save_backup, FailedWrite};
