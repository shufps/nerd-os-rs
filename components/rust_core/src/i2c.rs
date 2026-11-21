//! Register access on the board's I2C bus for the sensor and fan drivers:
//! a trait (fakes in the tests) and the firmware's bus (`i2c_master.h`,
//! port 0).

/// Byte registers of 7-bit addressed devices.
pub trait RegBus {
    /// One register; None on a bus error.
    fn read(&mut self, addr: u8, reg: u8) -> Option<u8>;
    /// Writes one register; false on a bus error.
    fn write(&mut self, addr: u8, reg: u8, value: u8) -> bool;
    /// `len` registers from `reg` on (the device auto-increments).
    fn read_into(&mut self, addr: u8, reg: u8, out: &mut [u8]) -> bool {
        for (i, b) in out.iter_mut().enumerate() {
            match self.read(addr, reg.wrapping_add(i as u8)) {
                Some(v) => *b = v,
                None => return false,
            }
        }
        true
    }
}

/// The firmware's I2C master (`crate::idf::i2c`); on the host every access
/// fails.
pub struct Master;

impl RegBus for Master {
    fn read(&mut self, addr: u8, reg: u8) -> Option<u8> {
        let mut v = [0u8];
        self.read_into(addr, reg, &mut v).then_some(v[0])
    }

    fn write(&mut self, _addr: u8, _reg: u8, _value: u8) -> bool {
        #[cfg(target_os = "espidf")]
        return crate::idf::i2c::write_to_device(_addr, &[_reg, _value]) == 0;
        #[cfg(not(target_os = "espidf"))]
        false
    }

    /// One transaction (register address, then `out.len()` bytes).
    fn read_into(&mut self, _addr: u8, _reg: u8, _out: &mut [u8]) -> bool {
        #[cfg(target_os = "espidf")]
        return crate::idf::i2c::register_read(_addr, _reg, _out) == 0;
        #[cfg(not(target_os = "espidf"))]
        false
    }
}
