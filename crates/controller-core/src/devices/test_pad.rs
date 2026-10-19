//! A made-up second controller, for tests only.
//!
//! It differs from the Pro 3 on purpose: one mode, two slots, no macros, a 64-byte blob,
//! no model id (its USB id names it) and a firmware request of its own. A test that passes for both models shows that the code around
//! the codec does not assume the Pro 3.
//!
//! Blob layout, 64 bytes:
//!
//! | Offset | Bytes | Holds |
//! | --- | --- | --- |
//! | `0x00` | 4 per slot | The 8BitDo active-slot marker |
//! | `0x10` | 2 per slot | Rumble level, then lights on (1) or off (0) |
//! | `0x20` | 16 per slot | Profile name, ASCII, zero-padded |

use std::sync::LazyLock;

use crate::description::ControllerDescription;
use crate::detect::ACTIVE_SLOT_MARKER;
use crate::device::{ControllerSpec, ProtocolCodec, RawRequest};
use crate::error::{Error, Result};
use crate::model::profile::canonical_id;
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::bytes::{put_slice, read_u8, take};
use crate::protocol::wire_write::PACKET_LEN;

/// Blob size in bytes.
const BLOB_SIZE: usize = 0x40;
/// Bytes per slot of the active-marker table.
const MARKER_STRIDE: usize = 4;
/// Where the rumble level and light byte pairs start.
const FEEL_OFFSET: usize = 0x10;
/// Where the profile names start.
const NAME_OFFSET: usize = 0x20;
/// Bytes per profile name.
const NAME_LEN: usize = 16;
/// Head of the made-up firmware request.
const FIRMWARE_REQUEST: [u8; 4] = [0x81, 0x04, 0x21, 0x00];
/// Head of the made-up firmware reply.
const FIRMWARE_REPLY: [u8; 2] = [0x02, 0x22];

/// The made-up controller.
#[derive(Debug, Clone, Copy, Default)]
pub struct TestPad;

/// The test pad's one mode. No Pro 3 mode has this id.
pub const STANDARD: Mode = Mode::from_static("standard");

static DESCRIPTION: LazyLock<Result<ControllerDescription>> = LazyLock::new(|| {
    ControllerDescription::parse(
        include_str!("../../controllers/test-pad/description.json"),
        &[("front.svg", include_str!("../../controllers/test-pad/front.svg"))],
    )
});

