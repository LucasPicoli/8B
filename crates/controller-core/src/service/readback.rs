//! Pre-write readback: read the mode's blob, check whether the target slot is
//! occupied, and apply the confirmation policy before anything is overwritten.
//!
//! Ports `PreWriteReadbackService::readbackAndConfirm`. The blob it returns is both the
//! rollback backup and the read-modify-write base for `compile_profile`, which is how
//! the macros already on the controller survive a write.

use crate::detect::is_slot_active;
use crate::error::{Error, Result};
use crate::model::{Mode, ProfileReadResult, Slot};
use crate::service::read::blob_for_mode;
use crate::transport::device_io::DeviceIo;
use crate::transport::write_input::PROFILE_SIZE;

/// What to do when the target slot already holds a profile.
pub enum ConfirmPolicy {
    /// Overwrite without asking.
    Force,
    /// Refuse to overwrite. For non-interactive callers.
    Abort,
    /// Call the closure with a warning message; `true` overwrites, `false` aborts.
    Ask(Box<dyn Fn(&str) -> bool + Send>),
}

/// Result of [`readback_and_confirm`].
#[derive(Debug, Clone)]
pub struct ReadbackResult {
    /// Whether the write may proceed (slot empty, forced, or confirmed).
    pub proceed: bool,
    /// The blob read from the device, kept for rollback and as the compile base.
    pub backup_blob: Vec<u8>,
    /// Whether the target slot held a profile before the write.
    pub slot_active: bool,
    /// Status message. When `proceed` is false it says why.
    pub message: String,
}

/// Returns the blob of `mode`'s bank from a full read, checked for size.
///
/// # Errors
/// Returns [`Error::Usb`] if the read has no blob for `mode` or one that is not 2348
/// bytes.
pub fn bank_of(read: &ProfileReadResult, mode: Mode) -> Result<&Vec<u8>> {
    if read.raw_blobs.is_empty() {
        return Err(Error::Usb("pre-write readback returned no blobs".to_owned()));
    }
    let blob = blob_for_mode(read, mode)
        .ok_or_else(|| Error::Usb(format!("no blob available for mode '{mode}'")))?;
    if blob.len() != PROFILE_SIZE {
        return Err(Error::Usb(format!(
            "readback blob size mismatch (expected {PROFILE_SIZE}, got {})",
            blob.len()
        )));
    }
    Ok(blob)
}

/// Reads the blob for `mode`, checks `slot` for an existing profile, and applies `policy`.
///
/// A declined or aborted overwrite is not an error: it returns `Ok` with
/// `proceed == false` and the reason in `message`.
///
/// # Errors
/// Returns the device's error if the read fails. Returns [`Error::Usb`] if the read
/// returned no blob for `mode` or a blob that is not 2348 bytes.
pub fn readback_and_confirm(
    dev: &dyn DeviceIo,
    mode: Mode,
    slot: Slot,
    policy: &ConfirmPolicy,
) -> Result<ReadbackResult> {
    let read = dev.read_all_profiles()?;
    confirm_slot(bank_of(&read, mode)?, mode, slot, policy)
}

