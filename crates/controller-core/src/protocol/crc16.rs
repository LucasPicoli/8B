//! CRC-16/MODBUS (poly 0xA001 reflected, init 0xFFFF). Check("123456789") = 0x4B37.

/// Computes the CRC-16/MODBUS checksum of `data`.
#[must_use]
pub fn crc16_modbus(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &byte in data {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            let lsb = crc & 1;
            crc >>= 1;
            if lsb != 0 {
                crc ^= 0xA001;
            }
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn modbus_check_value() {
        assert_eq!(crc16_modbus(b"123456789"), 0x4B37);
    }
}
