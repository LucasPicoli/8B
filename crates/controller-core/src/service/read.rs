//! Read services: thin orchestration of [`DeviceIo`] + codec calls.
//!
//! A read takes every bank of the model, so a blob is picked by mode alone.

use std::collections::BTreeMap;

use crate::device::Model;
use crate::error::{Error, Result};
use crate::model::{MacroDefinition, MacroSlot, Mode, ProfileReadResult, Slot};
use crate::transport::device_io::DeviceIo;

/// The result of a successful [`read_macros`] call.
#[derive(Debug, Clone)]
pub struct MacroReadResult {
    /// The active macro definitions read from the device, ordered by macro slot.
    pub macros: Vec<MacroDefinition>,
}

/// Reads the profiles of every mode's bank.
///
/// A thin delegate to [`DeviceIo::read_all_profiles`].
///
/// # Errors
/// Propagates any [`Error`] returned by the device.
pub fn read_profiles(dev: &dyn DeviceIo) -> Result<ProfileReadResult> {
    dev.read_all_profiles()
}

/// Reads and decodes all active macros for `profile_slot` in `mode`.
///
/// # Control flow
/// 1. Refuse a slot past the model's slot count, then read all profile blobs.
/// 2. Select the mode's blob via [`blob_for_mode`].
/// 3. Verify the blob is the model's blob size.
/// 4. Check that `profile_slot` is active; inactive slot → `Err`.
/// 5. Decode macro metadata descriptors from Section 4.
/// 6. For each descriptor, read the step stream and decode it.
///
/// # Errors
/// - [`Error::Validation`] if the slot is past the model's slot count or inactive.
/// - [`Error::Usb`] if no blob is available or the blob size mismatches.
/// - Any [`Error`] propagated from the codec or device I/O.
pub fn read_macros(dev: &dyn DeviceIo, mode: Mode, profile_slot: Slot) -> Result<MacroReadResult> {
    // 1. Refuse a slot past the model's, then read profile blobs. The read identifies
    // the model again, in case another pad took the port.
    profile_slot.check(dev.model()?.description()?.slot_count)?;
    let read = dev.read_all_profiles()?;
    let codec = dev.model()?;
    profile_slot.check(codec.description()?.slot_count)?;

    // 2-3. Pick the mode's blob, then size-check it.
    let blob = blob_for_mode(codec, &read, mode)
        .ok_or_else(|| Error::Usb("no blob available for mode".into()))?;

    if blob.len() != codec.blob_size() {
        return Err(Error::Usb(format!("readback blob size mismatch: {}", blob.len())));
    }

    // 4. Active-slot check — a zeroed blob (or any inactive slot) is an error.
    if !codec.slot_active(blob, profile_slot)? {
        return Err(Error::Validation(format!("no active profile in slot {}", profile_slot.get())));
    }

    // 5. Decode Section-4 macro metadata.
    let metadata = codec.decode_macro_metadata(blob, profile_slot)?;

    if metadata.is_empty() {
        return Ok(MacroReadResult { macros: Vec::new() });
    }

    // 6. Read step stream for each active macro and decode.
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
            // An empty stream keeps metadata-only steps; a read error propagates via
            // `?` above.
        } else {
            def.steps.clear();
        }

        macros.push(def);
    }

    Ok(MacroReadResult { macros })
}

/// Returns the blob of `mode`'s bank from a full read of `model`, which holds one blob
/// per mode in the order of the model's description. `None` when the read is short or
/// the model has no such mode.
pub(crate) fn blob_for_mode<'r>(
    model: &dyn Model,
    read: &'r ProfileReadResult,
    mode: Mode,
) -> Option<&'r Vec<u8>> {
    let index = model.description().ok()?.modes.iter().position(|m| m.id == mode)?;
    read.raw_blobs.get(index)
}

