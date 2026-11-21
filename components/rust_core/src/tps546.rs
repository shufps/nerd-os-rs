//! TPS546D24A/S PMBus buck converter of the ASIC core voltage (port of
//! `main/boards/drivers/nerdaxe/TPS546.cpp`, NerdAxe/Gamma/Gaia, and of the
//! configurable `main/boards/drivers/rev7/TPS546.cpp`, NerdQAxe++ rev7 with
//! one or two stacked converters): detection by the device ID, the
//! configuration written on every boot, the SLINEAR11/ULINEAR16
//! conversions, the output voltage (per voltage domain), the switching
//! frequency, the telemetry getters and the status registers. The two
//! drivers differ in a few steps of the sequences (`Variant`). The SMBus
//! transactions go through the `Bus` trait (`idf::i2c::SmBus` in the
//! firmware: the former I2C command links and register reads). Verified against both C++ drivers by
//! `tests/tps546.rs`.
//!
//! Behaviour notes:
//! - a failed SMBus word read gives the getter's 0 (also for the
//!   temperature); a failed VOUT_MODE read fails the ULINEAR16 conversion;
//! - SMBus block writes send the whole block, and the MFR model and
//!   revision go to MFR_MODEL/MFR_REVISION (as in the rev7 driver).

use core::ffi::{c_int, CStr};
use std::sync::Mutex;

use crate::cutil::fixed;
use crate::log::log;

const TAG: &CStr = c"TPS546";
pub const ADDR: u8 = 0x24;

/// PMBus commands (`pmbus_commands.h`).
pub mod cmd {
    pub const PHASE: u8 = 0x04;
    pub const CLEAR_FAULTS: u8 = 0x03;
    pub const STATUS_BYTE: u8 = 0x78;
    pub const STATUS_IOUT: u8 = 0x7b;
    pub const STATUS_TEMPERATURE: u8 = 0x7d;
    pub const SYNC_CONFIG: u8 = 0xe4;
    pub const STACK_CONFIG: u8 = 0xec;
    pub const OPERATION: u8 = 0x01;
    pub const ON_OFF_CONFIG: u8 = 0x02;
    pub const VOUT_MODE: u8 = 0x20;
    pub const VOUT_COMMAND: u8 = 0x21;
    pub const VOUT_MAX: u8 = 0x24;
    pub const VOUT_MARGIN_HIGH: u8 = 0x25;
    pub const VOUT_MARGIN_LOW: u8 = 0x26;
    pub const VOUT_SCALE_LOOP: u8 = 0x29;
    pub const VOUT_MIN: u8 = 0x2b;
    pub const FREQUENCY_SWITCH: u8 = 0x33;
    pub const VIN_ON: u8 = 0x35;
    pub const VIN_OFF: u8 = 0x36;
    pub const VOUT_OV_FAULT_LIMIT: u8 = 0x40;
    pub const VOUT_OV_WARN_LIMIT: u8 = 0x42;
    pub const VOUT_UV_WARN_LIMIT: u8 = 0x43;
    pub const VOUT_UV_FAULT_LIMIT: u8 = 0x44;
    pub const IOUT_OC_FAULT_LIMIT: u8 = 0x46;
    pub const IOUT_OC_FAULT_RESPONSE: u8 = 0x47;
    pub const IOUT_OC_WARN_LIMIT: u8 = 0x4a;
    pub const OT_FAULT_LIMIT: u8 = 0x4f;
    pub const OT_FAULT_RESPONSE: u8 = 0x50;
    pub const OT_WARN_LIMIT: u8 = 0x51;
    pub const VIN_OV_FAULT_LIMIT: u8 = 0x55;
    pub const VIN_OV_FAULT_RESPONSE: u8 = 0x56;
    pub const VIN_UV_WARN_LIMIT: u8 = 0x58;
    pub const TON_DELAY: u8 = 0x60;
    pub const TON_RISE: u8 = 0x61;
    pub const TON_MAX_FAULT_LIMIT: u8 = 0x62;
    pub const TON_MAX_FAULT_RESPONSE: u8 = 0x63;
    pub const TOFF_DELAY: u8 = 0x64;
    pub const TOFF_FALL: u8 = 0x65;
    pub const STATUS_WORD: u8 = 0x79;
    pub const STATUS_VOUT: u8 = 0x7a;
    pub const STATUS_INPUT: u8 = 0x7c;
    pub const READ_VIN: u8 = 0x88;
    pub const READ_VOUT: u8 = 0x8b;
    pub const READ_IOUT: u8 = 0x8c;
    pub const READ_TEMPERATURE_1: u8 = 0x8d;
    pub const MFR_ID: u8 = 0x99;
    pub const MFR_MODEL: u8 = 0x9a;
    pub const MFR_REVISION: u8 = 0x9b;
    pub const IC_DEVICE_ID: u8 = 0xad;
    pub const COMPENSATION_CONFIG: u8 = 0xb1;
    pub const PIN_DETECT_OVERRIDE: u8 = 0xee;
}

