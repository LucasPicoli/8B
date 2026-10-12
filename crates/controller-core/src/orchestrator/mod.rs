//! High-level orchestrators that compose device detection, reading, and export
//! into reusable pipelines.

pub mod batch;
pub mod macros;
mod pages;
pub mod patch;
pub mod profile;
pub mod write;

pub use batch::{MacroEdit, WriteJob, WriteOp};

pub use write::ProfileWriteOrchestrator;
