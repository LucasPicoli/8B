//! A made-up controller that keeps its macro steps inside the profile blob, for tests
//! only.
//!
//! It is the Pro 3 with a step area added after the Pro 3 blob: one fixed area per
//! profile slot and macro slot, holding the Pro 3 step stream zero-padded. A test that
//! writes and reads its macros shows that no step page is touched for such a model.
#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::description::ControllerDescription;
use crate::device::{ControllerSpec, ProtocolCodec};
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, MacroDefinition, MacroSlot, MacroStep, Mode,
    RawProfilePayload, Slot,
};
use crate::protocol::bytes::{put_slice, take};
use crate::service::validation::ValidationError;

/// The Pro 3 blob size.
const HEAD: usize = 0x092C;
/// Bytes of one macro's step area, room for 9 steps.
const AREA: usize = 96;
/// Step areas per profile slot, one per macro slot.
const MACROS: usize = 4;
/// The step areas of the 3 profile slots.
const TAIL: usize = 3 * MACROS * AREA;

/// The made-up controller.
#[derive(Debug, Clone, Copy, Default)]
struct InlinePad;

/// Where the step area of macro slot `m` of `slot` starts.
fn area(slot: Slot, m: MacroSlot) -> usize {
    HEAD + ((usize::from(slot.get()) - 1) * MACROS + usize::from(m.get())) * AREA
}

/// The Pro 3 blob and the step areas of a full blob.
fn split(blob: &[u8]) -> Result<(&[u8], &[u8])> {
    if blob.len() != HEAD + TAIL {
        return Err(Error::Validation(format!("not a full blob: {} bytes", blob.len())));
    }
    Ok(blob.split_at(HEAD))
}

/// `head` with `tail` after it.
fn join(head: Vec<u8>, tail: &[u8]) -> Vec<u8> {
    [head, tail.to_vec()].concat()
}

/// Puts `steps` in the step area of macro slot `m` of `slot`, zero-padded.
fn put_steps(
    blob: &mut [u8],
    slot: Slot,
    m: MacroSlot,
    steps: &[MacroStep],
    mode: Mode,
) -> Result<()> {
    let mut stream =
        if steps.is_empty() { Vec::new() } else { Pro3.encode_macro_steps(steps, mode)? };
    if stream.len() > AREA {
        return Err(Error::Validation("too many steps for the step area".to_owned()));
    }
    stream.resize(AREA, 0);
    put_slice(blob, area(slot, m), &stream)
}

impl ControllerSpec for InlinePad {
    fn description(&self) -> Result<&'static ControllerDescription> {
        Pro3.description()
    }
    fn blob_size(&self) -> usize {
        HEAD + TAIL
    }
    fn apply_argument(&self) -> u16 {
        Pro3.apply_argument()
    }
    fn slot_select_value(&self, mode: Mode) -> Result<u8> {
        Pro3.slot_select_value(mode)
    }
    fn macro_gamepad_mode(&self, mode: Mode) -> Result<u8> {
        Pro3.macro_gamepad_mode(mode)
    }
}

impl ProtocolCodec for InlinePad {
    fn macro_steps_in_blob(&self) -> bool {
        true
    }

    fn map_profile(&self, raw: &RawProfilePayload) -> Result<CanonicalProfileSummary> {
        let head = split(&raw.payload)?.0.to_vec();
        Pro3.map_profile(&RawProfilePayload { payload: head, ..raw.clone() })
    }

    fn default_profile(&self, mode: Mode) -> CanonicalProfile {
        Pro3.default_profile(mode)
    }

    fn profile_schema_errors(&self, json: &serde_json::Value) -> Result<Vec<ValidationError>> {
        Pro3.profile_schema_errors(json)
    }

    fn decode_macro_metadata(&self, blob: &[u8], slot: Slot) -> Result<Vec<MacroDefinition>> {
        Pro3.decode_macro_metadata(split(blob)?.0, slot)?
            .into_iter()
            .map(|mut def| {
                let m = MacroSlot::new(def.macro_slot.unwrap_or_default())?;
                let stream = take(blob, area(slot, m), AREA)?;
                def.steps = Pro3.decode_macro_steps(stream, def.steps.len(), def.mode)?;
                Ok(def)
            })
            .collect()
    }

