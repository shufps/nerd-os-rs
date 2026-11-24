//! The TPS53647 / TPS53667 multiphase buck controllers of the ASIC core
//! voltage (port of `main/boards/drivers/TPS53647.cpp` and `TPS53667.cpp`):
//! detection, the init sequence, VID and SLINEAR11 conversions, the output
//! voltage, the telemetry getters and the status registers. The SMBus
//! transactions go through the `Bus` trait (`idf::i2c::SmBus` in the
//! firmware: the former I2C command links, unchanged).
//! Verified against the C++ drivers by
//! `tests/tps536x7.rs` (the same SMBus transactions).
//!
//! Behaviour notes:
//! - a failed SMBus read returns the getter's default (0, or the status
//!   default 0xff/0xffff);
//! - the "initialized" flag starts false until `init()`.

use core::ffi::{c_int, CStr};

use crate::log::log;

pub const ADDR: u8 = 0x71;
pub const DEVICE_CODE_TPS53647: u16 = 0x01f0;
pub const DEVICE_CODE_TPS53667: u16 = 0x01f8;

/// PMBus commands (`pmbus_commands.h`).
pub mod cmd {
    pub const ON_OFF_CONFIG: u8 = 0x02;
    pub const CLEAR_FAULTS: u8 = 0x03;
    pub const RESTORE_DEFAULT_ALL: u8 = 0x12;
    pub const VOUT_COMMAND: u8 = 0x21;
    pub const VOUT_MAX: u8 = 0x24;
    pub const VOUT_MARGIN_HIGH: u8 = 0x25;
    pub const VOUT_MARGIN_LOW: u8 = 0x26;
    pub const IOUT_OC_FAULT_LIMIT: u8 = 0x46;
    pub const IOUT_OC_WARN_LIMIT: u8 = 0x4a;
    pub const OT_FAULT_LIMIT: u8 = 0x4f;
    pub const OT_WARN_LIMIT: u8 = 0x51;
    pub const IIN_OC_FAULT_LIMIT: u8 = 0x5b;
    pub const IIN_OC_WARN_LIMIT: u8 = 0x5d;
    pub const STATUS_BYTE: u8 = 0x78;
    pub const STATUS_WORD: u8 = 0x79;
    pub const STATUS_VOUT: u8 = 0x7a;
    pub const STATUS_IOUT: u8 = 0x7b;
    pub const STATUS_INPUT: u8 = 0x7c;
    pub const STATUS_TEMPERATURE: u8 = 0x7d;
    pub const STATUS_MFR_SPECIFIC: u8 = 0x80;
    pub const READ_VIN: u8 = 0x88;
    pub const READ_IIN: u8 = 0x89;
    pub const READ_IOUT: u8 = 0x8c;
    pub const READ_TEMPERATURE_1: u8 = 0x8d;
    pub const READ_POUT: u8 = 0x96;
    pub const READ_PIN: u8 = 0x97;
    pub const MFR_SPECIFIC_00: u8 = 0xd0;
    pub const MFR_SPECIFIC_04: u8 = 0xd4;
    pub const MFR_SPECIFIC_10: u8 = 0xda;
    pub const MFR_SPECIFIC_12: u8 = 0xdc;
    pub const MFR_SPECIFIC_13: u8 = 0xdd;
    pub const MFR_SPECIFIC_16: u8 = 0xe0;
    pub const MFR_SPECIFIC_19: u8 = 0xe3;
    pub const MFR_SPECIFIC_20: u8 = 0xe4;
    pub const MFR_SPECIFIC_24: u8 = 0xe8;
    pub const MFR_SPECIFIC_44: u8 = 0xfc;
}

/// The SMBus transactions with the controller (address included).
pub trait Bus {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8>;
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16>;
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool;
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool;
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Tps53647,
    /// `no_phase_shedding`: NerdOctaxeGamma and Q1373 keep all 6 phases on
    Tps53667 {
        no_phase_shedding: bool,
    },
}

const HW_MIN_VOLTAGE: f32 = 0.25;
const VOUT_MIN: f32 = 0.8;
const VOUT_MAX: f32 = 1.4;
const ON_OFF_CONFIG: u8 = 0b0001_0111;
const OT_WARN_LIMIT: f32 = 95.0;
const OT_FAULT_LIMIT: f32 = 125.0;