const DEVICE_IDS: [[u8; 6]; 3] = [
    [0x54, 0x49, 0x54, 0x6b, 0x24, 0x41], // TPS546D24A
    [0x54, 0x49, 0x54, 0x6d, 0x24, 0x41], // TPS546D24A
    [0x54, 0x49, 0x54, 0x6d, 0x24, 0x62], // TPS546D24S
];
const MFR_ID: [u8; 3] = *b"BAX";
const MFR_MODEL: [u8; 3] = *b"HEX";
const MFR_REVISION: [u8; 3] = [0x00, 0x00, 0x01];

/// PMBUS_ON_OFF_CONFIG: CMD | PU (the output follows OPERATION)
const ON_OFF_CONFIG: u8 = 0x18;
const OPERATION_OFF: u8 = 0x00;
const OPERATION_ON: u8 = 0x80;
const FREQUENCY: c_int = 650;
const VIN_OV_FAULT_RESPONSE: u8 = 0xb7;
const VOUT_OV_FAULT_LIMIT: f32 = 1.25;
const VOUT_OV_WARN_LIMIT: f32 = 1.1;
const VOUT_MARGIN_HIGH: f32 = 1.1;
const VOUT_MARGIN_LOW: f32 = 0.90;
const VOUT_UV_WARN_LIMIT: f32 = 0.90;
const VOUT_UV_FAULT_LIMIT: f32 = 0.75;
const IOUT_OC_FAULT_RESPONSE: u8 = 0xc0;
const OT_WARN_LIMIT: c_int = 105;
const OT_FAULT_LIMIT: c_int = 145;
const OT_FAULT_RESPONSE: u8 = 0xff;
const TON_DELAY: c_int = 0;
const TON_RISE: c_int = 3;
const TON_MAX_FAULT_LIMIT: c_int = 0;
const TON_MAX_FAULT_RESPONSE: u8 = 0x3b;
const TOFF_DELAY: c_int = 0;
const TOFF_FALL: c_int = 0;
const PIN_DETECT_OVERRIDE: u16 = 0x0000;

/// The SMBus transactions with the converter (address included).
pub trait Bus {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8>;
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16>;
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool;
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool;
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool;
    /// A block read: the length byte, then `out.len()` data bytes.
    fn read_block(&mut self, addr: u8, cmd: u8, out: &mut [u8]) -> bool;
    /// A block write: the length byte, then `data`.
    fn write_block(&mut self, addr: u8, cmd: u8, data: &[u8]) -> bool;
}

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

pub fn slinear11_to_int(value: u16) -> c_int {
    slinear11_to_float(value) as c_int
}

/// The smallest non-negative exponent whose truncated mantissa is below
/// 1024; 0 for negative values or without a solution.
pub fn int_to_slinear11(value: c_int) -> u16 {
    if value < 0 {
        log!(Info, TAG, "No negative numbers at this time");
        return 0;
    }
    for e in 0..=15 {
        let mantissa = (value as f32 / 2f32.powi(e)) as i32;
        if mantissa < 1024 {
            return ((((e << 11) & 0xf800) + mantissa) & 0xffff) as u16;
        }
    }
    log!(Info, TAG, "Could not find a solution");
    0
}

/// A negative exponent: the mantissa is truncated at the last power of two
/// (from 2^0) before it reaches 1024; 0 for values <= 0 or without a
/// solution.
pub fn float_to_slinear11(value: f32) -> u16 {
    if value.is_nan() || value <= 0.0 {
        log!(Info, TAG, "No negative numbers at this time");
        return 0;
    }
    for i in 0..=15 {
        if (value * 2f32.powi(i)) as i32 >= 1024 {
            let exponent = i - 1;
            let mantissa = (value * 2f32.powi(exponent)) as i32;
            return ((((-exponent) << 11) & 0xf800) + mantissa) as u16;
        }
    }
    log!(Info, TAG, "Could not find a solution");
    0
}

/// The VOUT_MODE exponent (bits 4..0, two's complement).
fn vout_exponent(mode: u8) -> i32 {
    let e = (mode & 0x1f) as i32;
    if e & 0x10 != 0 {
        e - 32
    } else {
        e
    }
}

pub fn ulinear16_to_float(value: u16, mode: u8) -> f32 {
    value as f32 * 2f32.powi(vout_exponent(mode))
}

pub fn float_to_ulinear16(value: f32, mode: u8) -> u16 {
    (value / 2f32.powi(vout_exponent(mode))) as u16
}

