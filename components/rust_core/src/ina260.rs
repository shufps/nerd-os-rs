//! INA260 current/voltage/power monitor of the NerdAxe (port of
//! `main/boards/drivers/nerdaxe/INA260.cpp`): 16-bit big-endian registers,
//! 1.25 mA, 1.25 mV and 10 mW per LSB. Verified against the C++ driver by
//! `tests/ina260_ds4432u.rs`.
//!
//! The current register is signed (two's complement), so a slightly
//! negative reading (offset at idle) stays a small negative current.

use core::ffi::CStr;

use crate::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"INA260";
pub const ADDR: u8 = 0x40;
const REG_CURRENT: u8 = 0x01;
const REG_BUSVOLTAGE: u8 = 0x02;
const REG_POWER: u8 = 0x03;

fn read16(bus: &mut impl RegBus, reg: u8, what: &str) -> Option<u16> {
    let mut b = [0u8; 2];
    if !bus.read_into(ADDR, reg, &mut b) {
        log!(Error, TAG, "error reading {what}");
        return None;
    }
    Some(u16::from_be_bytes(b))
}

pub fn installed(bus: &mut impl RegBus) -> bool {
    let mut b = [0u8; 2];
    bus.read_into(ADDR, REG_BUSVOLTAGE, &mut b)
}

/// mA; 0 on a bus error.
pub fn current(bus: &mut impl RegBus) -> f32 {
    read16(bus, REG_CURRENT, "current").map_or(0.0, |v| (v as i16 as f64 * 1.25) as f32)
}

/// mV; 0 on a bus error.
pub fn voltage(bus: &mut impl RegBus) -> f32 {
    read16(bus, REG_BUSVOLTAGE, "voltage").map_or(0.0, |v| (v as f64 * 1.25) as f32)
}

/// mW; 0 on a bus error.
pub fn power(bus: &mut impl RegBus) -> f32 {
    read16(bus, REG_POWER, "power").map_or(0.0, |v| (v as u32 * 10) as f32)
}
