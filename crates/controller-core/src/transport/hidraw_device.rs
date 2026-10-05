//! Real transport over the hidraw node of the config interface.

use std::path::Path;

use crate::detect::{is_slot_active, scan_sysfs};
use crate::device::{ControllerSpec as _, ProtocolCodec as _};
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, DeviceReadiness, MacroRef, MacroSlot,
    Mode, ProfileReadResult, RawProfilePayload, Slot, Sticks, Triggers, TriggersAnalog, Vibration,
};
use crate::protocol::wire::{
    build_query_status, build_read_macro_packet, build_slot_select, build_upload_packet,
    decode_read_macro_response, decode_upload_response,
};
use crate::protocol::wire_write::MACRO_PAGE_LEN;
use crate::transport::hidraw_write;
use crate::transport::session::{Session, READ_TIMEOUT};
use crate::transport::write_input::{PROFILE_CHUNK as UPLOAD_CHUNK, PROFILE_SIZE};

const MACRO_CHUNK: u16 = 32;
const MACRO_ERASE_LEN: u16 = MACRO_PAGE_LEN;

/// Real transport over hidraw. Works in every current mode; the USB id of the
/// attached controller picks the node and the framing.
pub struct HidrawDevice {
    spec: Pro3,
}

impl HidrawDevice {
    /// Opens a handle to the first attached 8BitDo Pro 3.
    ///
    /// The node is not opened until an operation is performed. This constructor
    /// is infallible and just stores the spec.
    ///
    /// # Errors
    /// Never returns an error; signature matches trait expectations.
    pub const fn open() -> Result<Self> {
        Ok(Self { spec: Pro3 })
    }
}

// ---------------------------------------------------------------------------
// Profile upload helpers (`START_CONFIG` only, never `QUERY_STATUS`)
// ---------------------------------------------------------------------------

/// Sends `SLOT_SELECT`, then the 53-chunk upload loop. The session already sent
/// `START_CONFIG`.
///
/// **Never sends `QUERY_STATUS`** — doing so kills joydev until reconnect.
fn read_blob(session: &mut Session, slot_select: u8) -> Result<Vec<u8>> {
    let _ = session.send_recv(&build_slot_select(slot_select))?;
    read_blob_chunks(session)
}

