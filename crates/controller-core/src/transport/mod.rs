//! Device I/O abstraction (`DeviceIo`) with real and mock implementations.
pub mod device_io;
pub mod mock;

pub use device_io::DeviceIo;
pub use mock::MockDevice;
