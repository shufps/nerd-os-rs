//! DS4432U current DAC that trims the TPS40305 core voltage of the NerdAxe
//! (port of `main/boards/drivers/nerdaxe/DS4432U.cpp`): one code register
//! per output. C API: the former `DS4432U.h` functions. Verified against
//! the C++ driver by `tests/ina260_ds4432u.rs`.
//!
//! `DS4432U_test()` logs the register value only when the read succeeds.

use core::ffi::{c_int, CStr};

use crate::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"DS4432U";
pub const ADDR: u8 = 0x48;
const REG_OUT0: u8 = 0xf8;
const REG_OUT1: u8 = 0xf9;

const ESP_OK: c_int = 0;
const ESP_FAIL: c_int = -1;

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

pub(crate) mod ffi {
    use super::*;
    use crate::i2c::Master;

    #[no_mangle]
    pub extern "C" fn DS4432U_test() -> bool {
        test(&mut Master)
    }

    #[no_mangle]
    pub extern "C" fn DS4432U_set_current_code(output: u8, code: u8) -> c_int {
        if set_code(&mut Master, output, code) {
            ESP_OK
        } else {
            ESP_FAIL
        }
    }

    /// # Safety
    /// `out` writable.
    #[no_mangle]
    pub unsafe extern "C" fn DS4432U_get_current_code(output: u8, out: *mut u8) -> c_int {
        match code(&mut Master, output) {
            Some(v) => {
                *out = v;
                ESP_OK
            }
            None => ESP_FAIL,
        }
    }
}
