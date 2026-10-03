//! A scripted `DeviceIo` for hardware-free tests.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result};
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};
use crate::transport::device_io::DeviceIo;
use crate::transport::write_input::{check_macro_stream, check_patch, check_profile_blob};

/// The write operations a [`MockDevice`] can record and fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockOp {
    /// [`DeviceIo::write_full_profile`].
    WriteFullProfile,
    /// [`DeviceIo::write_patch`].
    WritePatch,
    /// [`DeviceIo::send_slot_select`].
    SlotSelect,
    /// [`DeviceIo::send_apply`].
    Apply,
    /// [`DeviceIo::query_status`].
    QueryStatus,
    /// [`DeviceIo::erase_macro`].
    EraseMacro,
    /// [`DeviceIo::write_macro_stream`].
    WriteMacroStream,
}

/// One write call a [`MockDevice`] received, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    /// [`DeviceIo::write_full_profile`].
    WriteFullProfile {
        /// Target mode.
        mode: Mode,
        /// The blob that was sent.
        blob: Vec<u8>,
    },
    /// [`DeviceIo::write_patch`].
    WritePatch {
        /// Target mode.
        mode: Mode,
        /// Blob offset of the patch.
        offset: u16,
        /// Patch bytes.
        data: Vec<u8>,
    },
    /// [`DeviceIo::send_slot_select`].
    SlotSelect(Mode),
    /// [`DeviceIo::send_apply`].
    Apply(Mode),
    /// [`DeviceIo::query_status`].
    QueryStatus(Mode),
    /// [`DeviceIo::erase_macro`].
    EraseMacro {
        /// Target mode.
        mode: Mode,
        /// 1-based profile slot.
        profile_slot: Slot,
        /// Macro slot.
        macro_slot: MacroSlot,
    },
    /// [`DeviceIo::write_macro_stream`].
    WriteMacroStream {
        /// Target mode.
        mode: Mode,
        /// 1-based profile slot.
        profile_slot: Slot,
        /// Macro slot.
        macro_slot: MacroSlot,
        /// The step stream that was sent.
        stream: Vec<u8>,
    },
}

impl MockCall {
    /// The operation this call belongs to.
    #[must_use]
    pub const fn op(&self) -> MockOp {
        match self {
            Self::WriteFullProfile { .. } => MockOp::WriteFullProfile,
            Self::WritePatch { .. } => MockOp::WritePatch,
            Self::SlotSelect(_) => MockOp::SlotSelect,
            Self::Apply(_) => MockOp::Apply,
            Self::QueryStatus(_) => MockOp::QueryStatus,
            Self::EraseMacro { .. } => MockOp::EraseMacro,
            Self::WriteMacroStream { .. } => MockOp::WriteMacroStream,
        }
    }
}

#[derive(Default)]
struct WriteLog {
    calls: Vec<MockCall>,
    failures: HashMap<(MockOp, usize), Error>,
}

/// A configurable in-memory device for unit tests.
///
/// Reads return what was configured. Writes are recorded in order and succeed unless
/// [`MockDevice::fail_nth`] says otherwise. The mock does not model device state.
#[derive(Default)]
pub struct MockDevice {
    profiles: HashMap<&'static str, ProfileReadResult>,
    macro_streams: HashMap<(&'static str, u8, u8), Vec<u8>>,
    readiness: Option<DeviceReadiness>,
    writes: Mutex<WriteLog>,
}

impl MockDevice {
    /// Creates an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Configures the profiles returned for `mode`.
    #[must_use]
    pub fn with_profiles(mut self, mode: Mode, result: ProfileReadResult) -> Self {
        self.profiles.insert(mode.as_str(), result);
        self
    }

    /// Configures the macro step stream for a (mode, profile slot, macro slot).
    #[must_use]
    pub fn with_macro_stream(
        mut self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: Vec<u8>,
    ) -> Self {
        self.macro_streams.insert((mode.as_str(), profile_slot.get(), macro_slot.get()), stream);
        self
    }

    /// Configures the readiness result.
    #[must_use]
    pub fn with_readiness(mut self, readiness: DeviceReadiness) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// Makes the `n`th (0-based) call of `op` return `error`. Counting is per op.
    #[must_use]
    pub fn fail_nth(self, op: MockOp, n: usize, error: Error) -> Self {
        self.log().failures.insert((op, n), error);
        self
    }

    /// Returns every write call received so far, failed ones included, in order.
    #[must_use]
    pub fn calls(&self) -> Vec<MockCall> {
        self.log().calls.clone()
    }

