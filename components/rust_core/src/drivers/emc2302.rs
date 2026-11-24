//! EMC2302 two-channel fan controller (port of
//! `main/boards/drivers/EMC2302.cpp`): init (polarity, push-pull outputs,
//! 19.53 kHz PWM, 5-edge tach), duty cycle per channel, RPM from the tach
//! counter. Verified against the
//! C++ driver by `tests/emc2302.rs`.
//!
//! Edge cases:
//! - a tach reading of 0 gives 0 RPM;
//! - a negative (or NaN) duty cycle is 0 %.

use core::ffi::{c_int, CStr};

use crate::cutil::fixed;
use crate::drivers::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"emc2302";
pub const ADDR: u8 = 0x2e;
const POLARITY: u8 = 0x2a;
const OUTPUT_CONFIG: u8 = 0x2b;
const BASE_F123: u8 = 0x2d;
const FAN1: u8 = 0x30;
const FAN2: u8 = 0x40;
const OFS_FAN_SETTING: u8 = 0x00;
const OFS_FAN_CONFIG1: u8 = 0x02;
const OFS_TACH_READING_MSB: u8 = 0x0e;
const OFS_TACH_READING_LSB: u8 = 0x0f;

/// Channel 0 is the controller's fan 2, channel 1 its fan 1.
fn base(channel: c_int) -> u8 {
    if channel == 0 {
        FAN2
    } else {
        FAN1
    }
}

pub fn init(bus: &mut impl RegBus, invert_polarity: bool) -> bool {
    log!(Info, TAG, "initializing EMC2302");
    // polarity of both channels; push-pull outputs; 19.53 kHz base
    // frequency on both (PMB1 bits 1:0, PMB2 bits 3:2 = 01; the former 0x09
    // set PWM2 to 4.882 kHz); manual control with 5 tach edges (2 poles)
    bus.write(ADDR, POLARITY, if invert_polarity { 0x03 } else { 0x00 })
        && bus.write(ADDR, OUTPUT_CONFIG, 0x03)
        && bus.write(ADDR, BASE_F123, (0x01 << 0) | (0x01 << 2))
        && bus.write(ADDR, FAN1 + OFS_FAN_CONFIG1, 0b01 << 3)
        && bus.write(ADDR, FAN2 + OFS_FAN_CONFIG1, 0b01 << 3)
}

pub fn set_polarity(bus: &mut impl RegBus, invert: bool) -> bool {
    bus.write(ADDR, POLARITY, if invert { 0x03 } else { 0x00 })
}

/// The duty register value of `percent` (0..1).
pub fn duty(percent: f32) -> u8 {
    let v = (percent as f64 * 255.0 + 0.5) as i32;
    v.clamp(0, 255) as u8
}

pub fn set_speed(bus: &mut impl RegBus, channel: c_int, percent: f32) -> bool {
    let value = duty(percent);
    log!(
        Info,
        TAG,
        "setting fan {channel} speed to {}% (0x{value:02x})",
        fixed(percent as f64 * 100.0, 2)
    );
    bus.write(ADDR, base(channel) + OFS_FAN_SETTING, value)
}

/// RPM from the tach registers (8191 = no fan: 0).
pub fn rpm(msb: u8, lsb: u8) -> u16 {
    // the 3 low bits of the LSB are unused
    let raw = ((msb as i32) << 5) | (lsb as i32 >> 3);
    if raw == 0 || raw >= 8191 {
        return 0;
    }
    // 60 * 32768 Hz * range multiplier 1 * (5 edges - 1) / 2 poles / count
    // (NF-A9x14: 87 Hz tach at 100 %, raw 1487 -> 2644 RPM)
    let rpm = 60 * 32768 * 4 / 2 / raw;
    if rpm > 65535 {
        log!(Error, TAG, "fan speed RPM > 16bit: {rpm}");
        return 0;
    }
    rpm as u16
}

/// The channel's RPM; None on a bus error.
pub fn speed(bus: &mut impl RegBus, channel: c_int) -> Option<u16> {
    let msb = bus.read(ADDR, base(channel) + OFS_TACH_READING_MSB)?;
    let lsb = bus.read(ADDR, base(channel) + OFS_TACH_READING_LSB)?;
    let r = rpm(msb, lsb);
    log!(Info, TAG, "fan speed: {r}RPM");
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpm_and_duty() {
        // raw 1487: 2644 RPM
        assert_eq!(rpm((1487 >> 5) as u8, ((1487 & 0x1f) << 3) as u8), 2644);
        // raw 0: no division, 0 RPM
        assert_eq!(rpm(0, 0x07), 0);
        assert_eq!(rpm(0xff, 0xf8), 0);
        assert_eq!(duty(1.0), 255);
        assert_eq!(duty(2.0), 255);
        assert_eq!(duty(0.5), 128);
        assert_eq!(duty(-0.01), 0);
        assert_eq!(duty(f32::NAN), 0);
    }
}
