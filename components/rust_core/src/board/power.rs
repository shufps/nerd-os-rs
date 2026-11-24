//! The power sequencing of the boards (port of `initAsics()/setVoltage()/
//! shutdown()` of `NerdQaxePlus` in `main/boards/nerdqaxeplus.cpp` with the
//! rev7 variant of `NerdQaxePlus2`, and of `NerdAxe`, `NerdaxeGamma` and
//! `NerdaxeGaia`): ASIC reset, LDOs, regulator enable/initialization, init
//! voltage, ASIC enumeration, baud rate switch, final voltage, shutdown, and
//! the NerdAxe's DS4432U DAC code of a voltage. The pins, the regulator, the
//! serial port and the ASIC driver are the caller's hooks (GPIOs, or the I2C
//! expander on the Q1370/Q1373). Verified against the
//! C++ code by `tests/board_power.rs` (the same hardware calls
//! and delays in the same order).
//!
//! The NerdAxe's DAC code is limited to the DS4432U's 7-bit magnitude, so a
//! voltage below ~0.99 V (or above ~1.93 V) gives the largest code in its
//! direction.

use core::ffi::{c_int, CStr};

use crate::board::board_core::{validate_voltage, Limits};
use crate::cutil::fixed;
use crate::log::log;

/// The two sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// NerdQAxe+, Q1370/Q1373, NerdQAxe++ with a TPS53647
    Classic,
    /// NerdQAxe++ rev7 with the TPS546: the regulator enabled before its
    /// initialization, errors abort, a settle time before each voltage
    Rev7,
    /// NerdAxe: TPS40305 trimmed by the DS4432U DAC, inverted enable
    Axe,
    /// NerdAxeGamma: TPS546
    AxeGamma,
    /// NerdAxeGaia: TPS546 and the LDO (kept on until the shutdown)
    AxeGaia,
}

impl Variant {
    fn tag(self) -> &'static CStr {
        match self {
            Variant::Classic => c"nerdqaxe+",
            Variant::Rev7 => c"nerdqaxeplus2",
            Variant::Axe => c"nerd*axe",
            Variant::AxeGamma => c"nerdaxeGamma",
            Variant::AxeGaia => c"nerdaxeGaia",
        }
    }
}

/// `board_power_params_t`: the board's settings.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Params {
    pub init_voltage_mv: c_int,
    pub asic_voltage_mv: c_int,
    pub num_phases: c_int,
    pub imax: c_int,
    pub ifault: f32,
    pub asic_frequency: c_int,
    pub asic_count: c_int,
    pub max_difficulty: u32,
    pub vr_frequency: u32,
    pub limits: Limits,
}

/// The hardware of the sequence.
pub trait Hw {
    fn vreg(&mut self, on: bool);
    fn ldo(&mut self, on: bool);
    fn asic_reset(&mut self, released: bool);
    fn delay_ms(&mut self, ms: u32);
    fn buck_init(&mut self, phases: c_int, imax: c_int, ifault: f32) -> bool;
    fn buck_set_vout(&mut self, volts: f32) -> bool;
    fn buck_disable(&mut self) -> bool;
    /// the NerdAxe's DS4432U output 0 code
    fn dac_set(&mut self, code: u8);
    fn serial_clear(&mut self);
    fn serial_set_baud(&mut self, baud: c_int);
    /// the number of chips found
    fn asic_init(
        &mut self,
        frequency: c_int,
        count: c_int,
        max_difficulty: u32,
        vr_frequency: u32,
    ) -> c_int;
    fn asic_max_baud(&mut self) -> c_int;
    /// the state the board reports: 0 buck initialized, 1 chips detected
    /// (`value`), 2 initialized, 3 shut down
    fn state(&mut self, what: c_int, value: c_int);
}

pub const STATE_BUCK_INITIALIZED: c_int = 0;
pub const STATE_CHIPS_DETECTED: c_int = 1;
pub const STATE_INITIALIZED: c_int = 2;
pub const STATE_SHUTDOWN: c_int = 3;

/// The DS4432U code that trims the NerdAxe's TPS40305 to `volts`
/// (`ds4432_tps40305_bitaxe_voltage_to_reg()`): the current per the
/// datasheet's transfer function, bit 7 set below the nominal 1.451 V; 0
/// outside 0.046..2.39 V. The magnitude is limited to 127.
pub fn ds4432_code(volts: f32) -> u8 {
    const VFB: f64 = 0.6;
    const IFS: f64 = 0.000098921;
    const RA: f64 = 4750.0;
    const RB: f64 = 3320.0;
    const VNOM: f64 = 1.451;
    let v = volts as f64;
    if !(0.046..=2.39).contains(&v) {
        return 0;
    }
    let change = (((VFB / RB) - ((v - VFB) / RA)) / IFS * 127.0).abs();
    let mut code = change.ceil();
    if code > 127.0 {
        log!(
            Warn,
            c"nerd*axe",
            "{}V is outside the DAC range, limited",
            fixed(v, 3)
        );
        code = 127.0;
    }
    let mut reg = code as u8;
    if v < VNOM {
        reg |= 0x80;
    }
    reg
}

