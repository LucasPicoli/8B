//! Error types and the stable exit-code contract.

/// Classification of an error, mapped 1:1 from the C++ `core::ErrorCategory`.
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
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No supported device is connected.
    #[error("no supported device connected")]
    NoDevice,
    /// A USB-level failure occurred.
    #[error("usb error: {0}")]
    Usb(String),
    /// A transfer timed out or the device disconnected mid-transfer.
    #[error("device communication timed out")]
    Timeout,
    /// Device data was malformed or did not match the expected layout.
    #[error("malformed device data: {0}")]
    Decode(String),
    /// Schema or semantic validation failed.
    #[error("validation failed: {0}")]
    Validation(String),
    /// A filesystem operation failed.
    #[error("io error: {0}")]
    Io(String),
}

impl Error {
    /// Maps this error to its [`ErrorCategory`] for exit-code selection.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::NoDevice | Self::Usb(_) => ErrorCategory::ConnectionFailure,
            Self::Timeout => ErrorCategory::Timeout,
            Self::Decode(_) | Self::Validation(_) => ErrorCategory::ValidationFailure,
            Self::Io(_) => ErrorCategory::ExportFailure,
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
        assert_eq!(Error::Timeout.category(), ErrorCategory::Timeout);
        assert_eq!(Error::Decode("x".into()).category(), ErrorCategory::ValidationFailure);
    }
}
