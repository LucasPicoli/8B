//! Error types and the stable exit-code contract.

/// Classification of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// No error.
    None,
    /// No supported device found or USB connection failed.
    ConnectionFailure,
    /// Device communication timed out or disconnected mid-transfer.
    Timeout,
    /// One or more profiles could not be written to disk.
    ExportFailure,
    /// Schema or semantic validation failed.
    ValidationFailure,
    /// Profile/macro write to the device failed.
    WriteFailure,
}

impl ErrorCategory {
    /// Returns the stable process exit code (0–6) for this category.
    #[must_use]
    pub const fn exit_code(self) -> i32 {
        match self {
            Self::None => 0,
            Self::ConnectionFailure => 1,
            Self::Timeout => 3,
            Self::ExportFailure => 5,
            Self::ValidationFailure => 4,
            Self::WriteFailure => 6,
        }
    }

    /// Returns the stable machine-readable label (e.g. `"write_failure"`).
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::ConnectionFailure => "connection_failure",
            Self::Timeout => "timeout",
            Self::ExportFailure => "export_failure",
            Self::ValidationFailure => "validation_failure",
            Self::WriteFailure => "write_failure",
        }
    }
}

/// The crate-wide error type.
#[derive(Debug, Clone, thiserror::Error)]
pub enum Error {
    /// No supported device is connected.
    #[error("no supported device connected")]
    NoDevice,
    /// An 8BitDo pad answered `START_CONFIG` with a model id this build does not
    /// support. No write is sent to it.
    #[error("unsupported controller model 0x{0:04x}")]
    UnsupportedModel(u16),
    /// The config hidraw node exists but this user may not open it. The message
    /// carries the command that installs the udev rule.
    #[error(
        "permission denied on {0}; grant access with: {cmd}",
        cmd = crate::transport::udev::manual_command()
    )]
    PermissionDenied(String),
    /// A USB-level failure occurred.
    #[error("usb error: {0}")]
    Usb(String),
    /// A transfer timed out or the device disconnected mid-transfer.
    #[error("device communication timed out")]
    Timeout,
    /// The device went away mid-session (`EIO` or `ENODEV`), for example after the
    /// mode switch was moved.
    #[error("device disconnected")]
    Disconnected,
    /// Device data was malformed or did not match the expected layout.
    #[error("malformed device data: {0}")]
    Decode(String),
    /// Schema or semantic validation failed.
    #[error("validation failed: {0}")]
    Validation(String),
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(String),
    /// A write to the device failed or was rejected.
    #[error("write failed: {message}")]
    Write {
        /// What went wrong.
        message: String,
        /// 0-based index of the chunk that failed, when the failure is tied to one.
        failed_chunk: Option<usize>,
        /// Total chunks in the failed write (0 when not chunked).
        total_chunks: usize,
    },
}

impl Error {
    /// Builds a [`Error::Write`] that is not tied to a chunk.
    #[must_use]
    pub fn write(message: impl Into<String>) -> Self {
        Self::Write { message: message.into(), failed_chunk: None, total_chunks: 0 }
    }

    /// Maps this error to its [`ErrorCategory`] for exit-code selection.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::NoDevice
            | Self::UnsupportedModel(_)
            | Self::PermissionDenied(_)
            | Self::Usb(_) => ErrorCategory::ConnectionFailure,
            Self::Timeout | Self::Disconnected => ErrorCategory::Timeout,
            Self::Decode(_) | Self::Validation(_) => ErrorCategory::ValidationFailure,
            Self::Io(_) => ErrorCategory::ExportFailure,
            Self::Write { .. } => ErrorCategory::WriteFailure,
        }
    }
}

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::unwrap_used)]
    #[test]
    fn exit_codes_and_labels_match_contract() {
        assert_eq!(ErrorCategory::None.exit_code(), 0);
        assert_eq!(ErrorCategory::ConnectionFailure.exit_code(), 1);
        assert_eq!(ErrorCategory::Timeout.exit_code(), 3);
        assert_eq!(ErrorCategory::ValidationFailure.exit_code(), 4);
        assert_eq!(ErrorCategory::ExportFailure.exit_code(), 5);
        assert_eq!(ErrorCategory::WriteFailure.exit_code(), 6);
        assert_eq!(ErrorCategory::WriteFailure.label(), "write_failure");
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn error_maps_to_category() {
        assert_eq!(Error::NoDevice.category(), ErrorCategory::ConnectionFailure);
        assert_eq!(Error::UnsupportedModel(0x6012).category(), ErrorCategory::ConnectionFailure);
        assert_eq!(
            Error::PermissionDenied("/dev/hidraw3".into()).category(),
            ErrorCategory::ConnectionFailure
        );
        assert_eq!(Error::Timeout.category(), ErrorCategory::Timeout);
        assert_eq!(Error::Disconnected.category(), ErrorCategory::Timeout);
        assert_eq!(Error::Decode("x".into()).category(), ErrorCategory::ValidationFailure);
        assert_eq!(Error::write("x").category(), ErrorCategory::WriteFailure);
    }
}
