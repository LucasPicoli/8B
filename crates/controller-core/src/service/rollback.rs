//! Rollback after a failed profile write, and the on-disk backup of last resort.
//!
//! Ports `WriteRollbackService`. When a write fails and the slot held a profile, the
//! backup blob from [`super::readback`] is written back and applied. If that fails too,
//! the blob is saved to a file so the user can restore it by hand.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::{Error, ErrorCategory, Result};
use crate::model::{Mode, Slot, WriteResult};
use crate::transport::device_io::DeviceIo;

/// Where a failed write stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FailedWrite {
    /// 0-based index of the chunk that failed, if known.
    pub chunk: Option<usize>,
    /// Total chunks in the failed write.
    pub total_chunks: usize,
}

impl FailedWrite {
    /// Reads the failure position out of a write error. Other errors give "unknown".
    #[must_use]
    pub const fn from_error(err: &Error) -> Self {
        match err {
            Error::Write { failed_chunk, total_chunks, .. } => {
                Self { chunk: *failed_chunk, total_chunks: *total_chunks }
            }
            _ => Self { chunk: None, total_chunks: 0 },
        }
    }

    fn prefix(self) -> String {
        match self.chunk {
            Some(i) if self.total_chunks > 0 => {
                format!("Write failed at chunk {}/{}.", i + 1, self.total_chunks)
            }
            _ => "Write failed.".to_owned(),
        }
    }
}

/// Tries to restore `backup` after a failed write to `slot` in `mode`.
///
/// - Slot was empty (or no backup): no rollback, the message says so.
/// - Otherwise writes `backup` back and sends apply. If either fails, saves the backup
///   under `backup_dir` and reports the path in `backup_file_path`.
///
/// The result always has `success == false` and category `WriteFailure`.
pub fn attempt_rollback(
    dev: &dyn DeviceIo,
    mode: Mode,
    slot: Slot,
    backup: &[u8],
    slot_was_active: bool,
    failed: FailedWrite,
    backup_dir: &Path,
) -> WriteResult {
    let prefix = failed.prefix();
    let mut result = WriteResult::failure(mode, slot, ErrorCategory::WriteFailure, prefix.clone());

    if !slot_was_active || backup.is_empty() {
        result.message = format!("{prefix} Slot was previously empty; no rollback needed.");
        return result;
    }
    result.rollback_attempted = true;

    let stage = if dev.write_full_profile(mode, backup).is_err() {
        "Rollback failed."
    } else if dev.send_apply(mode).is_err() {
        "Rollback write succeeded but APPLY failed."
    } else {
        result.rollback_succeeded = true;
        result.message = format!("{prefix} Rollback succeeded. Slot restored to previous state.");
        return result;
    };

    result.message = match save_backup(mode, slot, backup, backup_dir) {
        Ok(path) => {
            let shown = path.to_string_lossy().into_owned();
            let msg = format!(
                "{prefix} {stage} Original profile saved to {shown}. Manual recovery required."
            );
            result.backup_file_path = Some(shown);
            msg
        }
        Err(_) => {
            format!("{prefix} {stage} Could not save backup to disk. Manual recovery required.")
        }
    };
    result
}

/// Saves `blob` as `backup-<mode>-slot-<N>-<YYYYMMDD-HHMMSS>.bin` (UTC) in `dir`.
///
/// Never overwrites an existing file.
///
/// # Errors
/// Returns [`Error::Validation`] if `blob` is empty, or [`Error::Io`] if the file
/// cannot be created or written (including a name collision). The readback that made
/// the backup already checked its size.
pub fn save_backup(mode: Mode, slot: Slot, blob: &[u8], dir: &Path) -> Result<PathBuf> {
    if blob.is_empty() {
        return Err(Error::Validation("backup blob is empty".to_owned()));
    }
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let name = format!("backup-{mode}-slot-{}-{}.bin", slot.get(), utc_stamp(secs));
    let path = dir.join(name);
    let io = |e: std::io::Error| Error::Io(format!("{}: {e}", path.display()));
    let mut file = OpenOptions::new().write(true).create_new(true).open(&path).map_err(io)?;
    file.write_all(blob).and_then(|()| file.sync_all()).map_err(io)?;
    Ok(path)
}

