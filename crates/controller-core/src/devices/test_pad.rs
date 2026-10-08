//! A made-up second controller, for tests only.
//!
//! It differs from the Pro 3 on purpose: one mode, two slots, no macros, a 64-byte blob
//! and its own model id. A test that passes for both models shows that the code around
//! the codec does not assume the Pro 3.
//!
//! Blob layout, 64 bytes:
//!
//! | Offset | Bytes | Holds |
//! | --- | --- | --- |
//! | `0x00` | 4 per slot | The 8BitDo active-slot marker |
//! | `0x10` | 2 per slot | Left and right vibration level |
//! | `0x20` | 16 per slot | Profile name, ASCII, zero-padded |

use std::sync::LazyLock;

use crate::description::ControllerDescription;
use crate::detect::ACTIVE_SLOT_MARKER;
use crate::device::{ControllerSpec, ProtocolCodec};
use crate::error::{Error, Result};
use crate::model::profile::canonical_id;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot, Sticks, Triggers, TriggersAnalog, Vibration,
};
use crate::protocol::bytes::{put_slice, read_u8, take};
use crate::protocol::wire_write::PACKET_LEN;

/// Blob size in bytes.
const BLOB_SIZE: usize = 0x40;
/// Bytes per slot of the active-marker table.
const MARKER_STRIDE: usize = 4;
/// Where the vibration levels start.
const VIBRATION_OFFSET: usize = 0x10;
/// Where the profile names start.
const NAME_OFFSET: usize = 0x20;
/// Bytes per profile name.
const NAME_LEN: usize = 16;

/// The made-up controller.
#[derive(Debug, Clone, Copy, Default)]
pub struct TestPad;

static DESCRIPTION: LazyLock<Result<ControllerDescription>> = LazyLock::new(|| {
    ControllerDescription::parse(
        include_str!("../../controllers/test-pad/description.json"),
        &[("front.svg", include_str!("../../controllers/test-pad/front.svg"))],
    )
});

/// The 0-based index of `slot`, refused past the test pad's two slots.
fn slot_index(slot: Slot) -> Result<usize> {
    match slot.get() {
        n @ 1..=2 => Ok(usize::from(n) - 1),
        n => Err(Error::Validation(format!("slot {n} out of range (1-2)"))),
    }
}

impl ControllerSpec for TestPad {
    fn description(&self) -> Result<&'static ControllerDescription> {
        DESCRIPTION.as_ref().map_err(Clone::clone)
    }
    fn blob_size(&self) -> usize {
        BLOB_SIZE
    }
    fn joydev_name_match(&self) -> &'static str {
        "Test Pad"
    }
    fn slot_select_value(&self, _mode: Mode) -> u8 {
        1
    }
    fn macro_gamepad_mode(&self, _mode: Mode) -> u8 {
        0
    }
    fn mode_flip_command(&self, _target: Mode) -> Option<[u8; PACKET_LEN]> {
        None
    }
    fn mode_close_command(&self) -> [u8; PACKET_LEN] {
        [0; PACKET_LEN]
    }
}

