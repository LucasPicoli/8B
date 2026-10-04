//! Device I/O abstraction (`DeviceIo`) with real and mock implementations.
pub mod device_io;
pub mod hidraw_device;
mod hidraw_write;
pub mod mock;
mod session;
pub mod write_input;

pub use device_io::DeviceIo;
pub use hidraw_device::HidrawDevice;
pub use mock::MockDevice;
