//! Device I/O abstraction (`DeviceIo`) with real and mock implementations.
pub mod device_io;
pub mod mock;
pub mod nusb_device;

pub use device_io::DeviceIo;
pub use mock::MockDevice;
pub use nusb_device::NusbDevice;