impl ProtocolCodec for TestPad {
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        let blob = &raw.payload;
        let i = slot_index(Slot::new(raw.source_slot)?)?;
        let mode = raw.mode_hint;
        let mut canonical = self.default_profile(mode);
        let name = take(blob, NAME_OFFSET + i * NAME_LEN, NAME_LEN)?;
        canonical.name = name.iter().take_while(|&&b| b != 0).map(|&b| char::from(b)).collect();
        canonical.vibration = Vibration {
            left_level: i32::from(read_u8(blob, VIBRATION_OFFSET + i * 2)?),
            right_level: i32::from(read_u8(blob, VIBRATION_OFFSET + i * 2 + 1)?),
        };
        canonical.id = canonical_id(mode, raw.source_slot, raw.source_profile_index);
        Ok(CanonicalProfileSummary {
            id: canonical.id.clone(),
            name: canonical.name.clone(),
            mode,
            source_slot: raw.source_slot,
            source_profile_index: raw.source_profile_index,
            canonical,
        })
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        CanonicalProfile {
            id: String::new(),
            name: String::new(),
            version: 1,
            kind: "test.pad.profile".to_owned(),
            device: "test-pad".to_owned(),
            mode,
            preferred_slot: None,
            sticks: Sticks {
                left_min_pct: 0,
                left_max_pct: 100,
                right_min_pct: 0,
                right_max_pct: 100,
                invert_left_x: false,
                invert_left_y: false,
                invert_right_x: false,
                invert_right_y: false,
                swap_sticks: false,
                swap_dpad_with_left_stick: false,
            },
            triggers: Triggers::Analog(TriggersAnalog {
                left_min_pct: 0,
                left_max_pct: 100,
                right_min_pct: 0,
                right_max_pct: 100,
                swap_triggers: false,
            }),
            vibration: Vibration { left_level: 1, right_level: 1 },
            button_mappings: Vec::new(),
            macro_refs: Vec::new(),
        }
    }

    fn decode_macro_metadata(&self, _blob: &[u8], _slot: Slot) -> Result<Vec<MacroDefinition>> {
        Ok(Vec::new())
    }

    fn decode_macro_steps(&self, _: &[u8], _: usize, _: Mode) -> Result<Vec<MacroStep>> {
        Ok(Vec::new())
    }

    fn encode_macro_steps(&self, _: &[MacroStep], _: Mode) -> Result<Vec<u8>> {
        Err(Error::Validation("the test pad has no macros".to_owned()))
    }

    fn encode_macro_metadata(&self, _: &MacroDefinition, _: MacroSlot) -> Result<Vec<u8>> {
        Err(Error::Validation("the test pad has no macros".to_owned()))
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        _macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        let i = slot_index(target_slot)?;
        let mut blob =
            if base_blob.len() == BLOB_SIZE { base_blob.to_vec() } else { vec![0; BLOB_SIZE] };
        put_slice(&mut blob, i * MARKER_STRIDE, &ACTIVE_SLOT_MARKER)?;
        let level = |v: i32| {
            u8::try_from(v).map_err(|_| Error::Validation(format!("vibration {v} out of range")))
        };
        let levels = [level(profile.vibration.left_level)?, level(profile.vibration.right_level)?];
        put_slice(&mut blob, VIBRATION_OFFSET + i * 2, &levels)?;
        let mut name = [0u8; NAME_LEN];
        for (dst, c) in name.iter_mut().zip(profile.name.chars().filter(char::is_ascii)) {
            *dst = u8::try_from(c).unwrap_or(b'?');
        }
        put_slice(&mut blob, NAME_OFFSET + i * NAME_LEN, &name)?;
        Ok(blob)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        self.compile_profile(profile, target_slot, base_blob, &[])
    }

    fn drop_macros(&self, blob: &[u8], _slot: Slot, _triggers: &[String]) -> Result<Vec<u8>> {
        Ok(blob.to_vec())
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        let mut blob = base_blob.to_vec();
        put_slice(&mut blob, slot_index(slot)? * MARKER_STRIDE, &[0; MARKER_STRIDE])?;
        Ok(blob)
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        self.description()?.validate_remap(mode, source, target)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::indexing_slicing)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::device::Model;
    use crate::devices::{find_model, pro3::Pro3};
    use crate::model::ProfileReadResult;
    use crate::orchestrator::ProfileWriteOrchestrator;
    use crate::service::read::{leftover_macros, read_macros};
    use crate::service::ConfirmPolicy;
    use crate::transport::mock::{MockCall, MockDevice};
    use crate::transport::DeviceIo as _;

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    /// A bank with slot 2 holding "Pad two" at vibration 2 and 3.
    fn bank() -> Vec<u8> {
        let mut profile = TestPad.default_profile(Mode::DInput);
        profile.name = "Pad two".to_owned();
        profile.vibration = Vibration { left_level: 2, right_level: 3 };
        TestPad.compile_profile(&profile, slot(2), &[], &[]).unwrap()
    }

    fn device() -> MockDevice {
        MockDevice::new()
            .with_model(&TestPad)
            .with_profiles(ProfileReadResult { raw_blobs: vec![bank()], ..Default::default() })
    }

    fn written(dev: &MockDevice) -> Vec<u8> {
        dev.calls()
            .into_iter()
            .find_map(|c| match c {
                MockCall::WriteFullProfile { blob, .. } => Some(blob),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn the_description_loads_and_the_model_id_finds_it() {
        let d = TestPad.description().unwrap();
        assert_eq!((d.modes.len(), d.slot_count, d.macro_slot_count), (1, 2, 0));
        let models: [&'static dyn Model; 2] = [&Pro3, &TestPad];
        let found = find_model(&models, 0x7e57).unwrap();
        assert_eq!(found.blob_size(), BLOB_SIZE);
        assert_eq!(find_model(&models, 0x6009).unwrap().blob_size(), 0x092C);
        assert!(find_model(&models, 0x1234).is_none());
    }

    #[test]
    fn a_blob_decodes_back_to_what_was_compiled() {
        let raw = RawProfilePayload {
            payload: bank(),
            source_slot: 2,
            source_profile_index: 1,
            mode_hint: Mode::DInput,
        };
        let summary = TestPad.map_profile(&raw).unwrap();
        assert_eq!(summary.name, "Pad two");
        assert_eq!(summary.canonical.vibration, Vibration { left_level: 2, right_level: 3 });
        assert!(TestPad.slot_active(&bank(), slot(2)).unwrap());
        assert!(!TestPad.slot_active(&bank(), slot(1)).unwrap());
        assert!(TestPad.compile_profile(&summary.canonical, slot(3), &[], &[]).is_err());
    }

    #[test]
    fn a_vibration_patch_writes_a_test_pad_blob_in_its_own_range() {
        let dev = device();
        let model = dev.model().unwrap();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, model, Path::new("."));

        let refused =
            orchestrator.patch_vibration(Mode::DInput, slot(2), 4, 0, &ConfirmPolicy::Force);
        assert!(!refused.success, "the test pad's vibration stops at 3: {}", refused.message);

        let done = orchestrator.patch_vibration(Mode::DInput, slot(2), 3, 0, &ConfirmPolicy::Force);
        assert!(done.success, "{}", done.message);
        let blob = written(&dev);
        assert_eq!(blob.len(), BLOB_SIZE);
        assert_eq!(&blob[VIBRATION_OFFSET + 2..VIBRATION_OFFSET + 4], [3, 0]);
        assert_eq!(&blob[NAME_OFFSET + NAME_LEN..NAME_OFFSET + NAME_LEN + 7], b"Pad two");
    }

    #[test]
    fn a_clear_and_a_macro_read_work_on_a_pad_without_macros() {
        let dev = device();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let cleared = orchestrator.deactivate_slot(Mode::DInput, slot(2), &ConfirmPolicy::Force);
        assert!(cleared.success, "{}", cleared.message);
        assert!(!TestPad.slot_active(&written(&dev), slot(2)).unwrap());

        assert_eq!(read_macros(&dev, Mode::DInput, slot(2)).unwrap().macros, []);
        let read = dev.read_all_profiles().unwrap();
        assert!(leftover_macros(&TestPad, &read).is_empty());
    }

    #[test]
    fn a_blob_of_another_size_is_refused_before_it_is_written() {
        let dev = MockDevice::new().with_model(&TestPad).with_profiles(ProfileReadResult {
            raw_blobs: vec![vec![0; 0x092C]],
            ..Default::default()
        });
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let result =
            orchestrator.patch_vibration(Mode::DInput, slot(2), 1, 1, &ConfirmPolicy::Force);
        assert!(!result.success);
        assert!(dev.calls().iter().all(|c| !matches!(c, MockCall::WriteFullProfile { .. })));
    }
}
