//! Write orchestrator: the one pipeline behind upload, deactivate, remap and the patches.
//!
//! Ports `ProfileWriteOrchestrator`. Every operation reads the slot back first, builds the
//! new blob from that readback, then does slot select, write, apply, with rollback if the
//! write fails. Building from the readback is what keeps the other slots intact, and the
//! macro handling in [`ProtocolCodec::compile_profile_keep_macros`] keeps the target slot's
//! macros (the C++ pipeline clears them).

use std::path::Path;

use serde_json::Value;

use crate::device::ProtocolCodec;
use crate::error::{Error, ErrorCategory, Result};
use crate::model::{CanonicalProfile, Mode, Slot, WriteResult};
use crate::service::validation::validate_profile;
use crate::service::{
    attempt_rollback, readback_and_confirm, ConfirmPolicy, FailedWrite, ReadbackResult,
};
use crate::transport::device_io::DeviceIo;

/// What a build step decided.
pub(super) enum Plan {
    /// Write this blob.
    Write(Vec<u8>),
    /// Nothing to write. The operation succeeds with this message.
    Nothing(String),
}

/// Message for the user from an error, without the "validation failed:" prefix.
pub(super) fn text(err: &Error) -> String {
    match err {
        Error::Validation(message) => message.clone(),
        other => other.to_string(),
    }
}

/// Builds the failed [`WriteResult`] for `err`.
pub(super) fn failure_from(mode: Mode, slot: Slot, err: &Error) -> WriteResult {
    WriteResult::failure(mode, slot, err.category(), text(err))
}

/// Runs profile writes against one device.
///
/// Create one per operation. It holds no state between calls.
pub struct ProfileWriteOrchestrator<'a> {
    pub(super) dev: &'a dyn DeviceIo,
    pub(super) codec: &'a dyn ProtocolCodec,
    backup_dir: &'a Path,
}

impl<'a> ProfileWriteOrchestrator<'a> {
    /// Creates an orchestrator. `backup_dir` receives the backup file when a rollback
    /// fails (the CLI passes the current directory, the GUI its data directory).
    #[must_use]
    pub const fn new(
        dev: &'a dyn DeviceIo,
        codec: &'a dyn ProtocolCodec,
        backup_dir: &'a Path,
    ) -> Self {
        Self { dev, codec, backup_dir }
    }

    /// Writes a canonical profile JSON into `slot` of `mode`.
    ///
    /// The profile is validated first (schema and semantic rules), must be for `mode`, and
    /// must not carry `macro_refs` (macros are read-only). The macros already in an occupied
    /// target slot are kept. Nothing is sent to the device if the input is invalid.
    #[must_use]
    pub fn upload_profile(
        &self,
        profile: &Value,
        mode: Mode,
        slot: Slot,
        policy: &ConfirmPolicy,
    ) -> WriteResult {
        let parsed = match check_upload(profile, mode) {
            Ok(parsed) => parsed,
            Err(e) => return failure_from(mode, slot, &e),
        };
        let mut result = self.run(mode, slot, policy, "Profile uploaded successfully.", |rb| {
            let blob = if rb.slot_active {
                self.codec.compile_profile_keep_macros(&parsed, slot, &rb.backup_blob)
            } else {
                self.codec.compile_profile(&parsed, slot, &rb.backup_blob, &[])
            };
            let blob =
                blob.map_err(|e| Error::Validation(format!("Compilation failed: {}", text(&e))))?;
            Ok(Plan::Write(blob))
        });
        result.profile_id = parsed.id;
        result
    }

    /// Clears `slot` of `mode`. An already empty slot succeeds without a write.
    #[must_use]
    pub fn deactivate_slot(&self, mode: Mode, slot: Slot, policy: &ConfirmPolicy) -> WriteResult {
        self.run(mode, slot, policy, "Slot deactivated.", |rb| {
            if !rb.slot_active {
                let n = slot.get();
                return Ok(Plan::Nothing(format!("Slot {n} in {mode} mode is already empty.")));
            }
            Ok(Plan::Write(self.codec.deactivate_profile(&rb.backup_blob, slot)?))
        })
    }

