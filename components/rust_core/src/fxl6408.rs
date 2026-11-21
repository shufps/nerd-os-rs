//! FXL6408 8-bit I2C GPIO expander (port of `main/boards/drivers/fxl6408.cpp`):
//! direction, output levels and pull-ups are kept as shadow registers and
//! written whole. On the Q1370/Q1373 it switches the VREG and LDO enables
//! and the ASIC reset, and selects the TMP451 multiplexer channel.
//! Verified against the C++ driver by
//! `tests/sensors.rs`.
//!
//! Every read-modify-write of a shadow register happens under the
//! expander's lock: the expander is shared by the power sequencing
//! (VREG/LDO/reset pins) and the temperature multiplexer selection, and an
//! update must not write back a stale output register.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;
use std::time::Duration;

use crate::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"fxl6408";
pub const ADDR: u8 = 0x43;
const REG_CTRL: u8 = 0x01;
const REG_DIRECTION: u8 = 0x03;
const REG_OUTPUT_STATE: u8 = 0x05;
const REG_OUTPUT_HZ: u8 = 0x07;
const REG_PULL_ENABLE: u8 = 0x0b;
const REG_PULL_SELECT: u8 = 0x0d;
const REG_INPUT_STATUS: u8 = 0x0f;

pub const ESP_OK: c_int = 0;
pub const ESP_FAIL: c_int = -1;
pub const ESP_ERR_INVALID_ARG: c_int = 0x102;

#[derive(Default)]
struct Shadow {
    /// 1 = output
    direction: u8,
    output: u8,
    pull_enable: u8,
    /// 1 = pull-up
    pull_select: u8,
}

#[derive(Default)]
pub struct Fxl6408 {
    s: Mutex<Shadow>,
}

fn err(ok: bool) -> c_int {
    if ok {
        ESP_OK
    } else {
        ESP_FAIL
    }
}

impl Fxl6408 {
    fn with<T>(&self, f: impl FnOnce(&mut Shadow) -> T) -> T {
        f(&mut self.s.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Checks the device, software reset, outputs out of high-Z.
    pub fn init(&self, bus: &mut impl RegBus, sleep: &mut impl FnMut(Duration)) -> bool {
        let Some(code) = bus.read(ADDR, REG_CTRL) else {
            log!(Error, TAG, "no device code");
            return false;
        };
        log!(Info, TAG, "device code: {code:02x}");
        if !bus.write(ADDR, REG_CTRL, 0x01) {
            log!(Error, TAG, "error resetting");
        }
        sleep(Duration::from_millis(100));
        if !bus.write(ADDR, REG_OUTPUT_HZ, 0x00) {
            log!(Error, TAG, "error turning off output HZ");
        }
        sleep(Duration::from_millis(100));
        true
    }

    pub fn set_direction(&self, bus: &mut impl RegBus, pin: u8, output: bool) -> c_int {
        if pin > 7 {
            return ESP_ERR_INVALID_ARG;
        }
        self.with(|s| {
            if output {
                s.direction |= 1 << pin;
            } else {
                s.direction &= !(1 << pin);
            }
            let ok = bus.write(ADDR, REG_DIRECTION, s.direction);
            if !ok {
                log!(Error, TAG, "error writing direction");
            }
            err(ok)
        })
    }

    pub fn write(&self, bus: &mut impl RegBus, pin: u8, level: bool) -> c_int {
        if pin > 7 {
            return ESP_ERR_INVALID_ARG;
        }
        self.with(|s| {
            if level {
                s.output |= 1 << pin;
            } else {
                s.output &= !(1 << pin);
            }
            let ok = bus.write(ADDR, REG_OUTPUT_STATE, s.output);
            if !ok {
                log!(Error, TAG, "error writing output");
            }
            err(ok)
        })
    }

    /// Selects the pull-up, then enables the pull resistor.
    pub fn enable_pull_up(&self, bus: &mut impl RegBus, pin: u8) -> c_int {
        if pin > 7 {
            return ESP_ERR_INVALID_ARG;
        }
        self.with(|s| {
            s.pull_select |= 1 << pin;
            if !bus.write(ADDR, REG_PULL_SELECT, s.pull_select) {
                return ESP_FAIL;
            }
            s.pull_enable |= 1 << pin;
            err(bus.write(ADDR, REG_PULL_ENABLE, s.pull_enable))
        })
    }

    /// The input register (0xaa if it cannot be read, as before).
    pub fn input_status(&self, bus: &mut impl RegBus) -> (c_int, u8) {
        match bus.read(ADDR, REG_INPUT_STATUS) {
            Some(v) => (ESP_OK, v),
            None => (ESP_FAIL, 0xaa),
        }
    }

    pub fn read_pin(&self, bus: &mut impl RegBus, pin: u8) -> Result<bool, c_int> {
        if pin > 7 {
            return Err(ESP_ERR_INVALID_ARG);
        }
        let v = bus.read(ADDR, REG_INPUT_STATUS).ok_or(ESP_FAIL)?;
        Ok((v >> pin) & 1 != 0)
    }
}