    fn decode_macro_steps(
        &self,
        stream: &[u8],
        count: usize,
        mode: Mode,
    ) -> Result<Vec<MacroStep>> {
        Pro3.decode_macro_steps(stream, count, mode)
    }

    fn encode_macro_steps(&self, steps: &[MacroStep], mode: Mode) -> Result<Vec<u8>> {
        Pro3.encode_macro_steps(steps, mode)
    }

    fn encode_macro_metadata(&self, def: &MacroDefinition, m: MacroSlot) -> Result<Vec<u8>> {
        Pro3.encode_macro_metadata(def, m)
    }

    fn compile_profile(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
        macros: &[MacroDefinition],
    ) -> Result<Vec<u8>> {
        let (head, tail) = split(base_blob).unwrap_or((&[], &[0; TAIL]));
        let mut blob = join(Pro3.compile_profile(profile, target_slot, head, macros)?, tail);
        for def in macros {
            let m = MacroSlot::new(def.macro_slot.unwrap_or_default())?;
            put_steps(&mut blob, target_slot, m, &def.steps, def.mode)?;
        }
        Ok(blob)
    }

    fn compile_profile_keep_macros(
        &self,
        profile: &CanonicalProfile,
        target_slot: Slot,
        base_blob: &[u8],
    ) -> Result<Vec<u8>> {
        let (head, tail) = split(base_blob)?;
        Ok(join(Pro3.compile_profile_keep_macros(profile, target_slot, head)?, tail))
    }

    fn drop_macros(&self, blob: &[u8], slot: Slot, triggers: &[String]) -> Result<Vec<u8>> {
        let (head, tail) = split(blob)?;
        Ok(join(Pro3.drop_macros(head, slot, triggers)?, tail))
    }

    fn set_macro(
        &self,
        blob: &[u8],
        slot: Slot,
        m: MacroSlot,
        def: Option<&MacroDefinition>,
    ) -> Result<Vec<u8>> {
        let (head, tail) = split(blob)?;
        let mut out = join(Pro3.set_macro(head, slot, m, def)?, tail);
        match def {
            Some(def) => put_steps(&mut out, slot, m, &def.steps, def.mode)?,
            None => put_slice(&mut out, area(slot, m), &[0; AREA])?,
        }
        Ok(out)
    }

    fn deactivate_profile(&self, base_blob: &[u8], slot: Slot) -> Result<Vec<u8>> {
        let (head, tail) = split(base_blob)?;
        Ok(join(Pro3.deactivate_profile(head, slot)?, tail))
    }