/// The empty slots of a full read that still hold macro descriptors, with how many.
///
/// A slot with no profile keeps the macros it had. The next profile written to it
/// replaces them, and a clear leaves them. A slot whose descriptors cannot be decoded
/// is left out.
#[must_use]
pub fn leftover_macros(model: &dyn Model, read: &ProfileReadResult) -> BTreeMap<(Mode, u8), usize> {
    read.profiles
        .iter()
        .filter(|p| p.id.is_empty())
        .filter_map(|p| {
            let blob = blob_for_mode(model, read, p.mode)?;
            let found = model.decode_macro_metadata(blob, Slot::new(p.source_slot).ok()?).ok()?;
            (!found.is_empty()).then_some(((p.mode, p.source_slot), found.len()))
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::device::ProtocolCodec as _;
    use crate::devices::pro3::Pro3;
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
        // One blob per bank, as a full read returns.
        let dev = MockDevice::new().with_profiles(ProfileReadResult {
            raw_blobs: vec![zeroed_blob(), zeroed_blob(), zeroed_blob()],
            ..Default::default()
        });
        let result = read_macros(&dev, Mode::XInput, Slot::new(1).unwrap());
        assert!(result.is_err(), "expected Err for inactive slot");
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Validation(_)), "expected Validation error, got: {err:?}");
    }

    #[test]
    fn read_macros_reads_the_dinput_macro_from_a_real_pad() {
        // A DInput bank read from a Pro 3: macro slot 3 of each slot holds a 256-step
        // stick sweep on `r4`. The stream is the one read back from slot 1.
        let bank = std::fs::read("../../fixtures/pro3/dinput-macro.blob").unwrap();
        let stream = std::fs::read("../../fixtures/pro3/dinput-macro.steps.bin").unwrap();
        let dev = MockDevice::new()
            .with_profiles(ProfileReadResult {
                raw_blobs: vec![zeroed_blob(), zeroed_blob(), bank],
                ..Default::default()
            })
            .with_macro_stream(
                Mode::DInput,
                Slot::new(1).unwrap(),
                MacroSlot::new(3).unwrap(),
                stream,
            );
        let result = read_macros(&dev, Mode::DInput, Slot::new(1).unwrap()).unwrap();
        assert_eq!(result.macros.len(), 1, "the empty descriptors are skipped");
        let m = &result.macros[0];
        assert_eq!((m.name.as_str(), m.trigger.as_str()), ("mx_d14", "r4"));
        assert_eq!((m.mode, m.macro_slot, m.repeat_count), (Mode::DInput, Some(3), 1));
        assert_eq!(m.steps.len(), 256);
        assert_eq!((m.steps[0].duration_ms, m.steps[0].left_stick_x), (10, 0x7F));
        assert_eq!((m.steps[1].left_stick_x, m.steps[1].left_stick_y), (0x80, 0x7E));
    }

    #[test]
    fn read_macros_empty_when_active_but_no_macros() {
        // Active slot-1 marker, but Section-4 (macro descriptors) all zeroed → no macros.
        let dev = MockDevice::new().with_profiles(ProfileReadResult {
            raw_blobs: vec![active_slot1_blob(), zeroed_blob(), zeroed_blob()],
            ..Default::default()
        });
        let result = read_macros(&dev, Mode::XInput, Slot::new(1).unwrap());
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
            .with_profiles(ProfileReadResult {
                raw_blobs: vec![meta, zeroed_blob(), zeroed_blob()],
                ..Default::default()
            })
            .with_macro_stream(
                Mode::XInput,
                Slot::new(1).unwrap(),
                MacroSlot::new(0).unwrap(),
                stream,
            );

        let result = read_macros(&dev, Mode::XInput, Slot::new(1).unwrap()).unwrap();
        assert_eq!(result.macros.len(), 1, "expected exactly one macro");
        assert_eq!(result.macros[0].name, "GoldenMac");
        assert_eq!(result.macros[0].steps.len(), 3);
    }

    #[test]
    fn an_empty_slot_with_descriptors_is_a_leftover_and_an_active_one_is_not() {
        let meta = std::fs::read("../../fixtures/pro3/macro-meta.blob").unwrap();
        let summary = |slot: u8, id: &str| crate::model::CanonicalProfileSummary {
            id: id.to_owned(),
            name: String::new(),
            mode: Mode::XInput,
            source_slot: slot,
            source_profile_index: slot - 1,
            canonical: Pro3.default_profile(Mode::XInput),
        };
        let mut read = ProfileReadResult {
            profiles: vec![summary(1, ""), summary(2, "")],
            raw_blobs: vec![meta, zeroed_blob(), zeroed_blob()],
        };
        let found = leftover_macros(&Pro3, &read);
        assert_eq!(found.get(&(Mode::XInput, 1)), Some(&1), "slot 1 holds the fixture macro");
        assert_eq!(found.len(), 1, "slot 2 holds none");
        read.profiles[0].id = "xinput-slot-1-index-0".to_owned();
        assert!(leftover_macros(&Pro3, &read).is_empty(), "an active slot is not a leftover");
    }
}
