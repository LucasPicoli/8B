//! Read services: thin orchestration of [`DeviceIo`] + codec calls.
//!
//! Ports `macro_read_service.cpp::readMacros` and the blob-selection helper
//! `pre_write_readback_service.cpp::blobIndexForMode`.

use crate::device::ProtocolCodec;
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{MacroDefinition, MacroSlot, Mode, ProfileReadResult, Slot};
use crate::transport::device_io::DeviceIo;

/// Expected size of a profile blob in bytes.
const EXPECTED_BLOB_SIZE: usize = 0x092C;

/// The result of a successful [`read_macros`] call.
#[derive(Debug, Clone)]
pub struct MacroReadResult {
    /// The active macro definitions read from the device, ordered by macro slot.
    pub macros: Vec<MacroDefinition>,
}

/// Reads all on-device profiles for `mode`.
///
/// A thin delegate to [`DeviceIo::read_all_profiles`].
///
/// # Errors
/// Propagates any [`Error`] returned by the device.
pub fn read_profiles(dev: &dyn DeviceIo, mode: Mode) -> Result<ProfileReadResult> {
    dev.read_all_profiles(mode)
}

/// Reads and decodes all active macros for `profile_slot` in `mode`.
///
/// Faithful port of `MacroReadService::readMacros` from
/// `src/core/macro_read_service.cpp` (lines ~85-210).
///
/// # Control flow
/// 1. Reject `Mode::DInput` (not a valid macro mode).
/// 2. Read all profile blobs for `mode`.
/// 3. Select the right blob via [`blob_index_for_mode`].
/// 4. Verify blob size == `0x092C`.
/// 5. Check that `profile_slot` is active; inactive slot → `Err`.
/// 6. Decode macro metadata descriptors from Section 4.
/// 7. For each descriptor, read the step stream and decode it.
///
/// # Errors
/// - [`Error::Validation`] if `mode` is `DInput` or the slot is inactive.
/// - [`Error::Usb`] if no blob is available or the blob size mismatches.
/// - Any [`Error`] propagated from the codec or device I/O.
pub fn read_macros(
    dev: &dyn DeviceIo,
    codec: &Pro3,
    mode: Mode,
    profile_slot: Slot,
) -> Result<MacroReadResult> {
    // 1. Mode validation — DInput does not support macros.
    if mode == Mode::DInput {
        return Err(Error::Validation("invalid mode 'dinput': must be xinput or switch".into()));
    }

    // 2. Read profile blobs.
    let read = dev.read_all_profiles(mode)?;

    // 3. Select blob index for mode.
    let idx = blob_index_for_mode(mode, read.raw_blobs.len())
        .ok_or_else(|| Error::Usb("no blob available for mode".into()))?;

    // 4. Bounds-check and size-check the blob.
    let blob =
        read.raw_blobs.get(idx).ok_or_else(|| Error::Usb("no blob available for mode".into()))?;

    if blob.len() != EXPECTED_BLOB_SIZE {
        return Err(Error::Usb(format!("readback blob size mismatch: {}", blob.len())));
    }

    // 5. Active-slot check — a zeroed blob (or any inactive slot) is an error.
    if !crate::detect::is_slot_active(blob, profile_slot)? {
        return Err(Error::Validation(format!("no active profile in slot {}", profile_slot.get())));
    }

    // 6. Decode Section-4 macro metadata.
    let metadata = codec.decode_macro_metadata(blob, profile_slot)?;

    if metadata.is_empty() {
        return Ok(MacroReadResult { macros: Vec::new() });
    }

    // 7. Read step stream for each active macro and decode.
    let mut macros = Vec::with_capacity(metadata.len());

    for mut def in metadata {
        let step_count = def.steps.len();

        if step_count > 0 {
            let ms = MacroSlot::new(
                def.macro_slot
                    .ok_or_else(|| Error::Decode("macro descriptor missing slot index".into()))?,
            )?;
            let stream = dev.read_macro_stream(mode, profile_slot, ms, step_count)?;
            if !stream.is_empty() {
                def.steps = codec.decode_macro_steps(&stream, step_count, def.mode)?;
            }
            // C++ tolerates an empty stream by keeping metadata-only steps; Rust
            // propagates a read ERROR via `?` above — stricter than C++.
        } else {
            def.steps.clear();
        }

        macros.push(def);
    }

    Ok(MacroReadResult { macros })
}

