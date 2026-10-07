//! Plain-word sentences for the failures the read bar and the write dialogs show.
//!
//! The core's `Display` text names offsets, chunks and internal errors, and the CLI
//! keeps it. The GUI says one sentence a person can act on. The raw text stays in the
//! log, where `main.rs` prints it.

use controller_core::error::ErrorCategory;
use controller_core::model::WriteResult;
use controller_core::Error;

/// The sentence for a failed read. The caller adds the title and the holder hint.
#[must_use]
pub const fn read_failure(error: &Error) -> &'static str {
    match error {
        Error::NoDevice => "No supported controller is connected.",
        Error::UnsupportedModel(_) => "8B does not support this controller model.",
        Error::PermissionDenied(_) => "8B is not allowed to open the controller.",
        Error::Usb(_) => "The USB connection to the controller failed.",
        Error::Timeout => "The controller did not answer in time.",
        Error::Disconnected => "The controller disconnected, or its mode switch was moved.",
        Error::Decode(_) => "The controller sent something 8B could not understand.",
        Error::Validation(_) => "The data from the controller failed a check.",
        Error::Io(_) => "8B could not read or write a file it needs.",
        Error::Write { .. } => "The controller did not accept the data.",
    }
}

/// The sentence for a failed write, from how far it got. The caller adds the lead for a
/// lost connection and the holder hint; the backup path has its own line in the dialog.
#[must_use]
pub const fn write_failure(result: &WriteResult) -> &'static str {
    if result.rollback_succeeded {
        return "The controller did not accept the profile. The slot is back as it was.";
    }
    if result.rollback_attempted {
        return if result.backup_file_path.is_some() {
            "The controller did not accept the profile, and the slot could not be put back \
             as it was. A copy of the old profile is saved."
        } else {
            "The controller did not accept the profile, the slot could not be put back as it \
             was, and 8B could not save a copy of the old profile."
        };
    }
    match result.error_category {
        ErrorCategory::Timeout | ErrorCategory::ConnectionFailure => {
            "Check the cable and that the mode switch has not moved."
        }
        ErrorCategory::ValidationFailure => {
            "These edits do not make a valid profile, so nothing was written."
        }
        ErrorCategory::None | ErrorCategory::ExportFailure => "Nothing was written.",
        ErrorCategory::WriteFailure => "The controller did not accept the profile.",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use controller_core::model::{Mode, Slot};

    use super::*;

    /// Text the sentences must never carry: hex, and the core's own wording.
    const LEAKS: [&str; 6] = ["0x", "echo", "offset", "chunk", "Malformed", "os error"];

    fn clean(sentence: &str) {
        for leak in LEAKS {
            assert!(!sentence.contains(leak), "{sentence:?} leaks {leak:?}");
        }
        assert!(sentence.ends_with('.'), "{sentence:?} is not a sentence");
    }

    #[test]
    fn every_read_error_is_plain() {
        let payload = || "upload echo offset mismatch: expected 0x002D, got 0x0000".to_owned();
        let errors = [
            Error::NoDevice,
            Error::UnsupportedModel(0x2D),
            Error::PermissionDenied("/dev/hidraw3".to_owned()),
            Error::Usb(payload()),
            Error::Timeout,
            Error::Disconnected,
            Error::Decode(payload()),
            Error::Validation(payload()),
            Error::Io(payload()),
            Error::Write { message: payload(), failed_chunk: Some(11), total_chunks: 53 },
        ];
        for error in errors {
            let sentence = read_failure(&error);
            clean(sentence);
            assert!(!sentence.contains("/dev/"), "{sentence:?} names a device node");
        }
    }

    fn failed(category: ErrorCategory) -> WriteResult {
        let slot = Slot::new(1).unwrap();
        WriteResult::failure(Mode::XInput, slot, category, "Write failed at chunk 12/53.")
    }

    #[test]
    fn every_write_failure_is_plain() {
        let mut cases: Vec<_> = [
            ErrorCategory::None,
            ErrorCategory::ConnectionFailure,
            ErrorCategory::Timeout,
            ErrorCategory::ExportFailure,
            ErrorCategory::ValidationFailure,
            ErrorCategory::WriteFailure,
        ]
        .map(failed)
        .into();
        let mut restored = failed(ErrorCategory::WriteFailure);
        restored.rollback_attempted = true;
        restored.rollback_succeeded = true;
        let mut stuck = failed(ErrorCategory::WriteFailure);
        stuck.rollback_attempted = true;
        let mut saved = stuck.clone();
        saved.backup_file_path = Some("/home/x/b.bin".to_owned());
        cases.extend([restored, stuck, saved]);
        for case in &cases {
            let sentence = write_failure(case);
            clean(sentence);
            assert!(!sentence.contains("/home"), "{sentence:?} names a path");
        }
    }
}