/// `setVoltage()`: 0 V switches the output off.
pub fn set_voltage(hw: &mut impl Hw, v: Variant, volts: f32, limits: &Limits) -> bool {
    if !validate_voltage(volts, limits) {
        log!(
            Error,
            c"board",
            "Validation error. ASIC voltage {} outside the limits {}..{}",
            (volts * 1000.0) as i32,
            limits.abs_min_voltage_mv,
            limits.abs_max_voltage_mv
        );
        return false;
    }
    match v {
        Variant::Classic => {
            if volts == 0.0 {
                hw.vreg(false);
                return true;
            }
            log!(
                Info,
                v.tag(),
                "Set ASIC voltage = {}V",
                fixed(volts as f64, 3)
            );
            hw.vreg(true);
            hw.buck_set_vout(volts)
        }
        Variant::Rev7 => {
            if volts == 0.0 {
                log!(Warn, v.tag(), "Disable ASIC voltage");
                let ok = hw.buck_disable();
                hw.vreg(false);
                return ok;
            }
            hw.vreg(true);
            hw.delay_ms(20);
            log!(
                Info,
                v.tag(),
                "Set ASIC voltage = {}V",
                fixed(volts as f64, 3)
            );
            hw.buck_set_vout(volts)
        }
        Variant::Axe => {
            if volts == 0.0 {
                hw.vreg(false);
                return true;
            }
            let code = ds4432_code(volts);
            log!(
                Info,
                v.tag(),
                "Set ASIC voltage = {}V [0x{code:02X}]",
                fixed(volts as f64, 3)
            );
            hw.dac_set(code);
            hw.vreg(true);
            true
        }
        Variant::AxeGamma | Variant::AxeGaia => {
            // the TPS546 switches its output off at 0 V
            log!(
                Info,
                v.tag(),
                "Set ASIC voltage = {}V",
                fixed(volts as f64, 3)
            );
            hw.buck_set_vout(volts)
        }
    }
}

/// `initAsics()`: power up and enumerate the ASICs.
pub fn init_asics(hw: &mut impl Hw, v: Variant, p: &Params) -> bool {
    let tag = v.tag();
    let init_volts = p.init_voltage_mv.max(p.asic_voltage_mv) as f32 / 1000.0;
    let final_volts = p.asic_voltage_mv as f32 / 1000.0;
    match v {
        Variant::Axe => {
            // regulator off, reset low; the configured voltage directly
            set_voltage(hw, v, 0.0, &p.limits);
            hw.asic_reset(false);
            hw.delay_ms(250);
            set_voltage(hw, v, final_volts, &p.limits);
            hw.delay_ms(500);
        }
        Variant::AxeGamma | Variant::AxeGaia => {
            set_voltage(hw, v, 0.0, &p.limits);
            if v == Variant::AxeGaia {
                // the LDO stays on (the chip runs on it alone at first)
                hw.ldo(true);
            }
            hw.delay_ms(500);
            hw.asic_reset(false);
            hw.delay_ms(250);
            if v == Variant::AxeGaia {
                hw.delay_ms(100);
            }
            set_voltage(hw, v, init_volts, &p.limits);
            hw.delay_ms(500);
        }
        Variant::Classic => {
            // buck off (EN pin), LDOs off, reset low
            set_voltage(hw, v, 0.0, &p.limits);
            hw.ldo(false);
            hw.asic_reset(false);
            hw.delay_ms(250);
            hw.ldo(true);
            hw.delay_ms(100);
            // (the result is not checked: without power the ASICs are not found)
            hw.buck_init(p.num_phases, p.imax, p.ifault);
            // the higher of the init and the configured voltage
            set_voltage(hw, v, init_volts, &p.limits);
            hw.delay_ms(500);
        }
        Variant::Rev7 => {
            hw.vreg(false);
            hw.ldo(false);
            hw.asic_reset(false);
            hw.delay_ms(250);
            hw.ldo(true);
            hw.delay_ms(100);
            hw.vreg(true);
            hw.delay_ms(50);
            if !hw.buck_init(p.num_phases, p.imax, p.ifault) {
                log!(Error, tag, "error initializing buck regulator");
                return false;
            }
            if !set_voltage(hw, v, init_volts, &p.limits) {
                log!(Error, tag, "Failed to set init voltage");
                return false;
            }
            hw.delay_ms(500);
        }
    }
    hw.state(STATE_BUCK_INITIALIZED, 1);

    // release the reset
    hw.asic_reset(true);
    hw.delay_ms(250);

    hw.serial_clear();
    let chips = hw.asic_init(
        p.asic_frequency,
        p.asic_count,
        p.max_difficulty,
        p.vr_frequency,
    );
    hw.state(STATE_CHIPS_DETECTED, chips);
    if chips == 0 {
        log!(Error, tag, "error initializing asics!");
        return false;
    }
    let baud = hw.asic_max_baud();
    // (a delay is needed here since esp-idf 5.4)
    hw.delay_ms(500);
    hw.serial_set_baud(baud);
    hw.serial_clear();
    hw.delay_ms(500);

    // the NerdAxe family keeps the init voltage until the power management
    // applies the configured one
    if matches!(v, Variant::Classic | Variant::Rev7) {
        let ok = set_voltage(hw, v, final_volts, &p.limits);
        if v == Variant::Rev7 && !ok {
            log!(Error, tag, "Failed to set final voltage");
            return false;
        }
    }
    hw.state(STATE_INITIALIZED, 1);
    true
}

/// `shutdown()`: voltage off, LDOs off.
pub fn shutdown(hw: &mut impl Hw, v: Variant, limits: &Limits) {
    set_voltage(hw, v, 0.0, limits);
    // (the NerdAxe/NerdAxeGamma have no LDO switch)
    if !matches!(v, Variant::Axe | Variant::AxeGamma) {
        hw.delay_ms(500);
        hw.ldo(false);
        hw.delay_ms(500);
    }
    hw.state(STATE_SHUTDOWN, 1);
}