/// Selects the index of the profile blob that corresponds to `mode`.
///
/// Ports `PreWriteReadbackService::blobIndexForMode`:
/// - Two blobs (product 0x310b): `XInput` → 0, `Switch` → 1.
/// - One blob (product 0x6009): any mode → 0.
/// - Any other blob count → `None`.
const fn blob_index_for_mode(mode: Mode, blob_count: usize) -> Option<usize> {
    match blob_count {
        2 => match mode {
            Mode::XInput => Some(0),
            Mode::Switch => Some(1),
            Mode::DInput => None,
        },
        1 => Some(0),
        _ => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::model::ProfileReadResult;
    use crate::transport::mock::MockDevice;

    /// Helper: build a zeroed but correctly-sized blob.
    fn zeroed_blob() -> Vec<u8> {
        vec![0u8; 0x092C]
    }

    /// Helper: build a blob with the slot-1 active marker set.
    fn active_slot1_blob() -> Vec<u8> {
        let mut blob = zeroed_blob();
        blob[0..4].copy_from_slice(&crate::detect::ACTIVE_SLOT_MARKER);
        blob
    }

    #[test]
    fn read_macros_errors_when_slot_inactive() {
        // Two-blob layout so blob_index_for_mode resolves for XInput.
        let dev = MockDevice::new().with_profiles(
            Mode::XInput,
            ProfileReadResult {
                raw_blobs: vec![zeroed_blob(), zeroed_blob()],
                ..Default::default()
            },
        );
        let result = read_macros(&dev, &Pro3, Mode::XInput, Slot::new(1).unwrap());
        assert!(result.is_err(), "expected Err for inactive slot");
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "expected Validation error, got: {err:?}");
    }

    #[test]
    fn read_macros_dinput_is_rejected() {
        // Mode validation runs before any device call, so no profile setup needed.
        let dev = MockDevice::new();
        let result = read_macros(&dev, &Pro3, Mode::DInput, Slot::new(1).unwrap());
        assert!(result.is_err(), "expected Err for DInput mode");
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "expected Validation error, got: {err:?}");
    }

    #[test]
    fn read_macros_empty_when_active_but_no_macros() {
        // Active slot-1 marker, but Section-4 (macro descriptors) all zeroed → no macros.
        let dev = MockDevice::new().with_profiles(
            Mode::XInput,
            ProfileReadResult {
                raw_blobs: vec![active_slot1_blob(), zeroed_blob()],
                ..Default::default()
            },
        );
        let result = read_macros(&dev, &Pro3, Mode::XInput, Slot::new(1).unwrap());
        assert!(result.is_ok(), "expected Ok, got: {result:?}");
        assert!(result.unwrap().macros.is_empty(), "expected empty macro list");
    }

    #[test]
    fn read_macros_golden_positive() {
        // Load the committed golden fixtures at runtime (CWD is the crate dir under cargo test).
        let mut meta = std::fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
        let stream = std::fs::read("../../fixtures/pro3/macro-sample.steps.bin").unwrap();

        // Ensure slot-1 active marker is set.
        if meta[0..4] != crate::detect::ACTIVE_SLOT_MARKER {
            meta[0..4].copy_from_slice(&crate::detect::ACTIVE_SLOT_MARKER);
        }

        let dev = MockDevice::new()
            .with_profiles(
                Mode::XInput,
                ProfileReadResult { raw_blobs: vec![meta, zeroed_blob()], ..Default::default() },
            )
            .with_macro_stream(
                Mode::XInput,
                Slot::new(1).unwrap(),
                MacroSlot::new(0).unwrap(),
                stream,
            );

        let result = read_macros(&dev, &Pro3, Mode::XInput, Slot::new(1).unwrap()).unwrap();
        assert_eq!(result.macros.len(), 1, "expected exactly one macro");
        assert_eq!(result.macros[0].name, "GoldenMac");
        assert_eq!(result.macros[0].steps.len(), 3);
    }
}
