//! The board-independent logic of the `Board` classes (port of parts of
//! `main/boards/board.cpp` and `nerdqaxeplus.cpp`): the frequency/voltage
//! limits (validation and the clamping of the stored settings), the ASIC
//! chip temperatures and the decoding of the PMBus status registers of the
//! TPS536x7 buck controllers into a board error.
//! Verified against the C++ code by `tests/board_core.rs`.
//!
//! Behaviour notes:
//! - a stored core voltage is clamped to the board's absolute minimum and
//!   maximum when the settings are loaded, so the loaded value is always one
//!   that `setVoltage()` accepts;
//! - the chip temperatures are behind a lock (written by the ASIC result and
//!   power management tasks, read by the HTTP, display and CAN tasks) and
//!   exist from the start.

use core::ffi::c_int;
use std::sync::Mutex;

/// `Board::Error`
pub mod error {
    use core::ffi::c_int;
    pub const NONE: c_int = 0;
    pub const TEMP_FAULT: c_int = 1;
    pub const VREG_TEMP_FAULT: c_int = 2;
    pub const PSU_FAULT: c_int = 3;
    pub const IOUT_OC_FAULT: c_int = 4;
    pub const VOUT_FAULT: c_int = 5;
}

/// The absolute limits of a board (0: no limit).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Limits {
    pub abs_max_frequency: c_int,
    pub abs_min_voltage_mv: c_int,
    pub abs_max_voltage_mv: c_int,
}

/// `Board::validateVoltage()`: 0 V (output off) always passes.
pub fn validate_voltage(volts: f32, l: &Limits) -> bool {
    if volts == 0.0 {
        return true;
    }
    let mv = (volts * 1000.0) as i32;
    !(l.abs_min_voltage_mv != 0 && mv < l.abs_min_voltage_mv
        || l.abs_max_voltage_mv != 0 && mv > l.abs_max_voltage_mv)
}

/// `Board::validateFrequency()`
pub fn validate_frequency(mhz: f32, l: &Limits) -> bool {
    !(l.abs_max_frequency != 0 && mhz > l.abs_max_frequency as f32)
}

/// The stored frequency and voltage within the limits (`loadSettings()`).
pub fn clamp_settings(frequency: u16, voltage_mv: u16, l: &Limits) -> (c_int, c_int) {
    let mut f = frequency as c_int;
    if l.abs_max_frequency != 0 {
        f = f.min(l.abs_max_frequency);
    }
    let mut v = voltage_mv as c_int;
    if l.abs_max_voltage_mv != 0 {
        v = v.min(l.abs_max_voltage_mv);
    }
    if l.abs_min_voltage_mv != 0 {
        v = v.max(l.abs_min_voltage_mv);
    }
    (f, v)
}

/// The board error of the TPS536x7 status registers (`STATUS_BYTE`,
/// `_IOUT`, `_VOUT`, `_INPUT`, `_TEMPERATURE`; 0xff: unreadable) and the
/// status word reported with it.
pub fn pmbus_fault(byte: u8, iout: u8, vout: u8, input: u8, temp: u8) -> (c_int, u32) {
    let status = (byte as u32) << 24 | (iout as u32) << 16 | (vout as u32) << 8 | input as u32;
    // no 12 V: the controller does not answer, every read gives 0xff
    let e = if [byte, iout, vout, temp, input].iter().all(|&r| r == 0xff) {
        error::PSU_FAULT
    } else if iout != 0xff && iout & 0x80 != 0 {
        // IOUT_OCF
        error::IOUT_OC_FAULT
    } else if vout != 0xff && vout & 0x90 != 0 {
        // VOUT_OVF, VOUT_UVF
        error::VOUT_FAULT
    } else if temp != 0xff && temp & 0x80 != 0 {
        // OTF
        error::VREG_TEMP_FAULT
    } else if input != 0xff && input & 0x94 != 0 {
        // VIN_OVF, VIN_UVF, IIN_OCF
        error::PSU_FAULT
    } else {
        // STATUS_BYTE OFF alone is no fault (an over-temperature shutdown
        // sets it without a TPS fault flag)
        error::NONE
    };
    (e, status)
}

/// The ASIC chip temperatures.
pub struct ChipTemps(Mutex<Vec<f32>>);

impl ChipTemps {
    pub const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Vec<f32>) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// `count` chips at 0 °C.
    pub fn init(&self, count: c_int) {
        self.with(|t| *t = vec![0.0; count.max(0) as usize]);
    }

    pub fn set(&self, nr: c_int, temp: f32) {
        self.with(|t| {
            if let Some(v) = usize::try_from(nr).ok().and_then(|i| t.get_mut(i)) {
                *v = temp;
            }
        });
    }

    /// 0 for an unknown chip.
    pub fn get(&self, nr: c_int) -> f32 {
        self.with(|t| {
            usize::try_from(nr)
                .ok()
                .and_then(|i| t.get(i).copied())
                .unwrap_or(0.0)
        })
    }

    /// The hottest chip, 0 if none (`std::max`: a NaN is skipped).
    pub fn max(&self) -> f32 {
        self.with(|t| t.iter().fold(0.0f32, |m, &v| if m < v { v } else { m }))
    }
}

impl Default for ChipTemps {
    fn default() -> Self {
        Self::new()
    }
}

pub static CHIP_TEMPS: ChipTemps = ChipTemps::new();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limits_and_temps() {
        let q = Limits {
            abs_max_frequency: 730,
            abs_min_voltage_mv: 900,
            abs_max_voltage_mv: 1200,
        };
        // the minimum applies when loading too
        assert_eq!(clamp_settings(425, 800, &q), (425, 900));
        assert_eq!(clamp_settings(900, 1300, &q), (730, 1200));
        assert_eq!(clamp_settings(900, 1300, &Limits::default()), (900, 1300));
        assert!(validate_voltage(0.0, &q) && validate_voltage(0.9, &q));
        assert!(!validate_voltage(0.899, &q) && !validate_voltage(1.201, &q));
        let t = ChipTemps::new();
        assert_eq!(t.max(), 0.0);
        t.init(3);
        t.set(1, 55.5);
        t.set(2, f32::NAN);
        t.set(7, 99.0);
        t.set(-1, 99.0);
        assert_eq!((t.get(1), t.get(3), t.max()), (55.5, 0.0, 55.5));
    }
}
