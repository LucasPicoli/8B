//! High-level orchestrators that compose device detection, reading, and export
//! into reusable pipelines.
//!
//! Mirrors `src/core/profile_orchestrator.{h,cpp}` and
//! `src/core/profile_write_orchestrator.{h,cpp}` from the C++ reference.

pub mod patch;
pub mod profile;
pub mod write;

pub use patch::{StickPatch, TriggerPatch};
pub use write::{ProfileWriteOrchestrator, REFUSE_DINPUT_WRITES};
