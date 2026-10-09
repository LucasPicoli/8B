//! Real transport over the hidraw node of the config interface.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::detect::scan_sysfs;
use crate::device::Model;
use crate::devices::config_ports;
use crate::error::{Error, Result};
use crate::model::{
    CanonicalProfile, CanonicalProfileSummary, DeviceReadiness, MacroSlot, Mode, ProfileReadResult,
    RawProfilePayload, Slot,
};
use crate::protocol::wire::{
    build_query_status, build_read_macro_packet, build_slot_select, build_upload_packet,
    decode_read_macro_response, decode_upload_response,
};
use crate::protocol::wire_write::MACRO_PAGE_LEN;
use crate::transport::hidraw_write;
use crate::transport::session::{Session, Target, READ_TIMEOUT};
use crate::transport::write_input::PROFILE_CHUNK as UPLOAD_CHUNK;

const MACRO_CHUNK: u16 = 32;
const MACRO_ERASE_LEN: u16 = MACRO_PAGE_LEN;

/// Real transport over hidraw, in every current mode.
///
/// The USB id of the attached controller picks the node and the framing. The model id
/// in its `START_CONFIG` reply picks the model, on every session.
pub struct HidrawDevice {
    /// The USB port path to talk to, or `None` for the first controller found.
    port: Option<String>,
    /// The model the last read session identified, for [`DeviceIo::model`]. Each read
    /// refreshes it, so another pad plugged into the same port is seen.
    model: Mutex<Option<&'static dyn Model>>,
}

impl HidrawDevice {
    /// A handle to the first attached supported controller. Nothing is opened until an
    /// operation is performed.
    #[must_use]
    pub const fn first() -> Self {
        Self { port: None, model: Mutex::new(None) }
    }

    /// A handle to the controller on USB port path `port`, such as `8-5`. Nothing
    /// is opened until an operation is performed.
    #[must_use]
    pub fn at(port: &str) -> Self {
        Self { port: Some(port.to_owned()), model: Mutex::new(None) }
    }

    fn target(&self) -> Target<'_> {
        Target { port: self.port.as_deref() }
    }

    /// Opens a read session and remembers the model it identified. A failed open
    /// forgets the model, so a pad that no longer answers as it is not taken for it.
    fn read_session(&self) -> Result<Session> {
        let opened = Session::open(self.target(), READ_TIMEOUT);
        let model = opened.as_ref().ok().map(Session::model).transpose()?;
        *self.model.lock().unwrap_or_else(PoisonError::into_inner) = model;
        opened
    }
}

// ---------------------------------------------------------------------------
// Profile upload helpers (`START_CONFIG` only, never `QUERY_STATUS`)
// ---------------------------------------------------------------------------

/// Sends `SLOT_SELECT`, then the upload loop. The session already sent `START_CONFIG`.
///
/// **Never sends `QUERY_STATUS`** — doing so kills joydev until reconnect.
fn read_blob(session: &mut Session, slot_select: u8) -> Result<Vec<u8>> {
    let _ = session.send_recv(&build_slot_select(slot_select))?;
    read_blob_chunks(session)
}

/// Runs the `PROFILE_UPLOAD` loop over the model's blob size and returns the assembled
/// blob. A Pro 3 blob takes 53 chunks.
fn read_blob_chunks(session: &mut Session) -> Result<Vec<u8>> {
    let blob_size = session.model()?.blob_size();
    let size_u16 = u16::try_from(blob_size)
        .map_err(|_| Error::Decode(format!("blob size {blob_size} overflows 16 bits")))?;
    let mut blob = Vec::with_capacity(blob_size);
    let mut offset = 0usize;
    while offset < blob_size {
        let chunk_size = (blob_size - offset).min(UPLOAD_CHUNK);
        let filler = vec![0xCCu8; chunk_size];
        #[allow(clippy::cast_possible_truncation)]
        let offset_u16 = offset as u16;
        #[allow(clippy::cast_possible_truncation)]
        let chunk_size_u16 = chunk_size as u16;
        let pkt = build_upload_packet(offset_u16, &filler, size_u16);
        let resp = session.send_recv(&pkt)?;
        // Validate the echoed offset/size against what we requested.
        let payload = decode_upload_response(&resp, offset_u16, chunk_size_u16)
            .inspect_err(|e| session.log_failure(&pkt, e))?;
        blob.extend_from_slice(&payload);
        offset += chunk_size;
    }
    if blob.len() != blob_size {
        return Err(Error::Decode(format!(
            "assembled profile blob is {} bytes, expected {blob_size}",
            blob.len()
        )));
    }
    Ok(blob)
}