/// The two former drivers: the sequences differ in a few steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// `nerdaxe/TPS546.cpp` (NerdAxe, NerdAxeGamma, NerdAxeGaia)
    NerdAxe,
    /// `rev7/TPS546.cpp` (NerdQAxe++ rev7): output off before the
    /// configuration, stack/sync/phase and compensation, the VOUT limits
    /// rewritten with every voltage change, faults cleared after init and
    /// before the output is switched on
    Rev7,
}

/// `tps546_config_t` (the former `TPS546_CONFIG`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    pub phase: u8,
    pub vin_on: f32,
    pub vin_off: f32,
    /// not written if <= 0 (rev7)
    pub vin_uv_warn_limit: f32,
    pub vin_ov_fault_limit: f32,
    pub scale_loop: f32,
    pub vout_min: f32,
    pub vout_max: f32,
    pub vout_command: f32,
    pub iout_oc_warn_limit: f32,
    pub iout_oc_fault_limit: f32,
    pub stack_config: u16,
    pub sync_config: u8,
    /// written only if all five bytes are non-zero (rev7)
    pub compensation_config: [u8; 5],
}

impl Config {
    /// The NerdAxe driver's 5 V defaults.
    pub const fn nerdaxe() -> Self {
        Self {
            phase: 0x00,
            vin_on: 4.8,
            vin_off: 4.5,
            vin_uv_warn_limit: 5.8,
            vin_ov_fault_limit: 6.0,
            scale_loop: 0.25,
            vout_min: 0.8,
            vout_max: 3.0,
            vout_command: 1.2,
            iout_oc_warn_limit: 25.0,
            iout_oc_fault_limit: 30.0,
            stack_config: 0x0000,
            sync_config: 0x10,
            compensation_config: [0; 5],
        }
    }

    /// `TPS546_create_default_config()` of the rev7 driver: one converter.
    pub const fn rev7_single() -> Self {
        Self {
            vout_min: 1.0,
            ..Self::nerdaxe()
        }
    }

    /// `TPS546_create_dual_config()`: two stacked converters at 12 V.
    pub const fn rev7_dual() -> Self {
        Self {
            phase: 0xff,
            vin_on: 11.0,
            vin_off: 10.5,
            vin_uv_warn_limit: 11.0,
            vin_ov_fault_limit: 14.0,
            scale_loop: 0.125,
            vout_min: 1.0,
            vout_max: 3.0,
            vout_command: 1.2,
            iout_oc_warn_limit: 90.0,
            iout_oc_fault_limit: 100.0,
            stack_config: 0x0001,
            sync_config: 0xd0,
            compensation_config: [0x12, 0x34, 0x42, 0x25, 0x04],
        }
    }
}

/// The VOUT limits relative to VOUT_COMMAND.
const VOUT_LIMIT_RATIOS: [(u8, f32, &str); 6] = [
    (
        cmd::VOUT_OV_FAULT_LIMIT,
        VOUT_OV_FAULT_LIMIT,
        "VOUT_OV_FAULT_LIMIT",
    ),
    (
        cmd::VOUT_OV_WARN_LIMIT,
        VOUT_OV_WARN_LIMIT,
        "VOUT_OV_WARN_LIMIT",
    ),
    (cmd::VOUT_MARGIN_HIGH, VOUT_MARGIN_HIGH, "VOUT_MARGIN_HIGH"),
    (cmd::VOUT_MARGIN_LOW, VOUT_MARGIN_LOW, "VOUT_MARGIN_LOW"),
    (
        cmd::VOUT_UV_WARN_LIMIT,
        VOUT_UV_WARN_LIMIT,
        "VOUT_UV_WARN_LIMIT",
    ),
    (
        cmd::VOUT_UV_FAULT_LIMIT,
        VOUT_UV_FAULT_LIMIT,
        "VOUT_UV_FAULT_LIMIT",
    ),
];

struct State {
    config: Config,
    initialized: bool,
}

pub struct Tps546 {
    variant: Variant,
    /// the output voltage is split over this many ASIC voltage domains
    domains: f32,
    s: Mutex<State>,
}

/// The ULINEAR16 conversions read VOUT_MODE every time, as before.
fn to_volts(bus: &mut impl Bus, value: u16) -> Option<f32> {
    let mode = bus.read_byte(ADDR, cmd::VOUT_MODE)?;
    Some(ulinear16_to_float(value, mode))
}

fn from_volts(bus: &mut impl Bus, volts: f32) -> Option<u16> {
    let mode = bus.read_byte(ADDR, cmd::VOUT_MODE)?;
    Some(float_to_ulinear16(volts, mode))
}