    fn log(&self) -> MutexGuard<'_, WriteLog> {
        self.writes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `call`, then fails it if a failure was scripted for this occurrence.
    fn record(&self, call: MockCall) -> Result<()> {
        let mut log = self.log();
        let op = call.op();
        let n = log.calls.iter().filter(|c| c.op() == op).count();
        log.calls.push(call);
        log.failures.get(&(op, n)).cloned().map_or(Ok(()), Err)
    }
}

impl DeviceIo for MockDevice {
    fn read_all_profiles(&self, mode: Mode) -> Result<ProfileReadResult> {
        self.profiles.get(mode.as_str()).cloned().ok_or(Error::NoDevice)
    }

    fn read_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        _step_count: usize,
    ) -> Result<Vec<u8>> {
        self.macro_streams
            .get(&(mode.as_str(), profile_slot.get(), macro_slot.get()))
            .cloned()
            .ok_or(Error::NoDevice)
    }

    fn detect_readiness(&self) -> Result<DeviceReadiness> {
        self.readiness.clone().ok_or(Error::NoDevice)
    }

    fn write_full_profile(&self, mode: Mode, blob: &[u8]) -> Result<()> {
        check_profile_blob(blob)?;
        self.record(MockCall::WriteFullProfile { mode, blob: blob.to_vec() })
    }

    fn write_patch(&self, mode: Mode, offset: u16, data: &[u8]) -> Result<()> {
        check_patch(data)?;
        self.record(MockCall::WritePatch { mode, offset, data: data.to_vec() })
    }

    fn send_slot_select(&self, mode: Mode) -> Result<()> {
        self.record(MockCall::SlotSelect(mode))
    }

    fn send_apply(&self, mode: Mode) -> Result<()> {
        self.record(MockCall::Apply(mode))
    }

    fn query_status(&self, mode: Mode) -> Result<()> {
        self.record(MockCall::QueryStatus(mode))
    }

    fn erase_macro(&self, mode: Mode, profile_slot: Slot, macro_slot: MacroSlot) -> Result<()> {
        self.record(MockCall::EraseMacro { mode, profile_slot, macro_slot })
    }

    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()> {
        check_macro_stream(stream, macro_slot)?;
        self.record(MockCall::WriteMacroStream {
            mode,
            profile_slot,
            macro_slot,
            stream: stream.to_vec(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Mode, ProfileReadResult};

    #[allow(clippy::unwrap_used)]
    #[test]
    fn mock_returns_configured_profiles() {
        let dev = MockDevice::new().with_profiles(
            Mode::XInput,
            ProfileReadResult { raw_blobs: vec![vec![1, 2, 3]], ..Default::default() },
        );
        let r = dev.read_all_profiles(Mode::XInput).unwrap();
        assert_eq!(r.raw_blobs, vec![vec![1, 2, 3]]);
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn writes_are_recorded_in_order() {
        let dev = MockDevice::new();
        dev.send_slot_select(Mode::XInput).unwrap();
        dev.write_full_profile(Mode::XInput, &[7; 0x092C]).unwrap();
        dev.send_apply(Mode::XInput).unwrap();
        let ops: Vec<MockOp> = dev.calls().iter().map(MockCall::op).collect();
        assert_eq!(ops, [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]);
        assert_eq!(
            dev.calls().get(1),
            Some(&MockCall::WriteFullProfile { mode: Mode::XInput, blob: vec![7; 0x092C] })
        );
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn fail_nth_fails_only_that_occurrence() {
        let dev = MockDevice::new().fail_nth(MockOp::WriteFullProfile, 1, Error::write("boom"));
        let blob = [0u8; 0x092C];
        dev.write_full_profile(Mode::Switch, &blob).unwrap();
        assert!(matches!(dev.write_full_profile(Mode::Switch, &blob), Err(Error::Write { .. })));
        dev.write_full_profile(Mode::Switch, &blob).unwrap();
        assert_eq!(dev.calls().len(), 3);
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn bad_inputs_are_rejected_and_not_recorded() {
        let dev = MockDevice::new();
        assert!(dev.write_full_profile(Mode::XInput, &[0; 10]).is_err());
        assert!(dev.write_patch(Mode::XInput, 0, &[]).is_err());
        let (ps, ms) = (Slot::new(1).unwrap(), MacroSlot::new(0).unwrap());
        assert!(dev.write_macro_stream(Mode::XInput, ps, ms, &[0; 33]).is_err());
        assert!(dev.calls().is_empty());
    }
}
