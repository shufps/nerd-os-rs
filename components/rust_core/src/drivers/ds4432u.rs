//! DS4432U current DAC that trims the TPS40305 core voltage of the NerdAxe
//! (port of `main/boards/drivers/nerdaxe/DS4432U.cpp`): one code register
//! per output. Verified against
//! the C++ driver by `tests/ina260_ds4432u.rs`.
//!
//! `DS4432U_test()` logs the register value only when the read succeeds.

use core::ffi::CStr;

use crate::drivers::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"DS4432U";
pub const ADDR: u8 = 0x48;
const REG_OUT0: u8 = 0xf8;
const REG_OUT1: u8 = 0xf9;

fn reg(output: u8) -> u8 {
    if output == 0 {
        REG_OUT0
    } else {
        REG_OUT1
    }
}

pub fn set_code(bus: &mut impl RegBus, output: u8, code: u8) -> bool {
    bus.write(ADDR, reg(output), code)
}

pub fn code(bus: &mut impl RegBus, output: u8) -> Option<u8> {
    bus.read(ADDR, reg(output))
}

/// Reads OUT0 (0x00 after power-up).
pub fn test(bus: &mut impl RegBus) -> bool {
    match bus.read(ADDR, REG_OUT0) {
        Some(v) => {
            log!(Info, TAG, "DS4432U+ OUT0 = 0x{v:02X}");
            true
        }
        None => {
            log!(Error, TAG, "DS4432U+ not responding");
            false
        }
    }
}
