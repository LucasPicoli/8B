//! A scripted `DeviceIo` for hardware-free tests.

use std::collections::HashMap;

use crate::error::{Error, Result};
use crate::model::{DeviceReadiness, MacroSlot, Mode, ProfileReadResult, Slot};
use crate::transport::device_io::DeviceIo;

/// A configurable in-memory device for unit tests.
#[derive(Default)]
pub struct MockDevice {
    profiles: HashMap<&'static str, ProfileReadResult>,
    macro_streams: HashMap<(&'static str, u8, u8), Vec<u8>>,
    readiness: Option<DeviceReadiness>,
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
}