    /// The shared pipeline: readback and confirm, `build`, then one write job: flip if
    /// the current mode needs it, slot select, write, apply, flip back.
    ///
    /// `done` is the success message when a blob was written. A write that fails after the
    /// device was opened is rolled back. An apply that fails after a good write is not.
    /// The flip back goes out on every path once the flip succeeded. If it fails, the
    /// result keeps its outcome and its message says to replug the controller.
    pub(super) fn run(
        &self,
        mode: Mode,
        slot: Slot,
        policy: &ConfirmPolicy,
        done: &str,
        build: impl FnOnce(&ReadbackResult) -> Result<Plan>,
    ) -> WriteResult {
        let fail = |category, message: String| WriteResult::failure(mode, slot, category, message);

        let rb = match readback_and_confirm(self.dev, mode, slot, policy) {
            Ok(rb) => rb,
            Err(e) => return failure_from(mode, slot, &e),
        };
        if !rb.proceed {
            return fail(ErrorCategory::None, rb.message);
        }
        let blob = match build(&rb) {
            Ok(Plan::Write(blob)) => blob,
            Ok(Plan::Nothing(message)) => return WriteResult::success(mode, slot, message),
            Err(e) => return failure_from(mode, slot, &e),
        };

        let back_to = match self.dev.begin_write() {
            Ok(back_to) => back_to,
            Err(e) => return failure_from(mode, slot, &e),
        };
        let mut result = self.write(mode, slot, &rb, &blob, done);
        if let Some(back_to) = back_to {
            if let Err(e) = self.dev.end_write(back_to) {
                result.message = format!(
                    "{} The controller did not return to {back_to} mode ({e}); \
                     unplug it and plug it back in.",
                    result.message
                );
            }
        }
        result
    }

    /// Slot select, write, apply, with rollback if the write fails.
    fn write(
        &self,
        mode: Mode,
        slot: Slot,
        rb: &ReadbackResult,
        blob: &[u8],
        done: &str,
    ) -> WriteResult {
        let fail = |category, message: String| WriteResult::failure(mode, slot, category, message);
        if let Err(e) = self.dev.send_slot_select(mode) {
            return fail(ErrorCategory::ConnectionFailure, format!("Slot select failed: {e}"));
        }
        match self.dev.write_full_profile(mode, blob) {
            Ok(()) => {}
            Err(e @ Error::Write { .. }) => {
                return attempt_rollback(
                    self.dev,
                    mode,
                    slot,
                    &rb.backup_blob,
                    rb.slot_active,
                    FailedWrite::from_error(&e),
                    self.backup_dir,
                );
            }
            // The device never took the write (it could not be opened), so there is nothing to undo.
            Err(e) => return fail(e.category(), format!("Write failed: {e}")),
        }
        if let Err(e) = self.dev.send_apply(mode) {
            return fail(
                ErrorCategory::WriteFailure,
                format!("Write succeeded but APPLY failed: {e}"),
            );
        }
        WriteResult::success(mode, slot, done)
    }
}

/// Validates an upload and parses it. No device access.
fn check_upload(profile: &Value, mode: Mode) -> Result<CanonicalProfile> {
    let validation = validate_profile(profile)?;
    if !validation.valid {
        let details: Vec<String> =
            validation.errors.iter().map(|e| format!("{}: {}", e.path, e.reason)).collect();
        return Err(Error::Validation(format!(
            "Profile validation failed: {}",
            details.join("; ")
        )));
    }
    let parsed: CanonicalProfile = serde_json::from_value(profile.clone())
        .map_err(|e| Error::Validation(format!("Profile could not be read: {e}")))?;
    if parsed.mode != mode {
        return Err(Error::Validation(format!(
            "Mode mismatch: the target mode is '{mode}' but the profile is for '{}'.",
            parsed.mode
        )));
    }
    if !parsed.macro_refs.is_empty() {
        return Err(Error::Validation(
            "Profiles with macro_refs cannot be uploaded: macros are read-only.".to_owned(),
        ));
    }
    Ok(parsed)
}