/// SLINEAR11 (5-bit exponent, 11-bit mantissa, both two's complement).
pub fn slinear11_to_float(value: u16) -> f32 {
    let mut exponent = (value >> 11) as i32;
    let mut mantissa = (value & 0x7ff) as i32;
    if exponent & 0x10 != 0 {
        exponent |= !0x1f;
    }
    if mantissa & 0x400 != 0 {
        mantissa |= !0x7ff;
    }
    mantissa as f32 * 2f32.powi(exponent)
}

/// The smallest exponent (from -16) whose rounded mantissa fits 0..=1023;
/// 0 for values <= 0 or without a solution.
pub fn float_to_slinear11(x: f32) -> u16 {
    if x <= 0.0 {
        log!(Info, c"TPS53647.c", "No negative numbers at this time");
        return 0;
    }
    for e in -16i32..=15 {
        let m = (x / 2f32.powi(e)).round() as i32;
        if (0..=1023).contains(&m) {
            return (((e & 0x1f) as u16) << 11) | (m as u16 & 0x7ff);
        }
    }
    log!(Info, c"TPS53647.c", "Could not find a solution");
    0
}

/// VID of a voltage (5 mV steps from 0.25 V, VID 1 = 0.25 V), in float32
/// as the C++ code; 0 for 0 V or out of range.
pub fn volt_to_vid(volts: f32) -> u8 {
    if volts == 0.0 {
        return 0;
    }
    let v = ((volts - HW_MIN_VOLTAGE) / 0.005f32) as i32 + 1;
    if v > 0xff {
        log!(
            Error,
            c"TPS53647.c",
            "ERR - vout register value out of range {v}"
        );
        return 0;
    }
    v as u8
}

pub fn vid_to_volt(vid: u8) -> f32 {
    if vid == 0 {
        return 0.0;
    }
    ((vid as f64 - 1.0) * 0.005 + HW_MIN_VOLTAGE as f64) as f32
}

pub struct Tps536x7 {
    chip: Chip,
    initialized: bool,
}

impl Tps536x7 {
    pub fn new(chip: Chip) -> Self {
        Self {
            chip,
            initialized: false,
        }
    }