/// The test pad's two settings groups.
fn feel(level: u8, light: bool) -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::from_iter([
        ("rumble".to_owned(), serde_json::json!({ "level": level })),
        ("lights".to_owned(), serde_json::json!({ "on": light })),
    ])
}

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
    fn apply_argument(&self) -> u16 {
        0x0123
    }
    fn firmware_request(&self) -> Option<RawRequest> {
        let mut wire = [0u8; PACKET_LEN];
        wire[..4].copy_from_slice(&FIRMWARE_REQUEST);
        Some(RawRequest { wire, reply_head: FIRMWARE_REPLY })
    }
    /// The major and minor version sit at reply bytes 2 and 3.
    fn firmware_version(&self, reply: &[u8]) -> Result<String> {
        Ok(format!("{}.{:02}", read_u8(reply, 2)?, read_u8(reply, 3)?))
    }
    /// The test pad answers through a mock, the way a pad with another protocol
    /// brings its own transport.
    fn transport(&self, _port: &str) -> Option<Box<dyn crate::transport::DeviceIo + Send>> {
        Some(Box::new(crate::transport::MockDevice::new().with_model(&Self)))
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
        canonical.settings =
            feel(read_u8(blob, FEEL_OFFSET + i * 2)?, read_u8(blob, FEEL_OFFSET + i * 2 + 1)? != 0);
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
            settings: feel(1, true),
            button_mappings: Vec::new(),
            macro_refs: Vec::new(),
            macros: None,
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
        let level = profile
            .setting("/rumble/level")
            .and_then(serde_json::Value::as_u64)
            .and_then(|v| u8::try_from(v).ok())
            .ok_or_else(|| Error::Validation("the profile has no rumble level".to_owned()))?;
        let light = profile.setting("/lights/on").and_then(serde_json::Value::as_bool);
        let light =
            light.ok_or_else(|| Error::Validation("the profile has no light".to_owned()))?;
        put_slice(&mut blob, FEEL_OFFSET + i * 2, &[level, u8::from(light)])?;
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

    fn set_macro(
        &self,
        _: &[u8],
        _: Slot,
        _: MacroSlot,
        _: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        Err(Error::Validation("the test pad has no macros".to_owned()))
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

/// The test pad's description with `validated` left out, a family and a lock: a model
/// nobody has checked on a real pad. It differs from [`TestPad`] in nothing else the
/// tests read.
fn unvalidated_description() -> Result<ControllerDescription> {
    let mut json: serde_json::Value =
        serde_json::from_str(include_str!("../../controllers/test-pad/description.json"))
            .map_err(|e| Error::Validation(e.to_string()))?;
    let map = json.as_object_mut().ok_or_else(|| Error::Validation("not an object".to_owned()))?;
    map.remove("validated");
    for (key, value) in [
        ("display_name", serde_json::json!("Unchecked Pad")),
        ("short_name", serde_json::json!("Unchecked")),
        ("family_name", serde_json::json!("Test family")),
        (
            "lock",
            serde_json::json!({
                "locked": { "usb": { "vendor": "0x2dc8", "product": "0x7e59" },
                            "product": "Unchecked Pad Controller" },
                "unlock": { "interface": 0, "endpoint": "0x02", "timeout_ms": 100,
                            "packets": [{ "at_ms": 0, "data": "0001" }] },
                "relock": { "packets": [{ "at_ms": 0, "data": "0002" }] }
            }),
        ),
    ] {
        map.insert(key.to_owned(), value);
    }
    if let Some(product) = json.pointer_mut("/config_ports/0/usb/product") {
        *product = "0x7e58".into();
    }
    ControllerDescription::parse(
        &json.to_string(),
        &[("front.svg", include_str!("../../controllers/test-pad/front.svg"))],
    )
}

static UNVALIDATED: LazyLock<Result<ControllerDescription>> =
    LazyLock::new(unvalidated_description);

/// A locked test pad whose model is not validated, for the tests of the refusal. Its
/// codec is the test pad's.
#[derive(Debug, Clone, Copy, Default)]
pub struct UnvalidatedPad;

impl ControllerSpec for UnvalidatedPad {
    fn description(&self) -> Result<&'static ControllerDescription> {
        UNVALIDATED.as_ref().map_err(Clone::clone)
    }
    fn blob_size(&self) -> usize {
        TestPad.blob_size()
    }
    fn apply_argument(&self) -> u16 {
        TestPad.apply_argument()
    }
}

impl ProtocolCodec for UnvalidatedPad {
    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        TestPad.map_profile(raw)
    }
    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        TestPad.default_profile(mode)
    }
    fn decode_macro_metadata(&self, blob: &[u8], slot: Slot) -> Result<Vec<MacroDefinition>> {
        TestPad.decode_macro_metadata(blob, slot)
    }
    fn decode_macro_steps(
        &self,
        stream: &[u8],
        count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>> {
        TestPad.decode_macro_steps(stream, count, mode)
    }
    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
        TestPad.encode_macro_steps(steps, mode)
    }
    fn encode_macro_metadata(&self, def: &MacroDefinition, slot: MacroSlot) -> Result<Vec<u8>> {
        TestPad.encode_macro_metadata(def, slot)
    }
    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        TestPad.compile_profile(profile, target_slot, base_blob, macros)
    }
    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        TestPad.compile_profile_keep_macros(profile, target_slot, base_blob)
    }
    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        TestPad.drop_macros(blob, slot, triggers)
    }
    fn set_macro(
        &self,
        blob: &[u8],
        slot: Slot,
        macro_slot: MacroSlot,
        def: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        TestPad.set_macro(blob, slot, macro_slot, def)
    }
    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        TestPad.deactivate_profile(base_blob, slot)
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
    use crate::devices::pro3::XINPUT;
    use crate::devices::{find_model, find_model_by_port, pro3::Pro3};
    use crate::model::ProfileReadResult;
    use crate::orchestrator::ProfileWriteOrchestrator;
    use crate::service::read::{leftover_macros, read_macros};
    use crate::service::validation::validate_profile;
    use crate::service::ConfirmPolicy;
    use crate::transport::mock::{MockCall, MockDevice};
    use crate::transport::DeviceIo as _;

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    /// A bank with slot 2 holding "Pad two" at rumble level 2 with the light off.
    fn bank() -> Vec<u8> {
        let mut profile = TestPad.default_profile(STANDARD);
        profile.name = "Pad two".to_owned();
        profile.settings = feel(2, false);
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
    fn a_read_that_finds_another_model_stops_the_write() {
        let pro3_bank = Pro3.compile_profile(&Pro3.default_profile(XINPUT), slot(1), &[], &[]);
        let blobs = vec![pro3_bank.unwrap(), Vec::new(), Vec::new()];
        let dev = MockDevice::new()
            .with_model(&TestPad)
            .with_profiles(ProfileReadResult { raw_blobs: blobs, ..Default::default() });
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &Pro3, Path::new("."));
        let mut profile = Pro3.default_profile(XINPUT);
        profile.id = "mine".to_owned();
        profile.name = "Mine".to_owned();
        let json = serde_json::to_value(profile).unwrap();
        let r = orchestrator.upload_profile(&json, XINPUT, slot(3), &ConfirmPolicy::Force);
        assert!(!r.success);
        assert!(r.message.contains("now a Test Pad"), "{}", r.message);
        let ops: Vec<_> = dev.calls().iter().map(MockCall::op).collect();
        assert_eq!(ops, [], "nothing went out after the read");
    }

    #[test]
    fn the_description_loads_and_the_usb_id_finds_it() {
        let d = TestPad.description().unwrap();
        assert_eq!((d.modes.len(), d.slot_count, d.macro_slot_count), (1, 2, 0));
        assert!(d.model_ids.is_empty(), "the test pad is named by its config port");
        let models: [&'static dyn Model; 2] = [&Pro3, &TestPad];
        let found = find_model_by_port(&models, d.config_ports[0].usb).unwrap();
        assert_eq!(found.blob_size(), BLOB_SIZE);
        assert_eq!(find_model(&models, 0x6009).unwrap().blob_size(), 0x092C);
        assert!(find_model(&models, 0x1234).is_none());
        assert_eq!(TestPad.firmware_version(&[0x02, 0x04, 1, 7, 0x21]).unwrap(), "1.07");
    }

    #[test]
    fn a_blob_decodes_back_to_what_was_compiled() {
        let raw = RawProfilePayload {
            payload: bank(),
            source_slot: 2,
            source_profile_index: 1,
            mode_hint: STANDARD,
        };
        let summary = TestPad.map_profile(&raw).unwrap();
        assert_eq!(summary.name, "Pad two");
        assert_eq!(summary.canonical.settings, feel(2, false));
        assert!(TestPad.slot_active(&bank(), slot(2)).unwrap());
        assert!(!TestPad.slot_active(&bank(), slot(1)).unwrap());
        assert!(TestPad.compile_profile(&summary.canonical, slot(3), &[], &[]).is_err());
    }

    #[test]
    fn a_settings_patch_writes_a_test_pad_blob_in_its_own_range() {
        let dev = device();
        let model = dev.model().unwrap();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, model, Path::new("."));

        let too_high = [("/rumble/level", 4.into())];
        let refused = orchestrator.patch_settings(STANDARD, slot(2), &too_high);
        assert!(!refused.success, "the test pad's rumble stops at 3: {}", refused.message);
        let pro3 = [("/vibration/left_level", 1.into())];
        let pro3 = orchestrator.patch_settings(STANDARD, slot(2), &pro3);
        assert!(pro3.message.contains("does not apply"), "{}", pro3.message);

        let set = [("/rumble/level", 3.into()), ("/lights/on", true.into())];
        let done = orchestrator.patch_settings(STANDARD, slot(2), &set);
        assert!(done.success, "{}", done.message);
        let blob = written(&dev);
        assert_eq!(blob.len(), BLOB_SIZE);
        assert_eq!(&blob[FEEL_OFFSET + 2..FEEL_OFFSET + 4], [3, 1]);
        assert_eq!(&blob[NAME_OFFSET + NAME_LEN..NAME_OFFSET + NAME_LEN + 7], b"Pad two");
    }

    #[test]
    fn a_clear_and_a_macro_read_work_on_a_pad_without_macros() {
        let dev = device();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let cleared = orchestrator.deactivate_slot(STANDARD, slot(2), &ConfirmPolicy::Force);
        assert!(cleared.success, "{}", cleared.message);
        assert!(!TestPad.slot_active(&written(&dev), slot(2)).unwrap());

        assert_eq!(read_macros(&dev, STANDARD, slot(2)).unwrap().macros, []);
        let read = dev.read_all_profiles().unwrap();
        assert!(leftover_macros(&TestPad, &read).is_empty());
    }

    #[test]
    fn its_own_profiles_validate_and_upload_and_wrong_ones_are_named() {
        let mut profile = TestPad.default_profile(STANDARD);
        profile.name = "Mine".to_owned();
        let json = serde_json::to_value(&profile).unwrap();
        let ok = validate_profile(&TestPad, &json).unwrap();
        assert!(ok.valid, "{:?}", ok.errors);

        let broken = |edit: fn(&mut serde_json::Value)| {
            let mut j = json.clone();
            edit(&mut j);
            let r = validate_profile(&TestPad, &j).unwrap();
            r.errors.into_iter().map(|e| e.path).collect::<Vec<_>>()
        };
        assert_eq!(broken(|j| j["device"] = "8bitdo-pro3".into()), ["/device"]);
        assert_eq!(broken(|j| j["rumble"]["level"] = 4.into()), ["/rumble/level"]);
        assert_eq!(broken(|j| j["lights"]["on"] = 1.into()), ["/lights/on"]);
        assert_eq!(broken(|j| j["sticks"] = serde_json::json!({})), ["/sticks"]);

        let dev = device();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let r = orchestrator.upload_profile(&json, STANDARD, slot(1), &ConfirmPolicy::Force);
        assert!(r.success, "{}", r.message);
        assert!(TestPad.slot_active(&written(&dev), slot(1)).unwrap());
    }

    #[test]
    fn a_slot_past_the_models_count_is_refused_before_any_device_access() {
        let dev = device();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let r = orchestrator.deactivate_slot(STANDARD, slot(3), &ConfirmPolicy::Force);
        assert!(!r.success && r.message.contains("1-2"), "{}", r.message);
        assert_eq!(dev.calls(), []);
        assert!(read_macros(&dev, STANDARD, slot(3)).is_err());
        let pro3 = Pro3.default_profile(XINPUT);
        assert!(Pro3.compile_profile(&pro3, slot(4), &[], &[]).is_err(), "the Pro 3 has 3 slots");
    }

    #[test]
    fn a_blob_of_another_size_is_refused_before_it_is_written() {
        let dev = MockDevice::new().with_model(&TestPad).with_profiles(ProfileReadResult {
            raw_blobs: vec![vec![0; 0x092C]],
            ..Default::default()
        });
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, Path::new("."));
        let set = [("/rumble/level", 1.into())];
        let result = orchestrator.patch_settings(STANDARD, slot(2), &set);
        assert!(!result.success);
        assert!(dev.calls().iter().all(|c| !matches!(c, MockCall::WriteFullProfile { .. })));
    }

    /// The files in `dir`, by name.
    fn files(dir: &Path) -> Vec<std::path::PathBuf> {
        let mut found: Vec<_> =
            std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).collect();
        found.sort();
        found
    }

    #[test]
    fn a_write_to_an_unvalidated_model_saves_the_bank_as_read_before_it_goes_out() {
        let dir = tempfile::tempdir().unwrap();
        let dev = MockDevice::new()
            .with_model(&UnvalidatedPad)
            .with_profiles(ProfileReadResult { raw_blobs: vec![bank()], ..Default::default() });
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &UnvalidatedPad, dir.path());
        let set = [("/rumble/level", 3.into())];
        let done = orchestrator.patch_settings(STANDARD, slot(2), &set);
        assert!(done.success, "{}", done.message);
        let saved = files(dir.path());
        assert_eq!(saved.len(), 1, "{saved:?}");
        let name = saved[0].file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("backup-standard-slot-2-before-write-"), "{name}");
        assert!(saved[0].extension().is_some_and(|e| e == "bin"), "{name}");
        assert_eq!(std::fs::read(&saved[0]).unwrap(), bank());
        assert_ne!(written(&dev), bank(), "the write changed the bank");
    }

    #[test]
    fn a_write_that_cannot_save_its_backup_does_not_go_out() {
        let dev = MockDevice::new()
            .with_model(&UnvalidatedPad)
            .with_profiles(ProfileReadResult { raw_blobs: vec![bank()], ..Default::default() });
        let missing = Path::new("/nonexistent/backups");
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &UnvalidatedPad, missing);
        let set = [("/rumble/level", 3.into())];
        let refused = orchestrator.patch_settings(STANDARD, slot(2), &set);
        assert!(!refused.success && refused.message.contains("backup"), "{}", refused.message);
        assert!(dev.calls().iter().all(|c| !matches!(c, MockCall::WriteFullProfile { .. })));
    }

    #[test]
    fn a_write_to_a_validated_model_saves_no_backup_unless_it_fails() {
        let dir = tempfile::tempdir().unwrap();
        let dev = device();
        let orchestrator = ProfileWriteOrchestrator::new(&dev, &TestPad, dir.path());
        let set = [("/rumble/level", 3.into())];
        assert!(orchestrator.patch_settings(STANDARD, slot(2), &set).success);
        assert_eq!(files(dir.path()), Vec::<std::path::PathBuf>::new());
    }
}