fn hex(b: &[u8]) -> String {
    b.iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn f2(x: f32) -> String {
    fixed(x as f64, 2)
}

/// Reads the device ID; true for the three known IDs.
pub fn probe(bus: &mut impl Bus) -> bool {
    let mut id = [0u8; 6];
    bus.read_block(ADDR, cmd::IC_DEVICE_ID, &mut id) && DEVICE_IDS.contains(&id)
}

impl Tps546 {
    pub fn new(variant: Variant, config: Config, domains: u16) -> Self {
        Self {
            variant,
            domains: domains as f32,
            s: Mutex::new(State {
                config,
                initialized: false,
            }),
        }
    }

    /// The NerdAxe driver with its 5 V defaults.
    pub fn nerdaxe() -> Self {
        Self::new(Variant::NerdAxe, Config::nerdaxe(), 1)
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.s.lock().unwrap_or_else(|e| e.into_inner()))
    }

    fn initialized(&self) -> bool {
        self.with(|s| s.initialized)
    }

    fn config(&self) -> Config {
        self.with(|s| s.config)
    }

    /// The VIN thresholds (V); before `init()`.
    pub fn set_vin_config(&self, on: f32, off: f32, uv_warn: f32, ov_fault: f32) {
        self.with(|s| {
            s.config.vin_on = on;
            s.config.vin_off = off;
            s.config.vin_uv_warn_limit = uv_warn;
            s.config.vin_ov_fault_limit = ov_fault;
        });
    }

    /// The output over-current warn/fault limits (A); before `init()`.
    pub fn set_iout_config(&self, warn: f32, fault: f32) {
        self.with(|s| {
            s.config.iout_oc_warn_limit = warn;
            s.config.iout_oc_fault_limit = fault;
        });
    }

    /// Checks the device ID, writes the configuration (output off until
    /// `set_vout()`); 0 or -1.
    pub fn init(&self, bus: &mut impl Bus) -> c_int {
        log!(Info, TAG, "Initializing the core voltage regulator");
        let mut id = [0u8; 6];
        let ok = bus.read_block(ADDR, cmd::IC_DEVICE_ID, &mut id);
        log!(Info, TAG, "Device ID: {}", hex(&id));
        if !ok || !DEVICE_IDS.contains(&id) {
            log!(
                Error,
                TAG,
                "Cannot find TPS546 regulator - Device ID mismatch"
            );
            return -1;
        }

        if self.variant == Variant::Rev7 {
            bus.write_byte(ADDR, cmd::OPERATION, OPERATION_OFF);
        }
        log!(Info, TAG, "Power config-ON_OFF_CONFIG: {ON_OFF_CONFIG:02x}");
        bus.write_byte(ADDR, cmd::ON_OFF_CONFIG, ON_OFF_CONFIG);

        self.read_mfr_info(bus);

        log!(Info, TAG, "Writing new config values");
        match bus.read_byte(ADDR, cmd::VOUT_MODE) {
            Some(m) => log!(Info, TAG, "VOUT_MODE: {m:02x}"),
            None => log!(Error, TAG, "Could not read VOUT_MODE"),
        }
        self.write_entire_config(bus);
        self.show_voltage_settings(bus);

        log!(Info, TAG, "-----------TIMING---------------------");
        for (c, name) in [
            (cmd::TON_DELAY, "TON_DELAY"),
            (cmd::TON_RISE, "TON_RISE"),
            (cmd::TON_MAX_FAULT_LIMIT, "TON_MAX_FAULT_LIMIT"),
        ] {
            if let Some(v) = bus.read_word(ADDR, c) {
                log!(Info, TAG, "{name}: {}", slinear11_to_int(v));
            }
        }
        if let Some(v) = bus.read_byte(ADDR, cmd::TON_MAX_FAULT_RESPONSE) {
            log!(Info, TAG, "TON_MAX_FAULT_RESPONSE: {v:02x}");
        }
        for (c, name) in [
            (cmd::TOFF_DELAY, "TOFF_DELAY"),
            (cmd::TOFF_FALL, "TOFF_FALL"),
        ] {
            if let Some(v) = bus.read_word(ADDR, c) {
                log!(Info, TAG, "{name}: {}", slinear11_to_int(v));
            }
        }
        log!(Info, TAG, "--------------------------------------");

        let mut comp = [0u8; 5];
        if !bus.read_block(ADDR, cmd::COMPENSATION_CONFIG, &mut comp) {
            log!(Error, TAG, "Failed to read COMPENSATION CONFIG");
            return -1;
        }
        log!(Info, TAG, "COMPENSATION CONFIG");
        log!(Info, TAG, "{}", hex(&comp));

        self.with(|s| s.initialized = true);
        if self.variant == Variant::Rev7 {
            self.clear_faults(bus);
        }
        0
    }

    /// Logs MFR_ID, MFR_MODEL and MFR_REVISION; the revision.
    pub fn read_mfr_info(&self, bus: &mut impl Bus) -> Option<[u8; 3]> {
        log!(Info, TAG, "Reading MFR info");
        let (mut id, mut model, mut rev) = ([0u8; 3], [0u8; 3], [0u8; 3]);
        for (c, buf, what) in [
            (cmd::MFR_ID, &mut id, "ID"),
            (cmd::MFR_MODEL, &mut model, "MODEL"),
            (cmd::MFR_REVISION, &mut rev, "REVISION"),
        ] {
            if !bus.read_block(ADDR, c, buf) {
                log!(Error, TAG, "Failed to read MFR {what}");
                return None;
            }
        }
        let text = |b: &[u8]| {
            let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
            String::from_utf8_lossy(&b[..end]).into_owned()
        };
        log!(Info, TAG, "MFR_ID: {}", text(&id));
        log!(Info, TAG, "MFR_MODEL: {}", text(&model));
        log!(Info, TAG, "MFR_REVISION: {}{}{} ", rev[0], rev[1], rev[2]);
        Some(rev)
    }

    pub fn clear_faults(&self, bus: &mut impl Bus) {
        if !bus.write_command(ADDR, cmd::CLEAR_FAULTS) {
            log!(Warn, TAG, "Could not clear TPS546 faults");
        }
    }

    /// The VOUT limits as ratios of VOUT_COMMAND (rev7); stops at the first
    /// failed write.
    fn write_vout_limit_ratios(&self, bus: &mut impl Bus, vout_command: f32) -> bool {
        for (c, ratio, name) in VOUT_LIMIT_RATIOS {
            log!(
                Info,
                TAG,
                "{name}: {}x ({}V)",
                f2(ratio),
                fixed(vout_command as f64 * ratio as f64, 2)
            );
            let ok = match from_volts(bus, ratio) {
                Some(w) => bus.write_word(ADDR, c, w),
                None => false,
            };
            if !ok {
                return false;
            }
        }
        true
    }

    /// Writes a ULINEAR16 voltage (VOUT_MODE read first).
    fn write_volts(bus: &mut impl Bus, c: u8, volts: f32) {
        match from_volts(bus, volts) {
            Some(w) => {
                bus.write_word(ADDR, c, w);
            }
            None => log!(Error, TAG, "Could not read VOUT_MODE"),
        }
    }

    /// The configuration for normal operation (into RAM only: storing it in
    /// the NVM can leave the converter unrecoverable).
    pub fn write_entire_config(&self, bus: &mut impl Bus) {
        let cfg = self.config();
        let rev7 = self.variant == Variant::Rev7;
        log!(Info, TAG, "---Writing new config values to TPS546---");
        log!(Info, TAG, "Setting ON_OFF_CONFIG");
        if !bus.write_byte(ADDR, cmd::ON_OFF_CONFIG, ON_OFF_CONFIG) {
            log!(Error, TAG, "Failed to write ON_OFF_CONFIG");
            return;
        }

        if rev7 {
            log!(Info, TAG, "Setting STACK_CONFIG: {:04X}", cfg.stack_config);
            bus.write_word(ADDR, cmd::STACK_CONFIG, cfg.stack_config);
            log!(Info, TAG, "Setting SYNC_CONFIG: {:02X}", cfg.sync_config);
            bus.write_byte(ADDR, cmd::SYNC_CONFIG, cfg.sync_config);
            log!(Info, TAG, "Setting PHASE: {:02X}", cfg.phase);
            bus.write_byte(ADDR, cmd::PHASE, cfg.phase);
        }

        log!(Info, TAG, "Setting FREQUENCY");
        bus.write_word(ADDR, cmd::FREQUENCY_SWITCH, int_to_slinear11(FREQUENCY));

        if rev7 && cfg.compensation_config.iter().all(|&b| b != 0) {
            log!(
                Info,
                TAG,
                "Setting COMPENSATION_CONFIG: {}",
                hex(&cfg.compensation_config)
            );
            bus.write_block(ADDR, cmd::COMPENSATION_CONFIG, &cfg.compensation_config);
        }

        log!(Info, TAG, "Setting VIN_ON: {}", f2(cfg.vin_on));
        bus.write_word(ADDR, cmd::VIN_ON, float_to_slinear11(cfg.vin_on));
        log!(Info, TAG, "Setting VIN_OFF: {}", f2(cfg.vin_off));
        bus.write_word(ADDR, cmd::VIN_OFF, float_to_slinear11(cfg.vin_off));
        if !rev7 || cfg.vin_uv_warn_limit > 0.0 {
            log!(
                Info,
                TAG,
                "Setting VIN_UV_WARN_LIMIT: {}",
                f2(cfg.vin_uv_warn_limit)
            );
            bus.write_word(
                ADDR,
                cmd::VIN_UV_WARN_LIMIT,
                float_to_slinear11(cfg.vin_uv_warn_limit),
            );
        }
        log!(
            Info,
            TAG,
            "Setting VIN_OV_FAULT_LIMIT: {}",
            f2(cfg.vin_ov_fault_limit)
        );
        bus.write_word(
            ADDR,
            cmd::VIN_OV_FAULT_LIMIT,
            float_to_slinear11(cfg.vin_ov_fault_limit),
        );
        log!(
            Info,
            TAG,
            "Setting VIN_OV_FAULT_RESPONSE: {VIN_OV_FAULT_RESPONSE:02X}"
        );
        bus.write_byte(ADDR, cmd::VIN_OV_FAULT_RESPONSE, VIN_OV_FAULT_RESPONSE);

        log!(Info, TAG, "Setting VOUT SCALE: {}", f2(cfg.scale_loop));
        bus.write_word(
            ADDR,
            cmd::VOUT_SCALE_LOOP,
            float_to_slinear11(cfg.scale_loop),
        );
        if rev7 {
            for (c, v, name) in [
                (cmd::VOUT_COMMAND, cfg.vout_command, "VOUT_COMMAND"),
                (cmd::VOUT_MAX, cfg.vout_max, "VOUT_MAX"),
                (cmd::VOUT_MIN, cfg.vout_min, "VOUT_MIN"),
            ] {
                log!(Info, TAG, "{name}: {}", f2(v));
                Self::write_volts(bus, c, v);
            }
            if !self.write_vout_limit_ratios(bus, cfg.vout_command) {
                log!(
                    Warn,
                    TAG,
                    "Failed to write VOUT limit ratios; continuing with existing limits"
                );
            }
        } else {
            for (c, v, name) in [
                (cmd::VOUT_COMMAND, cfg.vout_command, "VOUT_COMMAND"),
                (cmd::VOUT_MAX, cfg.vout_max, "VOUT_MAX"),
            ]
            .into_iter()
            .chain(VOUT_LIMIT_RATIOS)
            .chain([(cmd::VOUT_MIN, cfg.vout_min, "VOUT_MIN")])
            {
                log!(Info, TAG, "{name}: {}", f2(v));
                Self::write_volts(bus, c, v);
            }
        }

        log!(Info, TAG, "Setting IOUT");
        bus.write_word(
            ADDR,
            cmd::IOUT_OC_WARN_LIMIT,
            float_to_slinear11(cfg.iout_oc_warn_limit),
        );
        bus.write_word(
            ADDR,
            cmd::IOUT_OC_FAULT_LIMIT,
            float_to_slinear11(cfg.iout_oc_fault_limit),
        );
        bus.write_byte(ADDR, cmd::IOUT_OC_FAULT_RESPONSE, IOUT_OC_FAULT_RESPONSE);

        log!(Info, TAG, "Setting TEMPERATURE");
        log!(Info, TAG, "OT_WARN_LIMIT: {OT_WARN_LIMIT}");
        bus.write_word(ADDR, cmd::OT_WARN_LIMIT, int_to_slinear11(OT_WARN_LIMIT));
        log!(Info, TAG, "OT_FAULT_LIMIT: {OT_FAULT_LIMIT}");
        bus.write_word(ADDR, cmd::OT_FAULT_LIMIT, int_to_slinear11(OT_FAULT_LIMIT));
        log!(Info, TAG, "OT_FAULT_RESPONSE: {OT_FAULT_RESPONSE:02x}");
        bus.write_byte(ADDR, cmd::OT_FAULT_RESPONSE, OT_FAULT_RESPONSE);

        log!(Info, TAG, "Setting TIMING");
        log!(Info, TAG, "TON_DELAY: {TON_DELAY}");
        bus.write_word(ADDR, cmd::TON_DELAY, int_to_slinear11(TON_DELAY));
        log!(Info, TAG, "TON_RISE: {TON_RISE}");
        bus.write_word(ADDR, cmd::TON_RISE, int_to_slinear11(TON_RISE));
        log!(Info, TAG, "TON_MAX_FAULT_LIMIT: {TON_MAX_FAULT_LIMIT}");
        bus.write_word(
            ADDR,
            cmd::TON_MAX_FAULT_LIMIT,
            int_to_slinear11(TON_MAX_FAULT_LIMIT),
        );
        log!(
            Info,
            TAG,
            "TON_MAX_FAULT_RESPONSE: {TON_MAX_FAULT_RESPONSE:02x}"
        );
        bus.write_byte(ADDR, cmd::TON_MAX_FAULT_RESPONSE, TON_MAX_FAULT_RESPONSE);
        log!(Info, TAG, "TOFF_DELAY: {TOFF_DELAY}");
        bus.write_word(ADDR, cmd::TOFF_DELAY, int_to_slinear11(TOFF_DELAY));
        log!(Info, TAG, "TOFF_FALL: {TOFF_FALL}");
        bus.write_word(ADDR, cmd::TOFF_FALL, int_to_slinear11(TOFF_FALL));

        // boot behavior: pin detect values vs NVM values
        log!(Info, TAG, "Setting PIN_DETECT_OVERRIDE");
        bus.write_word(ADDR, cmd::PIN_DETECT_OVERRIDE, PIN_DETECT_OVERRIDE);

        log!(Info, TAG, "Writing MFR ID");
        bus.write_block(ADDR, cmd::MFR_ID, &MFR_ID);
        log!(Info, TAG, "Writing MFR MODEL");
        bus.write_block(ADDR, cmd::MFR_MODEL, &MFR_MODEL);
        log!(Info, TAG, "Writing MFR REVISION");
        bus.write_block(ADDR, cmd::MFR_REVISION, &MFR_REVISION);
    }

    pub fn frequency(&self, bus: &mut impl Bus) -> c_int {
        bus.read_word(ADDR, cmd::FREQUENCY_SWITCH)
            .map_or(0, slinear11_to_int)
    }

    pub fn set_frequency(&self, bus: &mut impl Bus, khz: c_int) {
        log!(Info, TAG, "Writing new frequency: {khz}");
        bus.write_word(ADDR, cmd::FREQUENCY_SWITCH, int_to_slinear11(khz));
    }

    /// An SLINEAR11 reading; 0 before `init()` or on an error.
    fn read_slinear(&self, bus: &mut impl Bus, c: u8, what: &str) -> f32 {
        if !self.initialized() {
            return 0.0;
        }
        match bus.read_word(ADDR, c) {
            Some(v) => slinear11_to_float(v),
            None => {
                if !what.is_empty() {
                    log!(Error, TAG, "Could not read {what}");
                }
                0.0
            }
        }
    }

    pub fn temperature(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear(bus, cmd::READ_TEMPERATURE_1, "")
    }

    pub fn vin(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear(bus, cmd::READ_VIN, "VIN")
    }

    pub fn iout(&self, bus: &mut impl Bus) -> f32 {
        self.read_slinear(bus, cmd::READ_IOUT, "Iout")
    }

    /// The converter's output voltage (all domains).
    pub fn vout_total(&self, bus: &mut impl Bus) -> f32 {
        if !self.initialized() {
            return 0.0;
        }
        let Some(v) = bus.read_word(ADDR, cmd::READ_VOUT) else {
            log!(Error, TAG, "Could not read Vout");
            return 0.0;
        };
        to_volts(bus, v).unwrap_or(0.0)
    }

    /// The output voltage per voltage domain.
    pub fn vout(&self, bus: &mut impl Bus) -> f32 {
        self.vout_total(bus) / self.domains
    }

    /// Output power (W).
    pub fn pout(&self, bus: &mut impl Bus) -> f32 {
        let v = self.vout_total(bus);
        v * self.iout(bus)
    }

    /// Input current estimated from the output power (A).
    pub fn iin(&self, bus: &mut impl Bus) -> f32 {
        let vin = self.vin(bus);
        if vin != 0.0 {
            self.pout(bus) / vin
        } else {
            0.0
        }
    }

    /// A status register; 0xff before `init()` or on an error.
    pub fn status_reg(&self, bus: &mut impl Bus, c: u8) -> u8 {
        if !self.initialized() {
            return 0xff;
        }
        bus.read_byte(ADDR, c).unwrap_or(0xff)
    }

    pub fn print_status(&self, bus: &mut impl Bus) {
        match bus.read_word(ADDR, cmd::STATUS_WORD) {
            Some(v) => log!(Info, TAG, "TPS546 Status: {v:04X}"),
            None => log!(Error, TAG, "Could not read STATUS_WORD"),
        }
        let mut regs = vec![(cmd::STATUS_VOUT, "VOUT"), (cmd::STATUS_INPUT, "INPUT")];
        if self.variant == Variant::Rev7 {
            regs.extend([
                (cmd::STATUS_IOUT, "IOUT"),
                (cmd::STATUS_TEMPERATURE, "TEMPERATURE"),
            ]);
        }
        for (c, name) in regs {
            match bus.read_byte(ADDR, c) {
                Some(v) => log!(Info, TAG, "TPS546 {name} Status: {v:02X}"),
                None => log!(Error, TAG, "Could not read STATUS_{name}"),
            }
        }
    }

    /// The core voltage per domain (0: output off).
    pub fn set_vout(&self, bus: &mut impl Bus, volts_per_domain: f32) -> bool {
        let volts = volts_per_domain * self.domains;
        if volts == 0.0 {
            if !bus.write_byte(ADDR, cmd::OPERATION, OPERATION_OFF) {
                log!(Error, TAG, "Could not turn off Vout");
                return false;
            }
            return true;
        }
        let cfg = self.config();
        if volts < cfg.vout_min || volts > cfg.vout_max {
            log!(
                Error,
                TAG,
                "Voltage requested ({} V) is out of range",
                fixed(volts as f64, 6)
            );
            return false;
        }
        let ok = match from_volts(bus, volts) {
            Some(value) => bus.write_word(ADDR, cmd::VOUT_COMMAND, value),
            None => false,
        };
        if !ok {
            log!(Error, TAG, "Could not set Vout to {} V", f2(volts));
            return false;
        }
        let rev7 = self.variant == Variant::Rev7;
        if rev7 && !self.write_vout_limit_ratios(bus, volts) {
            log!(
                Warn,
                TAG,
                "Could not update Vout limit ratios; continuing with existing limits"
            );
        }
        log!(Info, TAG, "Vout changed to {} V", f2(volts));
        if rev7 {
            self.clear_faults(bus);
        }
        if !bus.write_byte(ADDR, cmd::OPERATION, OPERATION_ON) {
            log!(Error, TAG, "Could not turn on Vout");
            return false;
        }
        true
    }

    /// Output off (true before `init()`).
    pub fn disable_vout(&self, bus: &mut impl Bus) -> bool {
        if !self.initialized() {
            return true;
        }
        bus.write_byte(ADDR, cmd::OPERATION, OPERATION_OFF)
    }

    pub fn show_voltage_settings(&self, bus: &mut impl Bus) {
        log!(Info, TAG, "-----------VOLTAGE---------------------");
        for (c, name) in [(cmd::VIN_ON, "VIN ON"), (cmd::VIN_OFF, "VIN OFF")] {
            if let Some(v) = bus.read_word(ADDR, c) {
                log!(Info, TAG, "{name} set to: {}", f2(slinear11_to_float(v)));
            }
        }
        for (c, name) in [
            (cmd::VOUT_MAX, "Vout Max set to"),
            (cmd::VOUT_OV_FAULT_LIMIT, "Vout OV Fault Limit"),
            (cmd::VOUT_OV_WARN_LIMIT, "Vout OV Warn Limit"),
            (cmd::VOUT_MARGIN_HIGH, "Vout Margin HIGH"),
            (cmd::VOUT_COMMAND, "Vout set to"),
            (cmd::VOUT_MARGIN_LOW, "Vout Margin LOW"),
            (cmd::VOUT_UV_WARN_LIMIT, "Vout UV Warn Limit"),
            (cmd::VOUT_UV_FAULT_LIMIT, "Vout UV Fault Limit"),
            (cmd::VOUT_MIN, "Vout Min set to"),
        ] {
            if let Some(v) = bus.read_word(ADDR, c) {
                if let Some(f) = to_volts(bus, v) {
                    log!(Info, TAG, "{name}: {} V", f2(f));
                }
            }
        }
    }
}

