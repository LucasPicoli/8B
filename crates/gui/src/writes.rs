//! The writes the worker runs: a profile upload, or a slot clear, through the write
//! orchestrator. Each job is one write session, so a write from the Switch position
//! flips the controller to `DInput` and back inside it.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use controller_core::devices::pro3::Pro3;
use controller_core::model::{Mode, Slot, WriteResult};
use controller_core::orchestrator::ProfileWriteOrchestrator;
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;
use serde_json::Value;

/// What a write does to its slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOp {
    /// Put this profile in the slot. The slot keeps the macros it has, except those
    /// the buttons in `drop_macros` start.
    Upload {
        /// The profile, as the profile schema holds it, with no `macro_refs`.
        profile: Value,
        /// The trigger of each macro to remove.
        drop_macros: Vec<String>,
    },
    /// Empty the slot. Its macros stay stored.
    Clear,
}

/// One write to one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteJob {
    /// The mode of the slot.
    pub mode: Mode,
    /// The slot.
    pub slot: Slot,
    /// What to do to it.
    pub op: WriteOp,
}

/// Where a failed rollback saves the old profile: `$XDG_STATE_HOME/8b/backups`, else
/// `~/.local/state/8b/backups`. An empty variable counts as unset, as the XDG spec
/// says.
#[must_use]
pub fn backup_dir(xdg_state_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let base = xdg_state_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("8b/backups")
}

/// Runs `job` against `dev`. Overwriting is forced: the window has asked already.
#[must_use]
pub fn run(dev: &dyn DeviceIo, job: &WriteJob, backup_dir: &Path) -> WriteResult {
    // The orchestrator reports a backup file it cannot create; a missing folder is
    // the common cause, so make it first.
    let _ = std::fs::create_dir_all(backup_dir);
    let writer = ProfileWriteOrchestrator::new(dev, &Pro3, backup_dir);
    match &job.op {
        WriteOp::Upload { profile, drop_macros } => writer.upload_profile_dropping_macros(
            profile,
            job.mode,
            job.slot,
            drop_macros,
            &ConfirmPolicy::Force,
        ),
        WriteOp::Clear => writer.deactivate_slot(job.mode, job.slot, &ConfirmPolicy::Force),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn the_backup_folder_follows_xdg_state_home_then_home() {
        let os = |s: &str| Some(OsString::from(s));
        assert_eq!(backup_dir(os("/s"), os("/h")), Path::new("/s/8b/backups"));
        assert_eq!(backup_dir(os(""), os("/h")), Path::new("/h/.local/state/8b/backups"));
        assert_eq!(backup_dir(None, os("/h")), Path::new("/h/.local/state/8b/backups"));
    }
}
