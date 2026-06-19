//! Real USB transport over nusb (interrupt transfers).

use std::path::Path;
use std::time::Duration;

use nusb::transfer::{Buffer, In, Interrupt, Out};
use nusb::MaybeFuture as _;

use crate::detect::{is_slot_active, scan_sysfs};
use crate::device::{ControllerSpec as _, ProtocolCodec as _};
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::{
    ButtonMapping, CanonicalProfile, CanonicalProfileSummary, DeviceReadiness, MacroRef, MacroSlot,
    Mode, ProfileReadResult, RawProfilePayload, Slot, Sticks, Triggers, TriggersAnalog, Vibration,
};
use crate::protocol::wire::{
    build_query_status, build_read_macro_packet, build_slot_select, build_start_config,
    build_upload_packet, decode_read_macro_response, decode_upload_response,
};

const VENDOR_ID: u16 = 0x2DC8;
const TIMEOUT: Duration = Duration::from_millis(1000);
const PROFILE_SIZE: usize = 0x092C;
const UPLOAD_CHUNK: usize = 45;
const MACRO_CHUNK: u16 = 32;
const MACRO_ERASE_LEN: u16 = 0x1000;

/// Real USB transport backed by nusb interrupt transfers.
pub struct NusbDevice {
    spec: Pro3,
}

impl NusbDevice {
    /// Opens a handle to the first attached 8BitDo Pro 3.
    ///
    /// The device is not claimed until an operation is performed — this
    /// constructor is infallible and just stores the spec.
    ///
    /// # Errors
    /// Never returns an error; signature matches trait expectations.
    pub const fn open() -> Result<Self> {
        Ok(Self { spec: Pro3 })
    }
}

// ---------------------------------------------------------------------------
// Low-level USB helpers
// ---------------------------------------------------------------------------

struct Session {
    ep_out: nusb::Endpoint<Interrupt, Out>,
    ep_in: nusb::Endpoint<Interrupt, In>,
}

impl Session {
    fn for_product(product: u16, interface: u8, ep_out_addr: u8, ep_in_addr: u8) -> Result<Self> {
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| Error::Usb(e.to_string()))?
            .find(|d| d.vendor_id() == VENDOR_ID && d.product_id() == product)
            .ok_or(Error::NoDevice)?;
        let device = info.open().wait().map_err(|e| Error::Usb(e.to_string()))?;
        let iface = device
            .detach_and_claim_interface(interface)
            .wait()
            .map_err(|e| Error::Usb(e.to_string()))?;
        let out =
            iface.endpoint::<Interrupt, Out>(ep_out_addr).map_err(|e| Error::Usb(e.to_string()))?;
        let inp =
            iface.endpoint::<Interrupt, In>(ep_in_addr).map_err(|e| Error::Usb(e.to_string()))?;
        Ok(Self { ep_out: out, ep_in: inp })
    }

    fn send_recv(&mut self, pkt: &[u8; 64]) -> Result<Vec<u8>> {
        let c = self.ep_out.transfer_blocking(pkt.to_vec().into(), TIMEOUT);
        c.status.map_err(|_| Error::Timeout)?;
        let c = self.ep_in.transfer_blocking(Buffer::new(64), TIMEOUT);
        c.status.map_err(|_| Error::Timeout)?;
        Ok(c.buffer.get(..c.actual_len).unwrap_or(&c.buffer).to_vec())
    }
}

// ---------------------------------------------------------------------------
// Profile upload helpers (`START_CONFIG` only, never `QUERY_STATUS`)
// ---------------------------------------------------------------------------

/// Sends `START_CONFIG`, `SLOT_SELECT`, then the 53-chunk upload loop.
///
/// **Never sends `QUERY_STATUS`** — doing so kills joydev until reconnect.
fn read_blob(session: &mut Session, slot_select: u8, payload_offset: usize) -> Result<Vec<u8>> {
    let _ = session.send_recv(&build_start_config())?;
    let _ = session.send_recv(&build_slot_select(slot_select))?;
    read_blob_chunks(session, payload_offset)
}