pub(crate) mod ffi {
    use super::*;

    /// The NerdAxe family's converter (NerdAxe defaults, one domain),
    /// created on first use.
    pub extern "C" fn tps546_dev() -> *mut Tps546 {
        static DEV: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
        *DEV.get_or_init(|| {
            Box::into_raw(Box::new(Tps546::new(
                Variant::NerdAxe,
                Config::nerdaxe(),
                1,
            ))) as usize
        }) as *mut Tps546
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions() {
        assert_eq!(int_to_slinear11(650), 650);
        assert_eq!(int_to_slinear11(1500), (1 << 11) | 750);
        assert_eq!(int_to_slinear11(-1), 0);
        // 12 V: 2^-6 steps
        let v = float_to_slinear11(12.0);
        assert_eq!(v, (((-6i32) << 11) as u16 & 0xf800) | 768);
        assert_eq!(slinear11_to_float(v), 12.0);
        assert_eq!(float_to_slinear11(0.0), 0);
        assert_eq!(float_to_slinear11(-1.0), 0);
        // VOUT_MODE 0x17: exponent -9
        assert_eq!(float_to_ulinear16(1.2, 0x17), 614);
        assert_eq!(ulinear16_to_float(512, 0x17), 1.0);
        assert_eq!(ulinear16_to_float(512, 0x01), 1024.0);
    }
}