/// Runs the 53-chunk `PROFILE_UPLOAD` loop and returns the assembled blob.
fn read_blob_chunks(session: &mut Session) -> Result<Vec<u8>> {
    let mut blob = Vec::with_capacity(PROFILE_SIZE);
    let mut offset = 0usize;
    while offset < PROFILE_SIZE {
        let chunk_size = (PROFILE_SIZE - offset).min(UPLOAD_CHUNK);
        let filler = vec![0xCCu8; chunk_size];
        #[allow(clippy::cast_possible_truncation)]
        let offset_u16 = offset as u16;
        #[allow(clippy::cast_possible_truncation)]
        let chunk_size_u16 = chunk_size as u16;
        let pkt = build_upload_packet(offset_u16, &filler);
        let resp = session.send_recv(&pkt)?;
        // Validate the echoed offset/size against what we requested (matches C++).
        let payload = decode_upload_response(&resp, offset_u16, chunk_size_u16)?;
        blob.extend_from_slice(&payload);
        offset += chunk_size;
    }
    if blob.len() != PROFILE_SIZE {
        return Err(Error::Decode(format!(
            "assembled profile blob is {} bytes, expected {PROFILE_SIZE}",
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
fn select_current_bank(session: &mut Session, spec: Pro3) -> Result<()> {
    let _ = session.send_recv(&build_slot_select(spec.slot_select_value(session.current_mode)))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Empty-slot placeholder (mirrors C++ default-constructed `CanonicalProfileSummary`)
// ---------------------------------------------------------------------------

fn empty_summary(mode: Mode, source_slot: u8) -> CanonicalProfileSummary {
    CanonicalProfileSummary {
        id: String::new(),
        name: String::new(),
        mode,
        source_slot,
        source_profile_index: source_slot.saturating_sub(1),
        canonical: CanonicalProfile {
            id: String::new(),
            name: String::new(),
            version: 1,
            kind: "8bitdo.pro3.profile".to_owned(),
            device: "8bitdo-pro3".to_owned(),
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
            vibration: Vibration { left_level: 0, right_level: 0 },
            button_mappings: Vec::<ButtonMapping>::new(),
            macro_refs: Vec::<MacroRef>::new(),
        },
    }
}

// ---------------------------------------------------------------------------
// `DeviceIo` impl
// ---------------------------------------------------------------------------

impl crate::transport::DeviceIo for HidrawDevice {
    /// Reads every bank in [`Mode::ALL`] order in one session, from any current mode.
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
        let mut session = Session::open(self.spec, READ_TIMEOUT)?;

        let mut profiles = Vec::new();
        let mut raw_blobs = Vec::new();

        for target_mode in Mode::ALL {
            let slot_select = self.spec.slot_select_value(target_mode);
            let _ = session.send_recv(&build_slot_select(slot_select))?;
            let blob = read_blob_chunks(&mut session)?;

            for source_slot in 1u8..=3 {
                let slot = Slot::new(source_slot)?;
                if !is_slot_active(&blob, slot)? {
                    profiles.push(empty_summary(target_mode, source_slot));
                    continue;
                }
                let raw = RawProfilePayload {
                    payload: blob.clone(),
                    source_slot,
                    source_profile_index: source_slot - 1,
                    mode_hint: target_mode,
                };
                profiles.push(self.spec.map_profile(&raw)?);
            }

            raw_blobs.push(blob);
        }

        select_current_bank(&mut session, self.spec)?;
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

        let macro_gamepad_mode = self.spec.macro_gamepad_mode(mode);
        let slot_select = self.spec.slot_select_value(mode);

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
        let mut session = Session::open(self.spec, READ_TIMEOUT)?;
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

        select_current_bank(&mut session, self.spec)?;
        result.truncate(data_bytes);
        Ok(result)
    }

    /// Probes device readiness: sysfs scan followed by a live active-slot check.
    ///
    /// Reads the full profile blob using `START_CONFIG` only (never `QUERY_STATUS`)
    /// and calls [`is_slot_active`] for slots 1–3. `active_slot_marker` is set to
    /// a comma-joined list of active slot numbers (e.g. `"1,3"`), or `"unknown"`
    /// if no slot is active or the probe fails.
    ///
    /// # Errors
    /// Never returns an error; a missing device or probe failure is reflected in
    /// the returned [`DeviceReadiness`] struct.
    fn detect_readiness(&self) -> Result<DeviceReadiness> {
        let ports = self.spec.description().map(|d| d.config_ports.as_slice()).unwrap_or_default();
        let Some(found) = scan_sysfs(Path::new("/sys/bus/usb/devices"), ports) else {
            return Ok(DeviceReadiness {
                message: "No supported 8BitDo Pro 3 detected. Connect it via USB, \
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
            message: "Supported 8BitDo Pro 3 detected.".to_owned(),
            ..DeviceReadiness::default()
        };

        let slot_select = self.spec.slot_select_value(found.port.mode);

        match Session::open(self.spec, READ_TIMEOUT) {
            Err(e @ Error::UnsupportedModel(_)) => {
                readiness.supported_device_connected = false;
                readiness.message = format!("8BitDo controller found, but not a Pro 3: {e}.");
            }
            Err(e) => {
                readiness.message =
                    format!("Supported 8BitDo Pro 3 detected. Active slot marker unavailable: {e}");
            }
            Ok(mut session) => {
                match read_blob(&mut session, slot_select) {
                    Err(e) => {
                        readiness.message = format!(
                            "Supported 8BitDo Pro 3 detected. Active slot marker unavailable: {e}"
                        );
                    }
                    Ok(blob) => {
                        // C++ `decodeSlotMarkerFromUploadResponse` reports the LOWEST active
                        // slot as a single digit ("1"/"2"/"3"), and the probe is "verified"
                        // only when such a marker is found. Match that exactly.
                        let mut found: Option<u8> = None;
                        for s in 1u8..=3 {
                            if let Ok(slot) = Slot::new(s) {
                                if is_slot_active(&blob, slot).unwrap_or(false) {
                                    found = Some(s);
                                    break;
                                }
                            }
                        }
                        if let Some(s) = found {
                            readiness.active_slot_marker = s.to_string();
                            readiness.active_slot_marker_verified = true;
                            "Supported 8BitDo Pro 3 detected and active slot marker verified."
                                .clone_into(&mut readiness.message);
                        } else {
                            "Supported 8BitDo Pro 3 detected. Active slot marker unavailable: \
                         no recognizable slot marker."
                                .clone_into(&mut readiness.message);
                        }
                    }
                }
            }
        }

        Ok(readiness)
    }

    fn begin_write(&self) -> Result<Option<Mode>> {
        hidraw_write::begin_write(self.spec)
    }

    fn end_write(&self, back_to: Mode) -> Result<()> {
        hidraw_write::end_write(self.spec, back_to)
    }

    fn write_full_profile(&self, _mode: Mode, blob: &[u8]) -> Result<()> {
        hidraw_write::write_full_profile(self.spec, blob)
    }

    fn write_patch(&self, _mode: Mode, offset: u16, data: &[u8]) -> Result<()> {
        hidraw_write::write_patch(self.spec, offset, data)
    }

    fn send_slot_select(&self, mode: Mode) -> Result<()> {
        hidraw_write::send_slot_select(self.spec, mode)
    }

    fn send_apply(&self, _mode: Mode) -> Result<()> {
        hidraw_write::send_apply(self.spec)
    }

    fn query_status(&self, _mode: Mode) -> Result<()> {
        hidraw_write::query_status(self.spec)
    }

    fn erase_macro(&self, mode: Mode, profile_slot: Slot, macro_slot: MacroSlot) -> Result<()> {
        hidraw_write::erase_macro(self.spec, mode, profile_slot, macro_slot)
    }

    fn write_macro_stream(
        &self,
        mode: Mode,
        profile_slot: Slot,
        macro_slot: MacroSlot,
        stream: &[u8],
    ) -> Result<()> {
        hidraw_write::write_macro_stream(self.spec, mode, profile_slot, macro_slot, stream)
    }
}