/// Runs the 53-chunk `PROFILE_UPLOAD` loop and returns the assembled blob.
fn read_blob_chunks(session: &mut Session, payload_offset: usize) -> Result<Vec<u8>> {
    let mut blob = Vec::with_capacity(PROFILE_SIZE);
    let mut offset = 0usize;
    while offset < PROFILE_SIZE {
        let chunk_size = (PROFILE_SIZE - offset).min(UPLOAD_CHUNK);
        let filler = vec![0xCCu8; chunk_size];
        #[allow(clippy::cast_possible_truncation)]
        let offset_u16 = offset as u16;
        #[allow(clippy::cast_possible_truncation)]
        let chunk_size_u16 = chunk_size as u16;
        let pkt = build_upload_packet(offset_u16, &filler, payload_offset);
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

impl crate::transport::DeviceIo for NusbDevice {
    /// Reads all on-device profiles, dispatching on product id (not mode).
    ///
    /// Product `0x310B` (`XInput` or `Switch`): reads `XInput` (`slot_select=3`) then
    /// `Switch` (`slot_select=0`), returning 2 blobs. Product `0x6009` (`DInput`):
    /// reads `DInput` (`slot_select=1`), returning 1 blob.
    ///
    /// **Never sends `QUERY_STATUS`** — that permanently kills joydev until reconnect.
    ///
    /// # Errors
    /// Returns [`Error::NoDevice`], [`Error::Usb`], [`Error::Timeout`], or
    /// [`Error::Decode`] on failure.
    fn read_all_profiles(&self, mode: Mode) -> Result<ProfileReadResult> {
        let product = self.spec.product_id_for_mode(mode);
        let tp = self.spec.transport_params(mode);

        // Targets: (mode, slot_select_value). Dispatch on product, not mode:
        // 0x310B → xinput+switch blobs; 0x6009 → dinput only.
        let targets: &[(Mode, u8)] = match product {
            0x310B => &[(Mode::XInput, 3), (Mode::Switch, 0)],
            0x6009 => &[(Mode::DInput, 1)],
            _ => return Err(Error::Usb(format!("unsupported product id 0x{product:04X}"))),
        };

        let mut session = Session::for_product(product, tp.interface, tp.ep_out, tp.ep_in)?;

        // Send `START_CONFIG` once for the whole session.
        let _ = session.send_recv(&build_start_config())?;

        let mut profiles = Vec::new();
        let mut raw_blobs = Vec::new();

        for &(target_mode, slot_select) in targets {
            let _ = session.send_recv(&build_slot_select(slot_select))?;
            let blob = read_blob_chunks(&mut session, tp.payload_offset)?;

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

        let product = self.spec.product_id_for_mode(mode);
        let tp = self.spec.transport_params(mode);

        let (macro_gamepad_mode, slot_select): (u8, u8) = match mode {
            Mode::XInput => (0x03, 3),
            Mode::Switch => (0x00, 0),
            Mode::DInput => (0x01, 1),
        };

        // Wire protocol uses 0-based profile slot.
        let ps = profile_slot.get() - 1;
        let macro_slot_idx = macro_slot.get();

        #[allow(clippy::cast_possible_truncation)]
        let total_len = (step_count * 10) as u16;
        let data_bytes = step_count * 10;
        let flash_base = u16::from(macro_slot_idx) * MACRO_ERASE_LEN;
        let chunk_count = data_bytes.div_ceil(usize::from(MACRO_CHUNK));

        let mut session = Session::for_product(product, tp.interface, tp.ep_out, tp.ep_in)?;

        // Prime: `START_CONFIG` → `QUERY_STATUS` → `SLOT_SELECT` → upload×53 → `QUERY_STATUS`
        let _ = session.send_recv(&build_start_config())?;
        let _ = session.send_recv(&build_query_status())?;
        let _ = session.send_recv(&build_slot_select(slot_select))?;
        let _ = read_blob_chunks(&mut session, tp.payload_offset)?;
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
        let Some(found) = scan_sysfs(Path::new("/sys/bus/usb/devices")) else {
            return Ok(DeviceReadiness {
                message: "No supported 8BitDo Pro 3 detected. Connect via USB in \
                          XInput or DInput mode, then re-run detect."
                    .to_owned(),
                ..DeviceReadiness::default()
            });
        };

        let mut readiness = DeviceReadiness {
            supported_device_connected: true,
            mode: Some(found.mode),
            active_slot_marker: "unknown".to_owned(),
            vendor_id: found.vendor_id,
            product_id: found.product_id.clone(),
            sysfs_path: found.sysfs_path,
            message: "Supported 8BitDo Pro 3 detected.".to_owned(),
            ..DeviceReadiness::default()
        };

        let tp = self.spec.transport_params(found.mode);
        let product = self.spec.product_id_for_mode(found.mode);
        let slot_select: u8 = match found.mode {
            Mode::XInput => 3,
            Mode::Switch => 0,
            Mode::DInput => 1,
        };

        match Session::for_product(product, tp.interface, tp.ep_out, tp.ep_in) {
            Err(e) => {
                readiness.message =
                    format!("Supported 8BitDo Pro 3 detected. Active slot marker unavailable: {e}");
            }
            Ok(mut session) => match read_blob(&mut session, slot_select, tp.payload_offset) {
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
            },
        }

        Ok(readiness)
    }
}
