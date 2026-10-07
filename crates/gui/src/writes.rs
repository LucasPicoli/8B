//! The writes the worker runs: profile uploads and slot clears, through the write
//! orchestrator. A command is one write session, however many slots it holds, so a
//! write from the Switch position flips the controller to `DInput` and back once.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use controller_core::devices::pro3::Pro3;
use controller_core::model::WriteResult;
use controller_core::orchestrator::ProfileWriteOrchestrator;
pub use controller_core::orchestrator::{WriteJob, WriteOp};
use controller_core::service::ConfirmPolicy;
use controller_core::transport::DeviceIo;

/// Where 8B keeps its state: `$XDG_STATE_HOME/8b`, else `~/.local/state/8b`. An empty
/// variable counts as unset, as the XDG spec says.
#[must_use]
pub fn state_dir(xdg_state_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    let base = xdg_state_home
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.filter(|v| !v.is_empty()).map(|h| Path::new(&h).join(".local/state")))
        .unwrap_or_else(|| PathBuf::from(".local/state"));
    base.join("8b")
}

/// Where a failed rollback saves the old profile: `backups` in the state dir.
#[must_use]
pub fn backup_dir(xdg_state_home: Option<OsString>, home: Option<OsString>) -> PathBuf {
    state_dir(xdg_state_home, home).join("backups")
}

/// Runs `jobs` against `dev` in one pass, one result per job. Overwriting is forced:
/// the window has asked already.
#[must_use]
pub fn run(dev: &dyn DeviceIo, jobs: &[WriteJob], backup_dir: &Path) -> Vec<WriteResult> {
    // The orchestrator reports a backup file it cannot create; a missing folder is
    // the common cause, so make it first.
    let _ = std::fs::create_dir_all(backup_dir);
    ProfileWriteOrchestrator::new(dev, &Pro3, backup_dir).write_slots(jobs, &ConfirmPolicy::Force)
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
