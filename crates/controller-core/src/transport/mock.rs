//! A scripted `DeviceIo` for hardware-free tests.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::device::Model;
use crate::devices::models;
use crate::error::{Error, Result};
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};
use crate::transport::device_io::DeviceIo;
use crate::transport::write_input::{check_macro_stream, check_profile_blob};

/// The write operations a [`MockDevice`] can record and fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MockOp {
    /// [`DeviceIo::begin_write`].
    BeginWrite,
    /// [`DeviceIo::end_write`].
    EndWrite,
    /// [`DeviceIo::write_full_profile`].
    WriteFullProfile,
    /// [`DeviceIo::send_slot_select`].
    SlotSelect,
    /// [`DeviceIo::send_apply`].
    Apply,
    /// [`DeviceIo::write_macro_stream`].
    WriteMacroStream,
}

/// One write call a [`MockDevice`] received, with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    /// [`DeviceIo::begin_write`].
    BeginWrite,
    /// [`DeviceIo::end_write`], with the mode to return to.
    EndWrite(Mode),
    /// [`DeviceIo::write_full_profile`].
    WriteFullProfile {
        /// Target mode.
        mode: Mode,
        /// The blob that was sent.
        blob: Vec<u8>,
    },
    /// [`DeviceIo::send_slot_select`].
    SlotSelect(Mode),
    /// [`DeviceIo::send_apply`].
    Apply(Mode),
    /// [`DeviceIo::write_macro_stream`].
    WriteMacroStream {
        /// Target mode.
        mode: Mode,
        /// The profile slot the macro belongs to.
        profile_slot: Slot,
        /// The macro slot whose page was erased and written.
        macro_slot: MacroSlot,
        /// The stream that was sent.
        stream: Vec<u8>,
    },
}