/// Checks `slot` of the bank `blob` for an existing profile and applies `policy`.
/// [`readback_and_confirm`] after the read, so a batch can read once and confirm
/// each of its slots.
///
/// # Errors
/// Returns [`Error::Decode`] if the slot's marker cannot be read from `blob`.
pub fn confirm_slot(
    blob: &[u8],
    mode: Mode,
    slot: Slot,
    policy: &ConfirmPolicy,
) -> Result<ReadbackResult> {
    let n = slot.get();
    let slot_active = is_slot_active(blob, slot)?;
    let result = |proceed: bool, message: String| ReadbackResult {
        proceed,
        backup_blob: blob.to_vec(),
        slot_active,
        message,
    };

    if !slot_active {
        return Ok(result(
            true,
            format!("Slot {n} in {mode} mode is empty. Proceeding with write."),
        ));
    }
    let warning = format!("Slot {n} in {mode} mode already contains an active profile.");
    Ok(match policy {
        ConfirmPolicy::Force => result(
            true,
            format!("Slot {n} in {mode} mode contains an active profile. Overwriting without confirmation."),
        ),
        ConfirmPolicy::Abort => result(false, format!("{warning} Use --force to overwrite.")),
        ConfirmPolicy::Ask(ask) if ask(&warning) => {
            result(true, format!("User confirmed overwrite of slot {n} in {mode} mode."))
        }
        ConfirmPolicy::Ask(_) => result(false, "Write aborted by user.".to_owned()),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::detect::ACTIVE_SLOT_MARKER;
    use crate::model::ProfileReadResult;
    use crate::transport::mock::MockDevice;

    fn blob(active: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; PROFILE_SIZE];
        for &s in active {
            let off = (usize::from(s) - 1) * 4;
            b[off..off + 4].copy_from_slice(&ACTIVE_SLOT_MARKER);
        }
        b
    }

    /// A full read with `mode_blob` in `mode`'s bank and the other banks empty.
    fn device(mode: Mode, mode_blob: &[u8]) -> MockDevice {
        let raw_blobs =
            Mode::ALL.iter().map(|&m| if m == mode { mode_blob.to_vec() } else { blob(&[]) });
        blobs(raw_blobs.collect())
    }

    fn blobs(raw_blobs: Vec<Vec<u8>>) -> MockDevice {
        MockDevice::new().with_profiles(ProfileReadResult { raw_blobs, ..Default::default() })
    }

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    fn asking(answer: bool, calls: &Arc<AtomicUsize>) -> ConfirmPolicy {
        let calls = Arc::clone(calls);
        ConfirmPolicy::Ask(Box::new(move |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            answer
        }))
    }

    #[test]
    fn empty_slot_proceeds_without_asking() {
        let dev = device(Mode::XInput, &blob(&[1]));
        let calls = Arc::new(AtomicUsize::new(0));
        let r = readback_and_confirm(&dev, Mode::XInput, slot(2), &asking(false, &calls)).unwrap();
        assert!(r.proceed && !r.slot_active);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(r.backup_blob, blob(&[1]));
    }

    #[test]
    fn active_slot_abort_policy_refuses() {
        let dev = device(Mode::XInput, &blob(&[2]));
        let r = readback_and_confirm(&dev, Mode::XInput, slot(2), &ConfirmPolicy::Abort).unwrap();
        assert!(!r.proceed && r.slot_active);
        assert!(r.message.contains("Use --force to overwrite"));
        assert_eq!(r.backup_blob, blob(&[2]));
    }

    #[test]
    fn active_slot_ask_decline_and_accept() {
        let dev = device(Mode::XInput, &blob(&[3]));
        let calls = Arc::new(AtomicUsize::new(0));
        let no = readback_and_confirm(&dev, Mode::XInput, slot(3), &asking(false, &calls)).unwrap();
        assert!(!no.proceed);
        assert_eq!(no.message, "Write aborted by user.");
        let yes = readback_and_confirm(&dev, Mode::XInput, slot(3), &asking(true, &calls)).unwrap();
        assert!(yes.proceed && yes.slot_active);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn active_slot_force_proceeds_without_asking() {
        let dev = device(Mode::DInput, &blob(&[1]));
        let r = readback_and_confirm(&dev, Mode::DInput, slot(1), &ConfirmPolicy::Force).unwrap();
        assert!(r.proceed && r.slot_active);
    }

    #[test]
    fn force_on_empty_slot_proceeds() {
        let dev = device(Mode::DInput, &blob(&[]));
        let r = readback_and_confirm(&dev, Mode::DInput, slot(1), &ConfirmPolicy::Force).unwrap();
        assert!(r.proceed && !r.slot_active);
    }

    #[test]
    fn switch_reads_the_switch_bank() {
        let dev = device(Mode::Switch, &blob(&[2]));
        let r = readback_and_confirm(&dev, Mode::Switch, slot(2), &ConfirmPolicy::Abort).unwrap();
        assert!(r.slot_active && !r.proceed);
    }

    #[test]
    fn read_error_propagates() {
        let dev = MockDevice::new();
        let err = readback_and_confirm(&dev, Mode::XInput, slot(1), &ConfirmPolicy::Force);
        assert!(matches!(err, Err(Error::NoDevice)));
    }

    #[test]
    fn missing_or_wrong_size_blob_is_an_error() {
        let none = blobs(vec![]);
        assert!(readback_and_confirm(&none, Mode::XInput, slot(1), &ConfirmPolicy::Force).is_err());
        let bad = device(Mode::DInput, &[0u8; 10]);
        assert!(readback_and_confirm(&bad, Mode::DInput, slot(1), &ConfirmPolicy::Force).is_err());
        let short = blobs(vec![blob(&[]), blob(&[])]);
        assert!(readback_and_confirm(&short, Mode::DInput, slot(1), &ConfirmPolicy::Force).is_err());
    }
}