    fn validate_remap(&self, mode: Mode, source: &str, target: &str) -> Result<()> {
        Pro3.validate_remap(mode, source, target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::pro3::XINPUT;
    use crate::model::macros::macro_to_json;
    use crate::model::{ProfileReadResult, WriteResult};
    use crate::orchestrator::ProfileWriteOrchestrator;
    use crate::service::read::{read_macro_steps, read_macros};
    use crate::service::ConfirmPolicy;
    use crate::transport::mock::{MockCall, MockDevice, MockOp};
    use crate::transport::DeviceIo as _;

    fn slot(n: u8) -> Slot {
        Slot::new(n).unwrap()
    }

    fn macro_slot(n: u8) -> MacroSlot {
        MacroSlot::new(n).unwrap()
    }

    /// `n` steps that press `button` and let go in turn, so no run folds into a glide.
    fn steps(button: &str, n: usize) -> Vec<MacroStep> {
        (0..n)
            .map(|i| MacroStep {
                duration_ms: 100,
                pressed_buttons: if i % 2 == 0 { vec![button.into()] } else { Vec::new() },
                ..MacroStep::default()
            })
            .collect()
    }

    fn def(name: &str, trigger: &str, steps: Vec<MacroStep>) -> MacroDefinition {
        MacroDefinition {
            name: name.into(),
            mode: XINPUT,
            trigger_buttons: vec![trigger.into()],
            unknown_trigger_bits: 0,
            repeat_count: 1,
            interval_ms: 0,
            steps,
            macro_slot: None,
        }
    }

    /// The macro in macro slot 0 of `XInput` slot 1 before the write.
    fn old() -> MacroDefinition {
        MacroDefinition { macro_slot: Some(0), ..def("Old", "l1", steps("bottom face", 3)) }
    }

    /// An `XInput` bank whose slot 1 holds a profile and [`old`].
    fn bank() -> Vec<u8> {
        InlinePad.compile_profile(&Pro3.default_profile(XINPUT), slot(1), &[], &[old()]).unwrap()
    }

    /// A pad that holds [`bank`] and no step page, so any page read fails.
    fn pad() -> MockDevice {
        MockDevice::new().with_model(&InlinePad).with_profiles(ProfileReadResult {
            raw_blobs: vec![bank(), vec![0; HEAD + TAIL], vec![0; HEAD + TAIL]],
            ..ProfileReadResult::default()
        })
    }

    fn put(dev: &MockDevice, new: &MacroDefinition) -> WriteResult {
        let dir = tempfile::tempdir().unwrap();
        ProfileWriteOrchestrator::new(dev, &InlinePad, dir.path()).put_macro(
            &macro_to_json(new, "8bitdo-pro3"),
            XINPUT,
            slot(1),
            macro_slot(0),
            &ConfirmPolicy::Force,
        )
    }

    fn ops(dev: &MockDevice) -> Vec<MockOp> {
        dev.calls().iter().map(MockCall::op).collect()
    }

    #[test]
    fn a_replace_writes_one_blob_and_no_page_and_reads_back_from_the_blob() {
        let dev = pad().keeping_writes();
        let new = def("New", "r4", steps("right face", 4));
        let r = put(&dev, &new);
        assert!(r.success, "{}", r.message);
        let blobs = ops(&dev).iter().filter(|&&op| op == MockOp::WriteFullProfile).count();
        assert_eq!(blobs, 1);
        assert!(!ops(&dev).contains(&MockOp::WriteMacroStream));

        let read = read_macros(&dev, XINPUT, slot(1)).unwrap().macros;
        assert_eq!(read, [MacroDefinition { macro_slot: Some(0), ..new.clone() }]);
        let held = &dev.read_all_profiles().unwrap().raw_blobs[0];
        let summary = InlinePad.decode_macro_metadata(held, slot(1)).unwrap();
        let full = read_macro_steps(&dev, vec![((XINPUT, 1), summary[0].clone())]).unwrap();
        assert_eq!(full.into_iter().map(|(_, d)| d.steps).collect::<Vec<_>>(), [new.steps]);
    }

    #[test]
    fn a_readback_that_holds_other_steps_fails() {
        // The pad keeps the old blob, whose descriptor matches the new one but not its steps.
        let dev = pad();
        let new = MacroDefinition { macro_slot: None, ..def("Old", "l1", steps("left face", 3)) };
        let sent = InlinePad.set_macro(&bank(), slot(1), macro_slot(0), Some(&new)).unwrap();
        let descriptors =
            |b: &[u8]| Pro3.decode_macro_metadata(split(b).unwrap().0, slot(1)).unwrap();
        assert_eq!(descriptors(&sent), descriptors(&bank()), "only the steps differ");

        let r = put(&dev, &new);
        assert!(!r.success);
        assert!(r.message.contains("Read-back check failed"), "{}", r.message);
        assert!(!ops(&dev).contains(&MockOp::WriteMacroStream));
    }

    #[test]
    fn a_failed_blob_write_rolls_the_blob_back_and_touches_no_page() {
        let dev = pad().fail_nth(MockOp::WriteFullProfile, 0, Error::write("boom"));
        let new = def("New", "r4", steps("right face", 4));
        let r = put(&dev, &new);
        assert!(!r.success && r.rollback_succeeded, "{}", r.message);
        let sent = InlinePad.set_macro(&bank(), slot(1), macro_slot(0), Some(&new)).unwrap();
        let blob = |blob| MockCall::WriteFullProfile { mode: XINPUT, blob };
        assert_eq!(
            dev.calls(),
            [
                MockCall::BeginWrite,
                MockCall::SlotSelect(XINPUT),
                blob(sent),
                blob(bank()),
                MockCall::Apply(XINPUT),
                MockCall::EndWrite(None),
            ]
        );
    }
}