    fn tag(&self) -> &'static CStr {
        match self.chip {
            Chip::Tps53647 => c"TPS53647.c",
            Chip::Tps53667 { .. } => c"TPS53667",
        }
    }

    pub fn device_code(bus: &mut impl Bus) -> u16 {
        bus.read_word(ADDR, cmd::MFR_SPECIFIC_44).unwrap_or(0)
    }

    fn set_phases(&self, bus: &mut impl Bus, phases: c_int) {
        if !(1..=6).contains(&phases) {
            log!(
                Error,
                c"TPS53647.c",
                "number of phases out of range: {phases}"
            );
        } else {
            log!(Info, c"TPS53647.c", "setting {phases} phases");
            bus.write_byte(ADDR, cmd::MFR_SPECIFIC_20, (phases - 1) as u8);
        }
        if let Chip::Tps53667 { .. } = self.chip {
            // all 6 phases enabled
            bus.write_byte(ADDR, cmd::MFR_SPECIFIC_24, 0x00);
        }
    }

    /// Detects the chip and sets it up (the regulator stays off).
    pub fn init(&mut self, bus: &mut impl Bus, phases: c_int, imax: c_int, ifault: f32) -> bool {
        let code = Self::device_code(bus);
        match self.chip {
            Chip::Tps53647 => {
                log!(Info, self.tag(), "Initializing the core voltage regulator");
                log!(Info, self.tag(), "Device Code: {code:04x}");
                if code != DEVICE_CODE_TPS53647 {
                    log!(
                        Error,
                        self.tag(),
                        "ERROR- cannot find TPS53647 buck controller"
                    );
                    return false;
                }
                log!(Info, self.tag(), "found TPS53647 controller");
                bus.write_command(ADDR, cmd::CLEAR_FAULTS);
                bus.write_command(ADDR, cmd::RESTORE_DEFAULT_ALL);
                // make sure the buck is switched off
                bus.write_byte(ADDR, cmd::ON_OFF_CONFIG, ON_OFF_CONFIG);
                // switching frequency 500 kHz
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_12, 0x20);
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_10, imax as u8);
                // VR12 mode, dynamic phase shedding, slew rate 0.68 mV/us
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_13, 0x89);
                bus.write_byte(ADDR, cmd::ON_OFF_CONFIG, ON_OFF_CONFIG);
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_12, 0x20);
                self.set_phases(bus, phases);
                self.limits(bus, ifault);
                self.initialized = true;
            }
            Chip::Tps53667 { no_phase_shedding } => {
                log!(Info, self.tag(), "Initializing TPS53667 regulator");
                if code != DEVICE_CODE_TPS53667 {
                    log!(
                        Error,
                        self.tag(),
                        "TPS53667 not found. Device code: 0x{code:04X}"
                    );
                    return false;
                }
                log!(Info, self.tag(), "Found TPS53667 controller");
                bus.write_command(ADDR, cmd::CLEAR_FAULTS);
                bus.write_command(ADDR, cmd::RESTORE_DEFAULT_ALL);
                bus.write_byte(ADDR, cmd::ON_OFF_CONFIG, ON_OFF_CONFIG);
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_12, 0x20);
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_10, imax as u8);
                if no_phase_shedding {
                    bus.write_byte(ADDR, cmd::MFR_SPECIFIC_13, 0x89);
                    log!(
                        Info,
                        self.tag(),
                        "Phase shedding DISABLED (0x89) - forcing all 6 phases ON"
                    );
                } else {
                    bus.write_byte(ADDR, cmd::MFR_SPECIFIC_13, 0x99);
                    log!(
                        Info,
                        self.tag(),
                        "Phase shedding ENABLED (0x99) - dynamic phase control"
                    );
                }
                self.set_phases(bus, phases);
                bus.write_word(ADDR, cmd::VOUT_MAX, volt_to_vid(VOUT_MAX) as u16);
                // per-phase overcurrent limit 45 A
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_00, 0x07);
                // VIN undervoltage (UVLO) 6 V
                bus.write_byte(ADDR, cmd::MFR_SPECIFIC_16, 0x01);
                bus.write_word(ADDR, cmd::MFR_SPECIFIC_19, 0x0003);
                self.limits(bus, ifault);
                // input current: about 300 W warning, 336 W fault
                bus.write_word(ADDR, cmd::IIN_OC_WARN_LIMIT, float_to_slinear11(25.0));
                bus.write_word(ADDR, cmd::IIN_OC_FAULT_LIMIT, float_to_slinear11(28.0));
                self.initialized = true;
                self.show_voltage_settings(bus);
            }
        }
        true
    }

    /// Temperature and output current limits (warning = fault for the
    /// current).
    fn limits(&self, bus: &mut impl Bus, ifault: f32) {
        bus.write_word(ADDR, cmd::OT_WARN_LIMIT, float_to_slinear11(OT_WARN_LIMIT));
        bus.write_word(
            ADDR,
            cmd::OT_FAULT_LIMIT,
            float_to_slinear11(OT_FAULT_LIMIT),
        );
        bus.write_word(ADDR, cmd::IOUT_OC_WARN_LIMIT, float_to_slinear11(ifault));
        bus.write_word(ADDR, cmd::IOUT_OC_FAULT_LIMIT, float_to_slinear11(ifault));
    }

    fn show_voltage_settings(&self, bus: &mut impl Bus) {
        log!(
            Info,
            c"TPS53647.c",
            "-----------VOLTAGE---------------------"
        );
        for (reg, what) in [
            (cmd::VOUT_MAX, "Vout Max set to"),
            (cmd::VOUT_MARGIN_HIGH, "Vout Margin HIGH"),
            (cmd::VOUT_COMMAND, "Vout set to"),
            (cmd::VOUT_MARGIN_LOW, "Vout Margin LOW"),
        ] {
            let v = bus
                .read_word(ADDR, reg)
                .map_or(0.0, |w| vid_to_volt(w as u8));
            log!(
                Info,
                c"TPS53647.c",
                "{what}: {} V",
                crate::util::cutil::fixed(v as f64, 6)
            );
        }
    }

    pub fn clear_faults(&self, bus: &mut impl Bus) {
        bus.write_command(ADDR, cmd::CLEAR_FAULTS);
    }

    /// A SLINEAR11 reading; 0 before `init()` or on an error.
    fn read_slinear11(&self, bus: &mut impl Bus, reg: u8) -> f32 {
        if !self.initialized {
            return 0.0;
        }
        bus.read_word(ADDR, reg).map_or(0.0, slinear11_to_float)
    }

    pub fn temperature(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_TEMPERATURE_1)
    }
    pub fn pin(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_PIN)
    }
    pub fn pout(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_POUT)
    }
    pub fn vin(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_VIN)
    }
    pub fn iin(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_IIN)
    }
    pub fn iout(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear11(bus, cmd::READ_IOUT)
    }

    /// The measured output voltage (MFR_SPECIFIC_04, 2^-9 V per LSB).
    pub fn vout(&self, bus: &mut impl Bus) -> f32 {
        if !self.initialized {
            return 0.0;
        }
        bus.read_word(ADDR, cmd::MFR_SPECIFIC_04)
            .map_or(0.0, |w| w as f32 * 2f32.powi(-9))
    }

    /// Sets the output voltage; 0 does nothing. The VID 0x97 (1.000 V, the
    /// factory default) is never set, so a reset of the controller can be
    /// detected: 0.995 V is used instead.
    pub fn set_vout(&self, bus: &mut impl Bus, volts: f32) -> bool {
        if volts == 0.0 {
            return true;
        }
        if !(VOUT_MIN..=VOUT_MAX).contains(&volts) {
            log!(
                Error,
                c"TPS53647.c",
                "ERR- Voltage requested ({} V) is out of range",
                crate::util::cutil::fixed(volts as f64, 6)
            );
            return false;
        }
        let mut vid = volt_to_vid(volts);
        if vid == 0x97 {
            vid = 0x96;
        }
        bus.write_word(ADDR, cmd::VOUT_COMMAND, vid as u16);
        log!(
            Info,
            c"TPS53647.c",
            "Vout changed to {} V",
            crate::util::cutil::fixed(volts as f64, 2)
        );
        true
    }

    /// The VID in VOUT_COMMAND (0x97 = 1.00 V if it cannot be read).
    pub fn vout_vid(&self, bus: &mut impl Bus) -> u16 {
        bus.read_word(ADDR, cmd::VOUT_COMMAND).unwrap_or(0x97)
    }

    /// A status register (0xff if it cannot be read).
    pub fn status_reg(&self, bus: &mut impl Bus, reg: u8) -> u8 {
        bus.read_byte(ADDR, reg).unwrap_or(0xff)
    }

    /// Logs the status registers (as error if any bit is set).
    pub fn status(&self, bus: &mut impl Bus) {
        let byte = self.status_reg(bus, cmd::STATUS_BYTE);
        let word = bus.read_word(ADDR, cmd::STATUS_WORD).unwrap_or(0xffff);
        let vout = self.status_reg(bus, cmd::STATUS_VOUT);
        let iout = self.status_reg(bus, cmd::STATUS_IOUT);
        let input = self.status_reg(bus, cmd::STATUS_INPUT);
        let mfr = self.status_reg(bus, cmd::STATUS_MFR_SPECIFIC);
        let phase = match self.chip {
            Chip::Tps53667 { .. } => Some(self.status_reg(bus, cmd::MFR_SPECIFIC_24)),
            Chip::Tps53647 => None,
        };
        // suppress the communication error bit
        let byte = byte & !0x02;
        let word = word & !0x0002;
        let error = byte != 0
            || word != 0
            || vout != 0
            || iout != 0
            || input != 0
            || mfr != 0
            || phase.is_some_and(|p| p != 0);
        let line = format!(
            "TPS536X7_status  bytes: {byte:02x}, word: {word:04x}, vout: {vout:02x}, iout: {iout:02x}, input: {input:02x}, mfr_spec: {mfr:02x}{}",
            phase.map(|p| format!(" phase: {p:02x}")).unwrap_or_default()
        );
        let level = if error {
            crate::log::Level::Error
        } else {
            crate::log::Level::Info
        };
        crate::log::write(level, self.tag(), &line);
    }
}

// ---------------------------------------------------------------------------
// C API

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(slinear11_to_float(0x0000), 0.0);
        // exponent -2, mantissa 200: 50.0
        assert_eq!(slinear11_to_float((0x1e << 11) | 200), 50.0);
        // negative mantissa
        assert_eq!(slinear11_to_float((0x1f << 11) | 0x7ff), -0.5);
        assert_eq!(float_to_slinear11(95.0), (0x1d << 11) | 760); // 760 * 2^-3
        assert_eq!(float_to_slinear11(0.0), 0);
        assert_eq!(volt_to_vid(1.0), 0x97);
        assert_eq!(volt_to_vid(0.0), 0);
        assert_eq!(vid_to_volt(0x96), 0.995);
        assert_eq!(volt_to_vid(2.0), 0);
    }
}