/// Selects the bank of the current mode, so the session leaves the pad as a replug would.
///
/// A slot select moves the pad to that bank and to the slot its `cur_slot` names, and
/// the pad stays there until the next select or replug. The profile button then cycles
/// that bank's slots, so a pad left on another bank runs that bank's profiles.
fn select_current_bank(session: &mut Session) -> Result<()> {
    let value = session.model()?.slot_select_value(session.current_mode)?;
    let _ = session.send_recv(&build_slot_select(value))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Empty-slot placeholder
// ---------------------------------------------------------------------------

fn empty_summary(model: &dyn Model, mode: Mode, source_slot: u8) -> CanonicalProfileSummary {
    CanonicalProfileSummary {
        id: String::new(),
        name: String::new(),
        mode,
        source_slot,
        source_profile_index: source_slot.saturating_sub(1),
        canonical: CanonicalProfile { id: String::new(), ..model.default_profile(mode) },
    }
}

// ---------------------------------------------------------------------------
// `DeviceIo` impl
// ---------------------------------------------------------------------------

impl crate::transport::DeviceIo for HidrawDevice {
    fn model(&self) -> Result<&'static dyn Model> {
        let known = *self.model.lock().unwrap_or_else(PoisonError::into_inner);
        match known {
            Some(model) => Ok(model),
            None => self.read_session()?.model(),
        }
    }

    /// Reads every bank in the model's mode order in one session, from any current mode.
    ///
    /// The session ends with the current mode's bank selected, which leaves the pad
    /// on that bank's stored slot.
    ///
    /// **Never sends `QUERY_STATUS`** beyond the session's own pause.
    ///
    /// # Errors
    /// Returns [`Error::NoDevice`], [`Error::Usb`], [`Error::Timeout`],
    /// [`Error::Disconnected`] or [`Error::Decode`] on failure.
    fn read_all_profiles(&self) -> Result<ProfileReadResult> {
        // The session sends `START_CONFIG` once, to identify the model.
        let mut session = self.read_session()?;
        let model = session.model()?;
        let description = model.description()?;

        let mut profiles = Vec::new();
        let mut raw_blobs = Vec::new();

        for target_mode in description.modes.iter().map(|m| m.id) {
            let slot_select = model.slot_select_value(target_mode)?;
            let _ = session.send_recv(&build_slot_select(slot_select))?;
            let blob = read_blob_chunks(&mut session)?;

            for source_slot in 1..=description.slot_count {
                let slot = Slot::new(source_slot)?;
                if !model.slot_active(&blob, slot)? {
                    profiles.push(empty_summary(model, target_mode, source_slot));
                    continue;
                }
                let raw = RawProfilePayload {
                    payload: blob.clone(),
                    source_slot,
                    source_profile_index: source_slot - 1,
                    mode_hint: target_mode,
                };
                profiles.push(model.map_profile(&raw)?);
            }

            raw_blobs.push(blob);
        }

        select_current_bank(&mut session)?;
        Ok(ProfileReadResult { profiles, raw_blobs })
    }

    /// Reads a raw macro step stream from device flash.
    ///
    /// Primes the session with `START_CONFIG` → `QUERY_STATUS` → `SLOT_SELECT` →
    /// 53-chunk upload loop → `QUERY_STATUS` (the "app-style" read session required
    /// by firmware for macro readback), then reads `chunk_count` macro chunks.
    ///
    /// **Hardware gap:** the connected device has no macros and this path sends
    /// `QUERY_STATUS` (disrupting joydev). This method is implemented and unit-tested
    /// at the wire-builder level but is not covered by a hardware integration test
    /// in this task. See the task brief for details.
    ///
    /// # Errors
    /// Returns [`Error::NoDevice`], [`Error::Usb`], [`Error::Timeout`], or
    /// [`Error::Decode`] on failure.
    fn read_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        step_count: usize,
    ) -> Result<Vec<u8>> {
        if step_count == 0 {
            return Ok(Vec::new());
        }

        // Wire protocol uses 0-based profile slot.
        let ps = profile_slot.get() - 1;
        let macro_slot_idx = macro_slot.get();

        #[allow(clippy::cast_possible_truncation)]
        let total_len = (step_count * 10) as u16;
        let data_bytes = step_count * 10;
        let flash_base = u16::from(macro_slot_idx) * MACRO_ERASE_LEN;
        let chunk_count = data_bytes.div_ceil(usize::from(MACRO_CHUNK));

        // Prime: `START_CONFIG` (sent by the session) → `QUERY_STATUS` → `SLOT_SELECT`
        // → upload×53 → `QUERY_STATUS`
        let mut session = self.read_session()?;
        let macro_gamepad_mode = session.model()?.macro_gamepad_mode(mode)?;
        let slot_select = session.model()?.slot_select_value(mode)?;
        let _ = session.send_recv(&build_query_status())?;
        let _ = session.send_recv(&build_slot_select(slot_select))?;
        let _ = read_blob_chunks(&mut session)?;
        let _ = session.send_recv(&build_query_status())?;

        let mut result = Vec::with_capacity(data_bytes);
        for chunk_idx in 0..chunk_count {
            #[allow(clippy::cast_possible_truncation)]
            let chunk_offset = flash_base + (chunk_idx as u16) * MACRO_CHUNK;
            #[allow(clippy::cast_possible_truncation)]
            let chunk_size = (data_bytes - chunk_idx * usize::from(MACRO_CHUNK))
                .min(usize::from(MACRO_CHUNK)) as u16;
            let pkt = build_read_macro_packet(
                ps,
                macro_gamepad_mode,
                chunk_offset,
                total_len,
                chunk_size,
            );
            let resp = session.send_recv(&pkt)?;
            let chunk = decode_read_macro_response(&resp, chunk_offset, total_len)?;
            result.extend_from_slice(&chunk);
        }

        select_current_bank(&mut session)?;
        result.truncate(data_bytes);
        Ok(result)
    }

    /// Probes device readiness: sysfs scan followed by a live active-slot check.
    ///
    /// Reads the full profile blob using `START_CONFIG` only (never `QUERY_STATUS`)
    /// and checks each of the model's slots for a profile. `active_slot_marker` is set to
    /// a comma-joined list of active slot numbers (e.g. `"1,3"`), or `"unknown"`
    /// if no slot is active or the probe fails.
    ///
    /// # Errors
    /// Never returns an error; a missing device or probe failure is reflected in
    /// the returned [`DeviceReadiness`] struct.
    fn detect_readiness(&self) -> Result<DeviceReadiness> {
        let Some(found) = scan_sysfs(Path::new("/sys/bus/usb/devices"), &config_ports()) else {
            return Ok(DeviceReadiness {
                message: "No supported controller detected. Connect it via USB, \
                          then re-run detect."
                    .to_owned(),
                ..DeviceReadiness::default()
            });
        };

        let mut readiness = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(found.port.mode),
            active_slot_marker: "unknown".to_owned(),
            vendor_id: found.vendor_id,
            product_id: found.product_id.clone(),
            sysfs_path: found.sysfs_path,
            message: "Supported controller detected.".to_owned(),
            ..DeviceReadiness::default()
        };

        let mut session = match self.read_session() {
            Err(e @ Error::UnsupportedModel(_)) => {
                readiness.supported_device_connected = false;
                readiness.message = format!("Controller found, but not a supported model: {e}.");
                return Ok(readiness);
            }
            Err(e) => {
                readiness.message =
                    format!("Supported controller detected. Active slot marker unavailable: {e}");
                return Ok(readiness);
            }
            Ok(session) => session,
        };
        let model = session.model()?;
        let description = model.description()?;
        let name = &description.display_name;
        readiness.firmware_version.clone_from(&session.firmware_version);
        let blob = match read_blob(&mut session, model.slot_select_value(found.port.mode)?) {
            Ok(blob) => blob,
            Err(e) => {
                readiness.message =
                    format!("Supported {name} detected. Active slot marker unavailable: {e}");
                return Ok(readiness);
            }
        };
        // Report the LOWEST active slot as a single digit; the probe is "verified" only
        // when such a marker is found.
        let lowest = (1..=description.slot_count).find(|&s| {
            Slot::new(s).is_ok_and(|slot| model.slot_active(&blob, slot).unwrap_or(false))
        });
        if let Some(s) = lowest {
            readiness.active_slot_marker = s.to_string();
            readiness.active_slot_marker_verified = true;
            readiness.message =
                format!("Supported {name} detected and active slot marker verified.");
        } else {
            readiness.message = format!(
                "Supported {name} detected. Active slot marker unavailable: \
                 no recognizable slot marker."
            );
        }
        Ok(readiness)
    }

    fn begin_write(&self) -> Result<Option<Mode>> {
        hidraw_write::begin_write(self.target())
    }

    fn end_write(&self, back_to: Mode) -> Result<()> {
        hidraw_write::end_write(self.target(), back_to)
    }

    fn write_full_profile(&self, _mode: Mode, blob: &[u8]) -> Result<()> {
        hidraw_write::write_full_profile(self.target(), blob)
    }

    fn write_patch(&self, _mode: Mode, offset: u16, data: &[u8]) -> Result<()> {
        hidraw_write::write_patch(self.target(), offset, data)
    }

    fn send_slot_select(&self, mode: Mode) -> Result<()> {
        hidraw_write::send_slot_select(self.target(), mode)
    }

    fn send_apply(&self, _mode: Mode) -> Result<()> {
        hidraw_write::send_apply(self.target())
    }

    fn query_status(&self, _mode: Mode) -> Result<()> {
        hidraw_write::query_status(self.target())
    }

    fn erase_macro(&self, mode: Mode, profile_slot: Slot, macro_slot: MacroSlot) -> Result<()> {
        hidraw_write::erase_macro(self.target(), mode, profile_slot, macro_slot)
    }

    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()> {
        hidraw_write::write_macro_stream(self.target(), mode, profile_slot, macro_slot, stream)
    }
}