/// Formats Unix seconds as `YYYYMMDD-HHMMSS` in UTC (Hinnant's civil-from-days).
fn utc_stamp(unix_secs: u64) -> String {
    let (days, rem) = (unix_secs / 86_400, unix_secs % 86_400);
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3_600, rem % 3_600 / 60, rem % 60)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::transport::mock::{MockCall, MockDevice, MockOp};

    fn backup() -> Vec<u8> {
        let mut b = vec![0u8; 0x092C];
        b[0x14] = 0xAA;
        b[0x15] = 0xBB;
        b
    }

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    const FAILED: FailedWrite = FailedWrite { chunk: Some(29), total_chunks: 53 };

    fn rollback(dev: &MockDevice, active: bool, dir: &Path) -> WriteResult {
        attempt_rollback(dev, Mode::XInput, slot(2), &backup(), active, FAILED, dir)
    }

    #[test]
    fn empty_slot_skips_rollback() {
        let dev = MockDevice::new();
        let dir = tempfile::tempdir().unwrap();
        let r = rollback(&dev, false, dir.path());
        assert!(!r.success && !r.rollback_attempted && !r.rollback_succeeded);
        assert!(r.message.contains("chunk 30/53"));
        assert!(r.message.contains("no rollback needed"));
        assert_eq!(dev.calls(), []);
        let none = attempt_rollback(&dev, Mode::XInput, slot(2), &[], true, FAILED, dir.path());
        assert!(!none.rollback_attempted);
    }

    #[test]
    fn rollback_succeeds_and_writes_the_backup() {
        let dev = MockDevice::new();
        let dir = tempfile::tempdir().unwrap();
        let r = rollback(&dev, true, dir.path());
        assert!(r.rollback_attempted && r.rollback_succeeded && !r.success);
        assert_eq!(r.error_category, ErrorCategory::WriteFailure);
        assert!(r.message.contains("Rollback succeeded"));
        assert_eq!(
            dev.calls(),
            [
                MockCall::WriteFullProfile { mode: Mode::XInput, blob: backup() },
                MockCall::Apply(Mode::XInput)
            ]
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn rollback_write_failure_saves_backup() {
        let dev = MockDevice::new().fail_nth(MockOp::WriteFullProfile, 0, Error::write("x"));
        let dir = tempfile::tempdir().unwrap();
        let r = rollback(&dev, true, dir.path());
        assert!(r.rollback_attempted && !r.rollback_succeeded);
        let path = r.backup_file_path.clone().unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), backup());
        assert!(r.message.contains(&path) && r.message.contains("Manual recovery required"));
        assert!(!dev.calls().iter().any(|c| c.op() == MockOp::Apply));
    }

    #[test]
    fn rollback_apply_failure_saves_backup() {
        let dev = MockDevice::new().fail_nth(MockOp::Apply, 0, Error::Timeout);
        let dir = tempfile::tempdir().unwrap();
        let r = rollback(&dev, true, dir.path());
        assert!(!r.rollback_succeeded);
        assert!(r.backup_file_path.is_some());
        assert!(r.message.contains("APPLY failed"));
    }

    #[test]
    fn unwritable_backup_dir_is_reported() {
        let dev = MockDevice::new().fail_nth(MockOp::WriteFullProfile, 0, Error::write("x"));
        let r = rollback(&dev, true, Path::new("/nonexistent/dir/for/backup"));
        assert!(r.backup_file_path.is_none());
        assert!(r.message.contains("Could not save backup to disk"));
    }

    #[test]
    fn unknown_failed_chunk_still_rolls_back() {
        let dev = MockDevice::new();
        let dir = tempfile::tempdir().unwrap();
        let r = attempt_rollback(
            &dev,
            Mode::XInput,
            slot(1),
            &backup(),
            true,
            FailedWrite::default(),
            dir.path(),
        );
        assert!(r.rollback_succeeded);
        assert!(r.message.starts_with("Write failed. Rollback succeeded"));
    }

    #[test]
    fn result_carries_mode_and_slot() {
        let dev = MockDevice::new();
        let dir = tempfile::tempdir().unwrap();
        let r = attempt_rollback(&dev, Mode::DInput, slot(3), &backup(), true, FAILED, dir.path());
        assert_eq!((r.mode, r.slot), (Mode::DInput, 3));
    }

    #[test]
    fn from_error_reads_chunk_position() {
        let e = Error::Write { message: "m".into(), failed_chunk: Some(4), total_chunks: 53 };
        assert_eq!(FailedWrite::from_error(&e), FailedWrite { chunk: Some(4), total_chunks: 53 });
        assert_eq!(FailedWrite::from_error(&Error::Timeout), FailedWrite::default());
    }

    #[test]
    fn save_backup_name_content_and_rejections() {
        let dir = tempfile::tempdir().unwrap();
        let path = save_backup(Mode::Switch, slot(3), &backup(), dir.path()).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("backup-switch-slot-3-")
                && path.extension().is_some_and(|e| e == "bin")
        );
        assert_eq!(name.len(), "backup-switch-slot-3-YYYYMMDD-HHMMSS.bin".len());
        assert_eq!(std::fs::read(&path).unwrap(), backup());
        assert!(save_backup(Mode::Switch, slot(3), &[], dir.path()).is_err());
    }

    #[test]
    fn save_backup_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_backup(Mode::XInput, slot(1), &backup(), dir.path()).unwrap();
        std::fs::write(&first, b"keep me").unwrap();
        // Same second, same name: the second save must fail, not clobber.
        if let Ok(second) = save_backup(Mode::XInput, slot(1), &backup(), dir.path()) {
            assert_ne!(second, first); // the clock ticked over; still no overwrite
        }
        assert_eq!(std::fs::read(&first).unwrap(), b"keep me");
    }

    #[test]
    fn utc_stamp_known_values() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_000_000_000), "20010909-014640");
        assert_eq!(utc_stamp(1_709_164_799), "20240228-235959");
        assert_eq!(utc_stamp(1_709_164_800 + 86_400), "20240301-000000");
    }
}
