//! One claimed USB interface with a blocking send/receive primitive.

use std::time::Duration;

use nusb::transfer::{Buffer, In, Interrupt, Out};
use nusb::MaybeFuture as _;

use crate::device::{ControllerSpec as _, TransportParams};
use crate::devices::pro3::Pro3;
use crate::error::{Error, Result};
use crate::model::Mode;
use crate::protocol::wire_write::PACKET_LEN;

const VENDOR_ID: u16 = 0x2DC8;
/// Per-transfer timeout for the read path.
pub(super) const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// Per-transfer timeout for the write path (the C++ default).
pub(super) const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

/// A claimed interface. Dropping it releases the interface.
pub(super) struct Session {
    ep_out: nusb::Endpoint<Interrupt, Out>,
    ep_in: nusb::Endpoint<Interrupt, In>,
    timeout: Duration,
    /// Transport parameters of the mode this session was opened for.
    pub(super) params: TransportParams,
}

impl Session {
    /// Opens the device for `mode`'s product and claims the mode's interface.
    pub(super) fn for_mode(spec: Pro3, mode: Mode, timeout: Duration) -> Result<Self> {
        let product = spec.product_id_for_mode(mode);
        let params = spec.transport_params(mode);
        let info = nusb::list_devices()
            .wait()
            .map_err(|e| Error::Usb(e.to_string()))?
            .find(|d| d.vendor_id() == VENDOR_ID && d.product_id() == product)
            .ok_or(Error::NoDevice)?;
        let device = info.open().wait().map_err(|e| Error::Usb(e.to_string()))?;
        let iface = device
            .detach_and_claim_interface(params.interface)
            .wait()
            .map_err(|e| Error::Usb(e.to_string()))?;
        let ep_out = iface
            .endpoint::<Interrupt, Out>(params.ep_out)
            .map_err(|e| Error::Usb(e.to_string()))?;
        let ep_in =
            iface.endpoint::<Interrupt, In>(params.ep_in).map_err(|e| Error::Usb(e.to_string()))?;
        Ok(Self { ep_out, ep_in, timeout, params })
    }

    /// Sends one packet and returns the response bytes.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if either transfer fails.
    pub(super) fn send_recv(&mut self, pkt: &[u8; PACKET_LEN]) -> Result<Vec<u8>> {
        let c = self.ep_out.transfer_blocking(pkt.to_vec().into(), self.timeout);
        c.status.map_err(|_| Error::Timeout)?;
        let c = self.ep_in.transfer_blocking(Buffer::new(PACKET_LEN), self.timeout);
        c.status.map_err(|_| Error::Timeout)?;
        Ok(c.buffer.get(..c.actual_len).unwrap_or(&c.buffer).to_vec())
    }
}
