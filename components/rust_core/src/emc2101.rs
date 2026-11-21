//! EMC2101 fan controller and temperature sensor of the NerdAxe boards
//! (port of `main/boards/drivers/nerdaxe/EMC2101.cpp`): tach input, PWM
//! polarity, the 6-bit duty cycle, RPM from the tach counter, the internal
//! and the external (ASIC diode) temperature, ideality and beta settings.
//! C API: the former `EMC2101.h` functions. Verified against the C++
//! driver by `tests/emc2101.rs`.
//!
//! Behaviour notes:
//! - the duty cycle is a fraction clamped to 0..1, so the boards' initial
//!   `setFanSpeed(100)` gives full speed;
//! - a tach reading of 0 gives 0 RPM, and readings above 65535 RPM give 0.

use core::ffi::CStr;

use crate::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"EMC2101";
pub const ADDR: u8 = 0x4c;
const REG_INTERNAL_TEMP: u8 = 0x00;
const REG_EXTERNAL_TEMP_MSB: u8 = 0x01;
const REG_CONFIG: u8 = 0x03;
const REG_EXTERNAL_TEMP_LSB: u8 = 0x10;
const REG_IDEALITY_FACTOR: u8 = 0x17;
const REG_BETA_COMPENSATION: u8 = 0x18;
const REG_TACH_LSB: u8 = 0x46;
const REG_TACH_MSB: u8 = 0x47;
const REG_FAN_CONFIG: u8 = 0x4a;
const REG_FAN_SETTING: u8 = 0x4c;
const TEMP_FAULT_OPEN_CIRCUIT: i16 = 0x3f8;
const TEMP_FAULT_SHORT: i16 = 0x3ff;

/// Tach input enabled (ALERT pin as TACH).
pub fn init(bus: &mut impl RegBus, invert_polarity: bool) -> bool {
    if !bus.write(ADDR, REG_CONFIG, 0x04) {
        log!(
            Error,
            TAG,
            "Failed to write 0x04 to EMC2101 register 0x{REG_CONFIG:02X}"
        );
        return false;
    }
    log!(
        Info,
        TAG,
        "Successfully wrote 0x04 to EMC2101 register 0x{REG_CONFIG:02X}"
    );
    set_polarity(bus, invert_polarity);
    true
}

/// Fan config: tach-based RPM, clock select, PWM polarity.
pub fn set_polarity(bus: &mut impl RegBus, invert: bool) -> bool {
    bus.write(
        ADDR,
        REG_FAN_CONFIG,
        0b0010_0011 | if invert { 1 << 4 } else { 0 },
    )
}

/// The 6-bit duty register value of `fraction` (0..1).
pub fn duty(fraction: f32) -> u8 {
    let f = if fraction.is_nan() {
        0.0
    } else {
        fraction.clamp(0.0, 1.0)
    };
    (63.0 * f as f64) as u8
}

pub fn set_speed(bus: &mut impl RegBus, fraction: f32) {
    if !bus.write(ADDR, REG_FAN_SETTING, duty(fraction)) {
        log!(Error, TAG, "error setting fan speed");
    }
}

/// RPM of a tach count (0xffff: stopped).
pub fn rpm(reading: u16) -> u16 {
    if reading == 0 || reading == 0xffff {
        return 0;
    }
    let rpm = 5_400_000 / reading as u32;
    u16::try_from(rpm).unwrap_or(0)
}

/// The fan RPM; 0 on a bus error.
pub fn speed(bus: &mut impl RegBus) -> u16 {
    let Some(lsb) = bus.read(ADDR, REG_TACH_LSB) else {
        log!(Error, TAG, "error reading tach LSB");
        return 0;
    };
    let Some(msb) = bus.read(ADDR, REG_TACH_MSB) else {
        log!(Error, TAG, "error reading tach MSB");
        return 0;
    };
    rpm(u16::from_le_bytes([lsb, msb]))
}

/// The 11-bit external temperature (0.125 °C); the fault codes are logged.
pub fn external_temp_of(msb: u8, lsb: u8) -> f32 {
    let mut v = (u16::from_be_bytes([msb, lsb]) >> 5) as i16;
    if v & 0x400 != 0 {
        v |= 0xf800u16 as i16;
    }
    if v == TEMP_FAULT_OPEN_CIRCUIT {
        log!(Error, TAG, "EMC2101 TEMP_FAULT_OPEN_CIRCUIT: {v:04X}");
    }
    if v == TEMP_FAULT_SHORT {
        log!(Error, TAG, "EMC2101 TEMP_FAULT_SHORT: {v:04X}");
    }
    (v as f64 / 8.0) as f32
}

/// The external (ASIC diode) temperature; 0 on a bus error.
pub fn external_temp(bus: &mut impl RegBus) -> f32 {
    let Some(msb) = bus.read(ADDR, REG_EXTERNAL_TEMP_MSB) else {
        log!(Error, TAG, "error getting external temp MSB");
        return 0.0;
    };
    let Some(lsb) = bus.read(ADDR, REG_EXTERNAL_TEMP_LSB) else {
        log!(Error, TAG, "error getting external temp LSB");
        return 0.0;
    };
    external_temp_of(msb, lsb)
}

/// The internal temperature (°C); 0 on a bus error.
pub fn internal_temp(bus: &mut impl RegBus) -> u8 {
    bus.read(ADDR, REG_INTERNAL_TEMP).unwrap_or_else(|| {
        log!(Error, TAG, "error reading internal temp");
        0
    })
}

pub fn set_ideality_factor(bus: &mut impl RegBus, ideality: u8) {
    if !bus.write(ADDR, REG_IDEALITY_FACTOR, ideality) {
        log!(Error, TAG, "error setting ideality factor");
    }
}

pub fn set_beta_compensation(bus: &mut impl RegBus, beta: u8) {
    if !bus.write(ADDR, REG_BETA_COMPENSATION, beta) {
        log!(Error, TAG, "error setting beta compensation");
    }
}

pub(crate) mod ffi {
    use super::*;
    use crate::i2c::Master;

    #[no_mangle]
    pub extern "C" fn EMC2101_init(invert_polarity: bool) -> bool {
        init(&mut Master, invert_polarity)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_set_fan_polarity(invert: bool) -> bool {
        set_polarity(&mut Master, invert)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_set_fan_speed(fraction: f32) {
        set_speed(&mut Master, fraction)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_get_fan_speed() -> u16 {
        speed(&mut Master)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_get_external_temp() -> f32 {
        external_temp(&mut Master)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_get_internal_temp() -> u8 {
        internal_temp(&mut Master)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_set_ideality_factor(ideality: u8) {
        set_ideality_factor(&mut Master, ideality)
    }

    #[no_mangle]
    pub extern "C" fn EMC2101_set_beta_compensation(beta: u8) {
        set_beta_compensation(&mut Master, beta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duty_rpm_temp() {
        assert_eq!(duty(1.0), 63);
        // the boards pass 100 at init
        assert_eq!(duty(100.0), 63);
        assert_eq!(duty(0.5), 31);
        assert_eq!(duty(-1.0), 0);
        assert_eq!(duty(f32::NAN), 0);
        assert_eq!(rpm(0), 0);
        assert_eq!(rpm(0xffff), 0);
        assert_eq!(rpm(1000), 5400);
        assert_eq!(rpm(10), 0);
        assert_eq!(external_temp_of(0x38, 0x20), 56.125);
        assert_eq!(external_temp_of(0xff, 0x00), -1.0);
    }
}