impl MockCall {
    /// The operation this call belongs to.
    #[must_use]
    pub const fn op(&self) -> MockOp {
        match self {
            Self::BeginWrite => MockOp::BeginWrite,
            Self::EndWrite(_) => MockOp::EndWrite,
            Self::WriteFullProfile { .. } => MockOp::WriteFullProfile,
            Self::SlotSelect(_) => MockOp::SlotSelect,
            Self::Apply(_) => MockOp::Apply,
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
/// Reads return what was configured, after any failures queued with
/// [`MockDevice::fail_next_read`]. Writes are recorded in order and succeed unless
/// [`MockDevice::fail_nth`] says otherwise. The mock does not model device state unless
/// [`MockDevice::keeping_writes`] asks it to.
#[derive(Default)]
pub struct MockDevice {
    model: Option<&'static dyn Model>,
    profiles: Mutex<Option<ProfileReadResult>>,
    macro_streams: Mutex<HashMap<(Mode, u8, u8), Vec<u8>>>,
    keeps_writes: bool,
    readiness: Option<DeviceReadiness>,
    flip_back_to: Option<Mode>,
    current_mode: Option<Mode>,
    read_failures: Mutex<Vec<Error>>,
    writes: Mutex<WriteLog>,
}

impl MockDevice {
    /// Creates an empty mock.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Makes [`DeviceIo::current_mode`] answer `mode`, as a pad whose modes share one
    /// USB id would.
    #[must_use]
    pub const fn with_current_mode(mut self, mode: Mode) -> Self {
        self.current_mode = Some(mode);
        self
    }

    /// Configures what [`DeviceIo::model`] returns. Without it, the mock is the first
    /// model in the registry.
    #[must_use]
    pub fn with_model(mut self, model: &'static dyn Model) -> Self {
        self.model = Some(model);
        self
    }

    /// Configures what [`DeviceIo::read_all_profiles`] returns.
    #[must_use]
    pub fn with_profiles(mut self, result: ProfileReadResult) -> Self {
        self.profiles = Mutex::new(Some(result));
        self
    }

    /// Configures the macro step stream for a (mode, profile slot, macro slot).
    #[must_use]
    pub fn with_macro_stream(
        self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: Vec<u8>,
    ) -> Self {
        lock(&self.macro_streams).insert((mode, profile_slot.get(), macro_slot.get()), stream);
        self
    }

    /// Makes each write that succeeds change what later reads return, as a pad does: a
    /// profile blob replaces its mode's bank, and a macro stream replaces its page.
    #[must_use]
    pub const fn keeping_writes(mut self) -> Self {
        self.keeps_writes = true;
        self
    }

    /// Configures the readiness result.
    #[must_use]
    pub fn with_readiness(mut self, readiness: DeviceReadiness) -> Self {
        self.readiness = Some(readiness);
        self
    }

    /// Makes [`DeviceIo::begin_write`] report a flip, as a controller in a mode that
    /// takes no writes in place does. `back_to` is the mode it returns.
    #[must_use]
    pub const fn with_flip(mut self, back_to: Mode) -> Self {
        self.flip_back_to = Some(back_to);
        self
    }

    /// Makes the next [`DeviceIo::read_all_profiles`] return `error`. Queued failures
    /// come back in the order they were added.
    #[must_use]
    pub fn fail_next_read(self, error: Error) -> Self {
        lock(&self.read_failures).push(error);
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
        lock(&self.writes)
    }

    /// Puts `blob` in `mode`'s bank of the configured read, when writes are kept.
    fn keep_bank(&self, mode: Mode, blob: &[u8]) -> Result<()> {
        if !self.keeps_writes {
            return Ok(());
        }
        let index = self.model()?.description()?.modes.iter().position(|m| m.id == mode);
        if let Some(bank) =
            lock(&self.profiles).as_mut().zip(index).and_then(|(read, i)| read.raw_blobs.get_mut(i))
        {
            blob.clone_into(bank);
        }
        Ok(())
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
    fn model(&self) -> Result<&'static dyn Model> {
        self.model.or_else(|| models().first().copied()).ok_or(Error::NoDevice)
    }

    fn read_all_profiles(&self) -> Result<ProfileReadResult> {
        let mut failures = lock(&self.read_failures);
        if !failures.is_empty() {
            return Err(failures.remove(0));
        }
        drop(failures);
        lock(&self.profiles).clone().ok_or(Error::NoDevice)
    }

    fn read_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        _step_count: usize,
    ) -> Result<Vec<u8>> {
        lock(&self.macro_streams)
            .get(&(mode, profile_slot.get(), macro_slot.get()))
            .cloned()
            .ok_or(Error::NoDevice)
    }

    fn detect_readiness(&self) -> Result<DeviceReadiness> {
        self.readiness.clone().ok_or(Error::NoDevice)
    }

    fn current_mode(&self) -> Option<Mode> {
        self.current_mode
    }

    fn begin_write(&self) -> Result<Option<Mode>> {
        self.record(MockCall::BeginWrite)?;
        Ok(self.flip_back_to)
    }

    fn end_write(&self, back_to: Mode) -> Result<()> {
        self.record(MockCall::EndWrite(back_to))
    }

    fn write_full_profile(&self, mode: Mode, blob: &[u8]) -> Result<()> {
        check_profile_blob(blob, self.model()?.blob_size())?;
        self.record(MockCall::WriteFullProfile { mode, blob: blob.to_vec() })?;
        self.keep_bank(mode, blob)
    }

    fn send_slot_select(&self, mode: Mode) -> Result<()> {
        self.record(MockCall::SlotSelect(mode))
    }

    fn send_apply(&self, mode: Mode) -> Result<()> {
        self.record(MockCall::Apply(mode))
    }

    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()> {
        check_macro_stream(stream, macro_slot)?;
        let stream = stream.to_vec();
        self.record(MockCall::WriteMacroStream {
            mode,
            profile_slot,
            macro_slot,
            stream: stream.clone(),
        })?;
        if self.keeps_writes {
            lock(&self.macro_streams).insert((mode, profile_slot.get(), macro_slot.get()), stream);
        }
        Ok(())
    }
}

/// Locks `m`, taking the data of a poisoned lock: a test that panicked holding it has
/// already failed.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::pro3::{SWITCH, XINPUT};
    use crate::model::ProfileReadResult;

    #[allow(clippy::unwrap_used)]
    #[test]
    fn mock_returns_configured_profiles() {
        let dev = MockDevice::new().with_profiles(ProfileReadResult {
            raw_blobs: vec![vec![1, 2, 3]],
            ..Default::default()
        });
        let r = dev.read_all_profiles().unwrap();
        assert_eq!(r.raw_blobs, vec![vec![1, 2, 3]]);
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn writes_are_recorded_in_order() {
        let dev = MockDevice::new();
        dev.send_slot_select(XINPUT).unwrap();
        dev.write_full_profile(XINPUT, &[7; 0x092C]).unwrap();
        dev.send_apply(XINPUT).unwrap();
        let ops: Vec<MockOp> = dev.calls().iter().map(MockCall::op).collect();
        assert_eq!(ops, [MockOp::SlotSelect, MockOp::WriteFullProfile, MockOp::Apply]);
        assert_eq!(
            dev.calls().get(1),
            Some(&MockCall::WriteFullProfile { mode: XINPUT, blob: vec![7; 0x092C] })
        );
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn fail_nth_fails_only_that_occurrence() {
        let dev = MockDevice::new().fail_nth(MockOp::WriteFullProfile, 1, Error::write("boom"));
        let blob = [0u8; 0x092C];
        dev.write_full_profile(SWITCH, &blob).unwrap();
        assert!(matches!(dev.write_full_profile(SWITCH, &blob), Err(Error::Write { .. })));
        dev.write_full_profile(SWITCH, &blob).unwrap();
        assert_eq!(dev.calls().len(), 3);
    }

    #[allow(clippy::unwrap_used)]
    #[test]
    fn bad_inputs_are_rejected_and_not_recorded() {
        let dev = MockDevice::new();
        assert!(dev.write_full_profile(XINPUT, &[0; 10]).is_err());
        assert_eq!(dev.calls(), []);
    }
}
