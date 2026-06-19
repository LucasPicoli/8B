//! High-level read/write services that orchestrate device I/O and codec calls.
//!
//! Each submodule exposes thin, testable functions that accept a [`crate::transport::device_io::DeviceIo`]
//! trait object, keeping hardware access behind an interface seam.

pub mod read;
