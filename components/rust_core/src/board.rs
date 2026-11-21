//! The boards (port of `main/boards/board.cpp` and the board classes
//! `nerdqaxeplus.cpp`, `nerdqaxeplus2.cpp`, `nerdoctaxeplus.cpp`,
//! `nerdqx.cpp`, `nerdeko.cpp`, `nerdhaxegamma.cpp`, `nerdoctaxegamma.cpp`,
//! `q1370.cpp`, `q1373.cpp`, `nerdaxe.cpp`, `nerdaxegamma.cpp`,
//! `nerdaxegaia.cpp` and the TMP451 multiplexers `tmp451_mux*.cpp`): the
//! board's constants (`board_profile.rs`) and settings, its hardware
//! (regulator, fans, temperature sensors, ethernet and CAN options, control
//! pins) on the Rust drivers, the power sequencing (`board_power.rs`). The
//! firmware is built for one board; it is chosen at runtime by its
//! `BOARD_*` id.
//!
//! The class hierarchy is flattened: each method matches the board id the
//! way the C++ overrides did.

use crate::asic::driver::api::Handle as Asics;
use crate::idf::idf_const;
use core::ffi::{c_int, CStr};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Instant;

use crate::board_core::{self, Limits};
use crate::board_power::{self as power, Params, Variant};
use crate::board_profile::{self, BoardId, Detect, Pid, Profile};
use crate::config;
use crate::fxl6408::Fxl6408;
use crate::i2c::Master;
use crate::idf::i2c::SmBus;
use crate::log::log;
use crate::tmp451::{RealClock, Tmp451};
use crate::tps536x7::{Chip, Tps536x7};
use crate::tps546::{self, Tps546};

use BoardId::*;

// control pins of the NerdQAxe family
const QAXE_VREG_EN: c_int = 10;
const QAXE_LDO_EN: c_int = 13;
const QAXE_ASIC_RST: c_int = 1;
// the NerdAxe family
const AXE_PWR_EN: c_int = 10;
const AXE_ASIC_RST: c_int = 1;
const GAIA_LDO_EN: c_int = 12;
// Q1370/Q1373: FXL6408 pins
const EXP_ASIC_RESET: u8 = 0;
const EXP_VREG: u8 = 1;
const EXP_LDO: u8 = 2;
const EXP_TMUX_A0: u8 = 3;
const EXP_TMUX_A1: u8 = 4;
/// the CAN slave DIP switch (Q1370 expander, NerdQAxe CAN extension)
const EXP_CAN_SLAVE: u8 = 5;
const Q1373_PHASE_DETECT: u8 = 6;
const Q1373_ETH_DETECT: u8 = 7;
/// Q1370: GPIO3 holds the FXL6408 in reset while high
const Q1370_IO_EXP_RESET: c_int = 3;
/// NerdOCTAXE-γ: GPIO3 high = TPS53667
const OCTAXE_VR_DETECT: c_int = 3;
/// the VR sensor among the TMP1075s of the NerdQAxe family
const VR_TMP1075: c_int = 1;
/// the TPS53647's PMBus device code (the TPS53667's is 0x01f8)
const TPS53667_DEVICE_CODE: u16 = 0x01f8;
/// `EMC2101_IDEALITY_1_0319`, `EMC2101_BETA_11`
const EMC2101_IDEALITY_1_0319: u8 = 0x24;
const EMC2101_BETA_11: u8 = 0x00;
/// PMBus status registers
const STATUS_BYTE: u8 = 0x78;
const STATUS_VOUT: u8 = 0x7a;
const STATUS_IOUT: u8 = 0x7b;
const STATUS_INPUT: u8 = 0x7c;
const STATUS_TEMPERATURE: u8 = 0x7d;
/// The NerdAxeGamma/Gaia power reading offset (W).
const AXE_POWER_OFFSET: f32 = 5.0;
/// `TPS546_THROTTLE_TEMP`
const TPS546_THROTTLE_TEMP: f32 = 105.0;
/// NerdQAxe+: chip temperatures from the ASICs (measuring is slow)
const ASIC_TEMP_PERIOD_US: i64 = 15_000_000;

/// The NerdQAxe++ rev7 (TPS546) values.
static REV7_FREQUENCIES: [u32; 23] = [
    500, 515, 525, 550, 575, 590, 600, 625, 650, 675, 700, 725, 750, 775, 800, 825, 850, 875, 900,
    925, 950, 975, 1000,
];
static REV7_VOLTAGES: [u32; 19] = [
    1120, 1130, 1140, 1150, 1160, 1170, 1180, 1190, 1200, 1210, 1220, 1230, 1240, 1250, 1260, 1270,
    1280, 1290, 1300,
];

idf_const!(GPIO_MODE_INPUT: u32 = gpio_mode_t_GPIO_MODE_INPUT, 1);
idf_const!(GPIO_MODE_OUTPUT: u32 = gpio_mode_t_GPIO_MODE_OUTPUT, 2);
idf_const!(GPIO_PULLDOWN_ONLY: u32 = gpio_pull_mode_t_GPIO_PULLDOWN_ONLY, 1);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::gpio_config_t as GpioConfig;
#[cfg(not(target_os = "espidf"))]
/// `gpio_config_t` (host model)
#[derive(Default)]
pub struct GpioConfig {
    pub pin_bit_mask: u64,
    pub mode: u32,
    pub pull_up_en: u32,
    pub pull_down_en: u32,
    pub intr_type: u32,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        gpio_config, gpio_get_level, gpio_pad_select_gpio, gpio_reset_pin, gpio_set_direction,
        gpio_set_level, gpio_set_pull_mode,
    };
}

/// On the host: no hardware.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn gpio_pad_select_gpio(_: u32) {}
    pub unsafe fn gpio_set_direction(_: c_int, _: u32) -> c_int {
        0
    }
    pub unsafe fn gpio_set_level(_: c_int, _: u32) -> c_int {
        0
    }
    pub unsafe fn gpio_get_level(_: c_int) -> c_int {
        0
    }
    pub unsafe fn gpio_reset_pin(_: c_int) -> c_int {
        0
    }
    pub unsafe fn gpio_set_pull_mode(_: c_int, _: u32) -> c_int {
        0
    }
    pub unsafe fn gpio_config(_: *const GpioConfig) -> c_int {
        0
    }
}

use crate::idf::task::delay_ms;

/// `gpio_pad_select_gpio()`, output, `level`.
fn gpio_output(pin: c_int, level: bool) {
    // SAFETY: plain ESP-IDF GPIO calls on a board pin number, no pointers
    unsafe {
        sys::gpio_pad_select_gpio(pin as u32);
        sys::gpio_set_direction(pin, GPIO_MODE_OUTPUT);
        sys::gpio_set_level(pin, level as u32);
    }
}

fn gpio_set(pin: c_int, level: bool) {
    // SAFETY: plain ESP-IDF GPIO call on a board pin number, no pointers
    unsafe { sys::gpio_set_level(pin, level as u32) };
}

/// The W5500 HAT pins: SCLK, MOSI, MISO, CS (RST and IRQ not connected).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EthPins {
    pub sclk: c_int,
    pub mosi: c_int,
    pub miso: c_int,
    pub cs: c_int,
}

/// The NerdQAxe family's HAT header (Q1370/Q1373 differ).
const QAXE_HAT: EthPins = EthPins {
    sclk: 2,
    mosi: 12,
    miso: 16,
    cs: 21,
};
const GAIA_ETH: EthPins = EthPins {
    sclk: 10,
    mosi: 3,
    miso: 16,
    cs: 21,
};

fn eth_probe(p: &EthPins) -> bool {
    crate::w5500::probe(p.sclk, p.mosi, p.miso, p.cs)
}

/// How a TMP451's multiplexer is switched.
#[derive(Clone, Copy)]
enum Select {
    /// two GPIOs (A0, A1)
    Gpio(c_int, c_int),
    /// two pins of the FXL6408
    Expander(u8, u8),
}

/// A TMP451 behind a 4-channel multiplexer (`Tmp451Mux`, `Tmp451MuxExp`),
/// active high.
struct Mux {
    dev: Tmp451,
    select: Select,
}

impl Mux {
    fn new(addr: u8, select: Select) -> Self {
        Mux {
            dev: Tmp451::new(addr),
            select,
        }
    }

    fn init(&self, io: &Fxl6408) -> bool {
        match self.select {
            Select::Gpio(a0, a1) => {
                let g = GpioConfig {
                    pin_bit_mask: (1u64 << a0) | (1u64 << a1),
                    mode: GPIO_MODE_OUTPUT,
                    ..Default::default()
                };
                // SAFETY: g is a valid config on the stack; gpio_config only reads it during the
                // call
                if unsafe { sys::gpio_config(&g) } != 0 {
                    log!(Error, c"Tmp451Mux", "gpio_config");
                    return false;
                }
            }
            Select::Expander(a0, a1) => {
                io.set_direction(&mut Master, a0, true);
                io.set_direction(&mut Master, a1, true);
            }
        }
        self.dev.init(&mut Master) == 0
    }

    fn temperature(&self, io: &Fxl6408, channel: c_int) -> f32 {
        let select = self.select;
        self.dev.temperature(
            &mut Master,
            &mut RealClock(Instant::now()),
            channel,
            &mut |ch| {
                let (b0, b1) = (ch & 1 != 0, ch & 2 != 0);
                match select {
                    Select::Gpio(a0, a1) => {
                        gpio_set(a0, b0);
                        gpio_set(a1, b1);
                    }
                    Select::Expander(a0, a1) => {
                        io.write(&mut Master, a0, b0);
                        io.write(&mut Master, a1, b1);
                    }
                }
                true
            },
        )
    }
}

/// The voltage regulator.
enum Buck {
    /// TPS53647/53667 (NerdQAxe family)
    Tps536x7(Tps536x7),
    /// the NerdQAxe++ rev7 TPS546
    Rev7(Tps546),
    /// the NerdAxe family's TPS546 (`tps546_dev()`), or none (NerdAxe)
    Axe,
}

fn axe_tps() -> &'static Tps546 {
    // SAFETY: tps546_dev() returns a leaked Box (never freed), so the pointer is valid for 'static
    unsafe { &*tps546::api::tps546_dev() }
}

/// The hardware objects and detections.
struct Hw {
    buck: Buck,
    /// the FXL6408: the Q1370's control expander or the NerdQAxe CAN
    /// extension (same address)
    io: Fxl6408,
    has_can_ext: bool,
    hat_detected: bool,
    /// Q1373 strap, NerdAxeGaia probe
    has_eth: bool,
    /// Q1373 device code, NerdOCTAXE-γ GPIO3
    tps53667: bool,
    mux: Vec<Mux>,
    has_mux: [bool; 2],
    /// NerdAxeGamma/Gaia: the TPS546 initialized
    vr_ok: bool,
    /// when the ASICs are asked for their temperatures next (NerdQAxe+)
    asic_temp_due_us: i64,
}

/// The board's state: the constants (a rev7 or a QX without its sensors
/// changes some) and the settings.
struct State {
    id: BoardId,
    p: Profile,
    asic_frequency: c_int,
    asic_voltage_mv: c_int,
    job_interval_ms: c_int,
    fan_invert: bool,
    fan_perc: f32,
    flip_screen: bool,
    vr_frequency: u32,
    fan_mode: [c_int; 2],
    pid: [Pid; 2],
    num_temp_sensors: c_int,
    chips_detected: c_int,
    initialized: bool,
    buck_initialized: bool,
    shutdown: bool,
}

// SAFETY: the profile's pointers are static strings and lists
unsafe impl Send for State {}

pub struct Board {
    s: Mutex<State>,
    hw: Mutex<Hw>,
    /// the driver of the board's ASIC chain (lives as long as the board)
    asic: OnceLock<&'static Asics>,
}

static BOARD: OnceLock<Board> = OnceLock::new();

pub fn get() -> Option<&'static Board> {
    BOARD.get()
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The log tag of the class whose method ran (the family's base class; the
/// board-specific steps log with their own tag).
fn tag(id: BoardId) -> &'static CStr {
    match id {
        Q1370 | Q1373 => c"q1370",
        NerdAxe => c"nerdaxe",
        NerdAxeGamma => c"nerdaxeGamma",
        NerdAxeGaia => c"nerdaxeGaia",
        _ => c"nerdqaxe+",
    }
}

/// The tag of the board's own class (its overrides).
fn own_tag(id: BoardId) -> &'static CStr {
    match id {
        NerdQX => c"NerdQX",
        NerdOctaxeGamma => c"nerdoctaxegamma",
        _ => tag(id),
    }
}

fn is_axe(id: BoardId) -> bool {
    matches!(id, NerdAxe | NerdAxeGamma | NerdAxeGaia)
}

fn is_q1370(id: BoardId) -> bool {
    matches!(id, Q1370 | Q1373)
}

/// The NerdQAxe++ and its derivatives (BM1370, chip temperature estimate,
/// rev7 probe).
fn is_plus2_family(id: BoardId) -> bool {
    matches!(
        id,
        NerdQAxePlus2 | NerdQX | NerdEko | NerdHaxeGamma | NerdOctaxeGamma
    )
}

/// The default fan mode of channel 0 (`CONFIG_AUTO_FAN_SPEED_VALUE`).
fn kconfig_fan_mode() -> c_int {
    use crate::sdkconfig as k;
    if k::FAN_MODE_MANUAL {
        0
    } else if k::FAN_MODE_PID {
        2
    } else {
        1
    }
}

fn cfg_u16(key: &[u8], def: c_int) -> c_int {
    config::get_u16(key, def as u16) as c_int
}

impl State {
    fn new(id: BoardId, p: Profile) -> Self {
        State {
            id,
            p,
            asic_frequency: p.default_frequency,
            asic_voltage_mv: p.default_voltage_mv,
            job_interval_ms: p.job_interval_ms,
            fan_invert: p.fan_invert_polarity,
            fan_perc: p.fan_perc,
            flip_screen: p.flip_screen,
            vr_frequency: p.vr_frequency,
            fan_mode: [
                if p.fan_mode0 < 0 {
                    kconfig_fan_mode()
                } else {
                    p.fan_mode0
                },
                3,
            ],
            pid: p.pid,
            num_temp_sensors: p.num_temp_sensors,
            chips_detected: 0,
            initialized: false,
            buck_initialized: false,
            shutdown: false,
        }
    }

    fn limits(&self) -> Limits {
        Limits {
            abs_max_frequency: self.p.abs_max_frequency,
            abs_min_voltage_mv: self.p.abs_min_voltage_mv,
            abs_max_voltage_mv: self.p.abs_max_voltage_mv,
        }
    }

    /// `Board::loadSettings()`
    fn load_settings(&mut self) {
        const TAG: &CStr = c"board";
        self.fan_perc = cfg_u16(b"fanspeed", crate::sdkconfig::FAN_SPEED as c_int) as f32;
        let l = self.limits();
        let (f, v) = board_core::clamp_settings(
            cfg_u16(b"asicfrequency", self.asic_frequency) as u16,
            cfg_u16(b"asicvoltage", self.asic_voltage_mv) as u16,
            &l,
        );
        self.asic_frequency = f;
        self.asic_voltage_mv = v;
        self.job_interval_ms = cfg_u16(b"asicjobinterval", self.job_interval_ms);
        self.fan_invert = cfg_u16(b"pwmfanpol", self.fan_invert as c_int) != 0;
        self.flip_screen = cfg_u16(b"flipscreen", self.flip_screen as c_int) != 0;
        self.vr_frequency = config::get_u64(b"vr_frequency", self.p.vr_frequency as u64) as u32;
        let keys: [[&[u8]; 5]; 2] = [
            [b"autofanspeed", b"pid_temp", b"pid_p", b"pid_i", b"pid_d"],
            [
                b"fan1mode",
                b"fan1_pid_temp",
                b"fan1_pid_p",
                b"fan1_pid_i",
                b"fan1_pid_d",
            ],
        ];
        for (ch, k) in keys.iter().enumerate() {
            self.fan_mode[ch] = cfg_u16(k[0], self.fan_mode[ch]);
            let pid = &mut self.pid[ch];
            pid.target_temp = config::get_u16(k[1], pid.target_temp);
            pid.p = config::get_u16(k[2], pid.p);
            pid.i = config::get_u16(k[3], pid.i);
            pid.d = config::get_u16(k[4], pid.d);
        }
        log!(Info, TAG, "ASIC Frequency: {}MHz", self.asic_frequency);
        log!(Info, TAG, "ASIC voltage: {}mV", self.asic_voltage_mv);
        log!(Info, TAG, "ASIC job interval: {}ms", self.job_interval_ms);
        log!(Info, TAG, "invert fan polarity: {}", self.fan_invert);
        log!(Info, TAG, "fan speed: {}%", self.fan_perc as c_int);
    }

    /// The NerdQAxe++ rev7 values (`applyRev7Profile()`).
    fn apply_rev7(&mut self) {
        self.p.version = 502;
        self.p.vr_max_temp = TPS546_THROTTLE_TEMP;
        self.p.max_pin = 120.0;
        self.p.min_pin = 45.0;
        self.p.max_current_a = 10.0;
        self.p.abs_max_frequency = 1000;
        self.p.frequencies = &REV7_FREQUENCIES;
        self.p.voltages = &REV7_VOLTAGES;
        self.p.default_voltage_mv = 1200;
        self.asic_voltage_mv = 1200;
        self.pid[0].target_temp = 65;
        if !config::has(b"autofanspeed") {
            config::set(
                b"autofanspeed",
                crate::ajson::Json::Num(crate::ajson::Num::U(2)),
            );
        }
    }

    fn variant(&self, hw: &Hw) -> Variant {
        match self.id {
            NerdAxe => Variant::Axe,
            NerdAxeGamma => Variant::AxeGamma,
            NerdAxeGaia => Variant::AxeGaia,
            _ if matches!(hw.buck, Buck::Rev7(_)) => Variant::Rev7,
            _ => Variant::Classic,
        }
    }
}

/// The hardware steps of the power sequencing.
struct PowerHw<'a> {
    b: &'a Board,
    id: BoardId,
    hw: &'a mut Hw,
}

impl power::Hw for PowerHw<'_> {
    fn vreg(&mut self, on: bool) {
        match self.id {
            id if is_axe(id) => gpio_set(AXE_PWR_EN, !on),
            id if is_q1370(id) => {
                self.hw.io.write(&mut Master, EXP_VREG, on);
            }
            _ => gpio_set(QAXE_VREG_EN, on),
        }
    }
    fn ldo(&mut self, on: bool) {
        match self.id {
            NerdAxe | NerdAxeGamma => {}
            NerdAxeGaia => gaia_ldo(on),
            id if is_q1370(id) => {
                self.hw.io.write(&mut Master, EXP_LDO, on);
            }
            id => {
                log!(
                    Info,
                    tag(id),
                    "{}",
                    if on { "Enabled LDOs" } else { "Disable LDOs" }
                );
                gpio_set(QAXE_LDO_EN, on);
            }
        }
    }
    fn asic_reset(&mut self, released: bool) {
        if is_q1370(self.id) {
            self.hw.io.write(&mut Master, EXP_ASIC_RESET, released);
        } else if is_axe(self.id) {
            gpio_set(AXE_ASIC_RST, released);
        } else {
            gpio_set(QAXE_ASIC_RST, released);
        }
    }
    fn delay_ms(&mut self, ms: u32) {
        delay_ms(ms)
    }
    fn buck_init(&mut self, phases: c_int, imax: c_int, ifault: f32) -> bool {
        match &mut self.hw.buck {
            Buck::Tps536x7(d) => d.init(&mut SmBus, phases, imax, ifault),
            Buck::Rev7(d) => d.init(&mut SmBus) == 0,
            Buck::Axe => true,
        }
    }
    fn buck_set_vout(&mut self, volts: f32) -> bool {
        match &self.hw.buck {
            Buck::Tps536x7(d) => d.set_vout(&mut SmBus, volts),
            Buck::Rev7(d) => d.set_vout(&mut SmBus, volts),
            Buck::Axe => axe_tps().set_vout(&mut SmBus, volts),
        }
    }
    fn buck_disable(&mut self) -> bool {
        match &self.hw.buck {
            // the TPS53647 has no disable (BuckConverter::disable_vout())
            Buck::Tps536x7(_) => true,
            Buck::Rev7(d) => d.disable_vout(&mut SmBus),
            Buck::Axe => axe_tps().set_vout(&mut SmBus, 0.0),
        }
    }
    fn dac_set(&mut self, code: u8) {
        if is_axe(self.id) {
            crate::ds4432u::set_code(&mut crate::i2c::Master, 0, code);
        }
    }
    fn serial_clear(&mut self) {
        crate::serial::clear()
    }
    fn serial_set_baud(&mut self, baud: c_int) {
        crate::serial::set_baud(baud)
    }
    fn asic_init(
        &mut self,
        frequency: c_int,
        count: c_int,
        max_difficulty: u32,
        vr_frequency: u32,
    ) -> c_int {
        self.b.with_asic(0, |a| {
            a.run(|d, u| {
                d.init(
                    u,
                    frequency as u64,
                    count as u16,
                    max_difficulty,
                    vr_frequency,
                )
            }) as c_int
        })
    }
    fn asic_max_baud(&mut self) -> c_int {
        self.b.with_asic(0, |a| a.run(|d, u| d.set_max_baud(u)))
    }
    fn state(&mut self, what: c_int, value: c_int) {
        let mut s = self.b.state();
        match what {
            power::STATE_BUCK_INITIALIZED => s.buck_initialized = true,
            power::STATE_CHIPS_DETECTED => s.chips_detected = value,
            power::STATE_INITIALIZED => s.initialized = true,
            _ => s.shutdown = true,
        }
    }
}

fn gaia_ldo(on: bool) {
    log!(
        Info,
        tag(NerdAxeGaia),
        "{}",
        if on { "Enable LDO" } else { "Disable LDO" }
    );
    gpio_set(GAIA_LDO_EN, on);
}

impl Board {
    /// Creates the board `id` (the C++ constructors): profile, detections.
    fn new(id: BoardId) -> Board {
        let mut detect = Detect::default();
        let mut hw = Hw {
            buck: Buck::Axe,
            io: Fxl6408::default(),
            has_can_ext: false,
            hat_detected: false,
            has_eth: false,
            tps53667: false,
            mux: Vec::new(),
            has_mux: [false; 2],
            vr_ok: true,
            asic_temp_due_us: 0,
        };
        match id {
            Q1370 => q1370_release_expander(),
            Q1373 => {
                q1370_release_expander();
                let (six_phase, eth, tps53667) = q1373_detect(&hw.io);
                detect = Detect {
                    tps53667,
                    six_phase,
                };
                hw.has_eth = eth;
                hw.tps53667 = tps53667;
                if six_phase && !tps53667 {
                    log!(
                        Error,
                        c"q1373",
                        "6-phase strap set but no TPS53667 found; staying with 4-phase TPS53647"
                    );
                }
            }
            NerdOctaxeGamma => {
                // GPIO3 high: TPS53667 (6 phases), low or open: TPS53647
                // SAFETY: plain ESP-IDF GPIO calls on a board pin number, no pointers
                unsafe {
                    sys::gpio_reset_pin(OCTAXE_VR_DETECT);
                    sys::gpio_set_direction(OCTAXE_VR_DETECT, GPIO_MODE_INPUT);
                    sys::gpio_set_pull_mode(OCTAXE_VR_DETECT, GPIO_PULLDOWN_ONLY);
                }
                delay_ms(1);
                // SAFETY: plain ESP-IDF GPIO call on a board pin number, no pointers
                hw.tps53667 = unsafe { sys::gpio_get_level(OCTAXE_VR_DETECT) } != 0;
                detect.tps53667 = hw.tps53667;
            }
            _ => {}
        }
        let p = board_profile::profile(id, &detect);

        if !is_axe(id) {
            let chip = match id {
                NerdEko => Chip::Tps53667 {
                    no_phase_shedding: false,
                },
                Q1373 | NerdOctaxeGamma if hw.tps53667 => Chip::Tps53667 {
                    no_phase_shedding: true,
                },
                _ => Chip::Tps53647,
            };
            hw.buck = Buck::Tps536x7(Tps536x7::new(chip));
        }
        if id == NerdOctaxeGamma {
            if hw.tps53667 {
                log!(
                    Info,
                    own_tag(id),
                    "TPS53667 voltage regulator detected (GPIO3=HIGH, 6 phases, 240A max with 24.9kΩ resistor)"
                );
            } else {
                log!(
                    Info,
                    own_tag(id),
                    "TPS53647 voltage regulator detected (GPIO3=LOW, 4 phases, using inherited)"
                );
            }
        }
        hw.mux = match id {
            NerdQX => vec![Mux::new(0x4c, Select::Gpio(2, 3))],
            NerdOctaxeGamma => vec![
                Mux::new(0x4c, Select::Gpio(2, 12)),
                Mux::new(0x4e, Select::Gpio(2, 12)),
            ],
            Q1370 | Q1373 => vec![Mux::new(0x4c, Select::Expander(EXP_TMUX_A0, EXP_TMUX_A1))],
            _ => Vec::new(),
        };
        if id == NerdAxeGaia {
            hw.has_eth = eth_probe(&GAIA_ETH);
        }
        Board {
            s: Mutex::new(State::new(id, p)),
            hw: Mutex::new(hw),
            asic: OnceLock::new(),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        lock(&self.s)
    }

    fn hw(&self) -> MutexGuard<'_, Hw> {
        lock(&self.hw)
    }

    fn id(&self) -> BoardId {
        self.state().id
    }

    fn with_asic<T>(&self, def: T, f: impl FnOnce(&Asics) -> T) -> T {
        match self.asic.get() {
            Some(a) => f(a),
            None => def,
        }
    }

    /// The ASIC driver once the ASICs are initialized (`getAsics()`).
    pub fn asic_handle(&self) -> Option<&'static Asics> {
        if !self.state().initialized {
            return None;
        }
        self.asic.get().copied()
    }

    /// The ASIC's small core count (0 before the ASICs are initialized).
    pub fn small_core_count(&self) -> u16 {
        if !self.state().initialized {
            return 0;
        }
        self.with_asic(0, |a| a.driver().chip().small_core_count())
    }

    pub fn load_settings(&self) {
        self.state().load_settings();
    }

    /// `initBoard()`
    pub fn init(&self) -> bool {
        let (id, asic_count, fan_invert, fan_perc) = {
            let s = self.state();
            (s.id, s.p.asic_count, s.fan_invert, s.fan_perc)
        };
        let t = tag(id);
        board_core::CHIP_TEMPS.init(asic_count);

        if is_axe(id) {
            return self.init_axe(id, fan_invert, fan_perc);
        }

        crate::serial::init();
        if crate::idf::i2c::init() != 0 {
            log!(Error, t, "I2C initializing failed");
            return false;
        }
        let n = detect_num_temp_sensors(t);
        self.state().num_temp_sensors = n;
        log!(Info, t, "found {n} ASIC temp measuring sensors");

        if is_q1370(id) {
            if !self.init_q1370(fan_invert, fan_perc) {
                return false;
            }
            if id == Q1373 {
                let hw = self.hw();
                if let Some(m) = hw.mux.first() {
                    m.dev.set_cal(1.06, [-25.4; 4]);
                }
            }
            return true;
        }

        // the CAN extension: FXL6408 + transceiver on GPIO21/16; pin 5 low
        // (DIP switch) = slave
        {
            let mut hw = self.hw();
            if hw.io.init(&mut Master, &mut std::thread::sleep) {
                hw.has_can_ext = true;
                hw.io.set_direction(&mut Master, EXP_CAN_SLAVE, false);
                hw.io.enable_pull_up(&mut Master, EXP_CAN_SLAVE);
                log!(Info, t, "CAN extension board detected");
            } else {
                log!(Info, t, "No CAN extension board");
            }
        }
        crate::emc2302::init(&mut crate::i2c::Master, fan_invert);
        self.set_fan_speed(fan_perc);
        self.set_fan_speed(fan_perc);
        gpio_output(QAXE_VREG_EN, false);
        gpio_output(QAXE_LDO_EN, false);
        gpio_output(QAXE_ASIC_RST, false);

        match id {
            // NerdQX and NerdOCTAXE-γ skip the rev7 probe (they called
            // NerdQaxePlus::initBoard())
            NerdQAxePlus2 | NerdEko | NerdHaxeGamma => {
                if tps546::probe(&mut SmBus) {
                    log!(
                        Info,
                        c"nerdqaxeplus2",
                        "NerdQAxe++ rev7 TPS546 regulator detected"
                    );
                    let mut hw = self.hw();
                    if !matches!(hw.buck, Buck::Rev7(_)) {
                        let mut c = tps546::Config::rev7_dual();
                        c.vout_min = 2.0;
                        c.vout_command = 2.4;
                        hw.buck = Buck::Rev7(Tps546::new(tps546::Variant::Rev7, c, 2));
                        drop(hw);
                        let mut s = self.state();
                        s.apply_rev7();
                        s.load_settings();
                    }
                }
            }
            NerdQX => {
                let ok = {
                    let mut hw = self.hw();
                    let ok = hw.mux[0].init(&hw.io);
                    hw.has_mux[0] = ok;
                    ok
                };
                if !ok {
                    log!(
                        Error,
                        own_tag(id),
                        "TMUX probe failed. Assuming non-QX board; applying safety limits."
                    );
                    let mut s = self.state();
                    s.p.abs_max_voltage_mv = 1150;
                    s.p.abs_max_frequency = 495;
                    s.load_settings();
                }
            }
            NerdOctaxeGamma => {
                let mut hw = self.hw();
                for i in 0..2 {
                    let ok = hw.mux[i].init(&hw.io);
                    hw.has_mux[i] = ok;
                    let (addr, range) = if i == 0 {
                        ("0x4c", "0-3")
                    } else {
                        ("0x4e", "4-7")
                    };
                    if ok {
                        log!(
                            Info,
                            own_tag(id),
                            "TMP451 mux {i} found (ASICs {range}, addr {addr})"
                        );
                    } else {
                        log!(
                            Warn,
                            own_tag(id),
                            "TMP451 mux {i} not found (ASICs {range})"
                        );
                    }
                }
            }
            _ => {}
        }
        true
    }

    fn init_q1370(&self, fan_invert: bool, fan_perc: f32) -> bool {
        let t = tag(self.id());
        crate::emc2302::init(&mut crate::i2c::Master, fan_invert);
        self.set_fan_speed(fan_perc);
        self.set_fan_speed(fan_perc);
        let mut hw = self.hw();
        let ok = hw.mux[0].init(&hw.io);
        hw.has_mux[0] = ok;
        if !hw.io.init(&mut Master, &mut std::thread::sleep) {
            log!(Error, t, "FXL6408 failed init");
            return false;
        }
        for pin in [EXP_VREG, EXP_LDO, EXP_ASIC_RESET] {
            hw.io.set_direction(&mut Master, pin, true);
            hw.io.write(&mut Master, pin, false);
        }
        // CAN slave detect: input with pull-up, the DIP switch pulls it low
        hw.io.set_direction(&mut Master, EXP_CAN_SLAVE, false);
        hw.io.enable_pull_up(&mut Master, EXP_CAN_SLAVE);
        true
    }

    fn init_axe(&self, id: BoardId, fan_invert: bool, fan_perc: f32) -> bool {
        let t = tag(id);
        if id == NerdAxeGaia {
            // SAFETY: plain ESP-IDF GPIO calls on a board pin number, no pointers
            unsafe {
                sys::gpio_pad_select_gpio(GAIA_LDO_EN as u32);
                sys::gpio_set_direction(GAIA_LDO_EN, GPIO_MODE_OUTPUT);
            }
            gaia_ldo(true);
            // let the LDO rail settle before I2C
            delay_ms(100);
        }
        crate::idf::adc::init();
        crate::serial::init();
        if crate::idf::i2c::init() != 0 {
            log!(Error, t, "I2C initializing failed");
            return false;
        }
        crate::emc2101::init(&mut crate::i2c::Master, fan_invert);
        if id == NerdAxe {
            self.set_fan_speed(fan_perc);
            gpio_output(AXE_PWR_EN, true);
            gpio_output(AXE_ASIC_RST, false);
            return true;
        }
        crate::emc2101::set_ideality_factor(&mut crate::i2c::Master, EMC2101_IDEALITY_1_0319);
        crate::emc2101::set_beta_compensation(&mut crate::i2c::Master, EMC2101_BETA_11);
        self.set_fan_speed(fan_perc);
        let tps = axe_tps();
        if id == NerdAxeGaia {
            tps.set_vin_config(10.5, 9.5, 10.5, 14.0);
            tps.set_iout_config(28.0, 33.0);
        }
        if tps.init(&mut SmBus) != 0 {
            log!(Error, t, "TPS546 init failed!");
            self.hw().vr_ok = false;
            return false;
        }
        if id == NerdAxeGaia {
            tps.set_frequency(&mut SmBus, 400);
        }
        self.set_voltage(0.0);
        gpio_output(AXE_ASIC_RST, false);
        true
    }

    /// `initAsics()`
    pub fn init_asics(&self) -> bool {
        let (id, params) = {
            let s = self.state();
            let init_voltage_mv = match s.id {
                NerdAxe => s.asic_voltage_mv,
                _ => s.p.init_voltage_mv,
            };
            let params = Params {
                init_voltage_mv,
                asic_voltage_mv: s.asic_voltage_mv,
                num_phases: s.p.num_phases,
                imax: s.p.imax,
                ifault: s.p.ifault,
                asic_frequency: s.asic_frequency,
                asic_count: s.p.asic_count,
                max_difficulty: s.p.max_difficulty,
                vr_frequency: s.vr_frequency,
                limits: s.limits(),
            };
            (s.id, params)
        };
        let mut hw = self.hw();
        if is_axe(id) && !hw.vr_ok {
            log!(
                Error,
                tag(id),
                "voltage regulator not initialized, not powering the ASICs"
            );
            return false;
        }
        let v = self.state().variant(&hw);
        power::init_asics(
            &mut PowerHw {
                b: self,
                id,
                hw: &mut hw,
            },
            v,
            &params,
        )
    }

    pub fn set_voltage(&self, volts: f32) -> bool {
        let (id, l) = {
            let s = self.state();
            (s.id, s.limits())
        };
        let mut hw = self.hw();
        let v = self.state().variant(&hw);
        power::set_voltage(
            &mut PowerHw {
                b: self,
                id,
                hw: &mut hw,
            },
            v,
            volts,
            &l,
        )
    }

    pub fn shutdown(&self) {
        let (id, l) = {
            let s = self.state();
            (s.id, s.limits())
        };
        let mut hw = self.hw();
        let v = self.state().variant(&hw);
        power::shutdown(
            &mut PowerHw {
                b: self,
                id,
                hw: &mut hw,
            },
            v,
            &l,
        );
    }

    pub fn set_asic_frequency(&self, mhz: f32) -> bool {
        let l = self.state().limits();
        if !board_core::validate_frequency(mhz, &l) {
            log!(
                Error,
                c"board",
                "Validation error. ASIC Frequency {} is higher than absolute maximum value {}",
                crate::cutil::fixed(mhz as f64, 3),
                crate::cutil::fixed(l.abs_max_frequency as f64, 3)
            );
            return false;
        }
        self.with_asic(false, |a| a.run(|d, u| d.set_frequency(u, mhz)))
    }

    pub fn set_vr_frequency(&self, hz: u32) {
        self.with_asic((), |a| a.run(|d, u| d.set_vr_frequency(u, hz)))
    }

    pub fn num_fans(&self) -> c_int {
        self.state().p.num_fans
    }

    pub fn set_fan_speed_ch(&self, ch: c_int, perc: f32) {
        if is_axe(self.id()) {
            if ch == 0 {
                crate::emc2101::set_speed(&mut crate::i2c::Master, perc);
            }
        } else {
            crate::emc2302::set_speed(&mut crate::i2c::Master, ch, perc);
        }
    }

    pub fn set_fan_speed(&self, perc: f32) {
        for ch in 0..self.num_fans() {
            self.set_fan_speed_ch(ch, perc);
        }
    }

    pub fn fan_rpm(&self, ch: c_int) -> u16 {
        if is_axe(self.id()) {
            crate::emc2101::speed(&mut crate::i2c::Master)
        } else {
            crate::emc2302::speed(&mut crate::i2c::Master, ch).unwrap_or(0)
        }
    }

    pub fn set_fan_polarity(&self, invert: bool) {
        if is_axe(self.id()) {
            crate::emc2101::set_polarity(&mut crate::i2c::Master, invert);
        } else {
            crate::emc2302::set_polarity(&mut crate::i2c::Master, invert);
        }
    }

    /// `getTemperature()`: the board's sensor `index`.
    pub fn temperature(&self, index: c_int) -> f32 {
        let (id, n, initialized) = {
            let s = self.state();
            (s.id, s.num_temp_sensors, s.initialized)
        };
        match id {
            NerdAxe => {
                if index > 0 {
                    0.0
                } else {
                    crate::emc2101::internal_temp(&mut crate::i2c::Master) as f32 + 5.0
                }
            }
            NerdAxeGamma | NerdAxeGaia => {
                if !initialized {
                    return crate::emc2101::internal_temp(&mut crate::i2c::Master) as f32 + 5.0;
                }
                if index > 0 {
                    return 0.0;
                }
                let t = crate::emc2101::external_temp(&mut crate::i2c::Master);
                log!(
                    Info,
                    tag(id),
                    "Read ASIC temp = {}ºC",
                    crate::cutil::fixed(t as f64, 3)
                );
                t
            }
            _ => {
                if index >= n {
                    return 0.0;
                }
                // the sensor at index 1 is the VR's
                let t = crate::tmp1075::read_temperature(index + (index != 0) as c_int);
                // the chips are hotter than the board sensors
                let offset = if is_q1370(id) {
                    3.0
                } else if is_plus2_family(id) {
                    10.0
                } else {
                    return t;
                };
                if t == 0.0 {
                    0.0
                } else {
                    t + offset
                }
            }
        }
    }

    pub fn vr_temp(&self) -> f32 {
        let id = self.id();
        match id {
            NerdAxe => 0.0,
            NerdAxeGamma | NerdAxeGaia => {
                let t = axe_tps().temperature(&mut SmBus);
                log!(
                    Info,
                    tag(id),
                    "Read vr temp = {}ºC",
                    crate::cutil::fixed(t as f64, 3)
                );
                t
            }
            _ => {
                let hw = self.hw();
                let t = match &hw.buck {
                    Buck::Rev7(d) => d.temperature(&mut SmBus),
                    _ => crate::tmp1075::read_temperature(VR_TMP1075),
                };
                // the TPS53667 reads 8 °C low on the NerdOCTAXE-γ
                if id == NerdOctaxeGamma && hw.tps53667 {
                    t + 8.0
                } else {
                    t
                }
            }
        }
    }

    /// The regulator's own temperature (NerdQAxe family).
    pub fn vr_temp_internal(&self) -> f32 {
        match &self.hw().buck {
            Buck::Tps536x7(d) => d.temperature(&mut SmBus),
            Buck::Rev7(d) => d.temperature(&mut SmBus),
            Buck::Axe => 0.0,
        }
    }

    /// 0 vin, 1 iin, 2 pin, 3 vout, 4 iout, 5 pout
    pub fn power(&self, what: c_int) -> f32 {
        let id = self.id();
        match id {
            NerdAxe => match what {
                0 => crate::ina260::voltage(&mut crate::i2c::Master) / 1000.0,
                1 => crate::ina260::current(&mut crate::i2c::Master) / 1000.0,
                2 => crate::ina260::power(&mut crate::i2c::Master) / 1000.0,
                3 => crate::idf::adc::vcore_mv() as f32 / 1000.0,
                _ => 0.0,
            },
            NerdAxeGamma | NerdAxeGaia => {
                let tps = axe_tps();
                let pin = || tps.vout(&mut SmBus) * tps.iout(&mut SmBus) + AXE_POWER_OFFSET;
                match what {
                    0 => tps.vin(&mut SmBus),
                    1 => {
                        let vin = tps.vin(&mut SmBus);
                        if vin == 0.0 {
                            0.0
                        } else {
                            pin() / vin
                        }
                    }
                    3 => crate::idf::adc::vcore_mv() as f32 / 1000.0,
                    4 => tps.iout(&mut SmBus),
                    _ => pin(),
                }
            }
            _ => match &self.hw().buck {
                Buck::Tps536x7(d) => match what {
                    0 => d.vin(&mut SmBus),
                    1 => d.iin(&mut SmBus),
                    2 => d.pin(&mut SmBus),
                    3 => d.vout(&mut SmBus),
                    4 => d.iout(&mut SmBus),
                    _ => d.pout(&mut SmBus),
                },
                Buck::Rev7(d) => match what {
                    0 => d.vin(&mut SmBus),
                    1 => d.iin(&mut SmBus),
                    3 => d.vout(&mut SmBus),
                    4 => d.iout(&mut SmBus),
                    // the rev7 reports Pout as Pin
                    _ => d.pout(&mut SmBus),
                },
                Buck::Axe => 0.0,
            },
        }
    }

    pub fn request_buck_telemetry(&self) {
        match &self.hw().buck {
            Buck::Tps536x7(d) => d.status(&mut SmBus),
            Buck::Rev7(d) => d.print_status(&mut SmBus),
            Buck::Axe => {}
        }
    }

    /// Reads the chip temperatures where the board can (TMP451 per chip,
    /// or every 15 s from the ASICs on the NerdQAxe+).
    pub fn request_chip_temps(&self) {
        let (id, count, shutdown) = {
            let s = self.state();
            (s.id, s.p.asic_count, s.shutdown)
        };
        let reads = matches!(
            id,
            NerdQAxePlus | NerdOctaxePlus | NerdQX | NerdOctaxeGamma | Q1370 | Q1373
        );
        if !reads {
            return;
        }
        if (id == NerdQAxePlus || id == NerdOctaxePlus) && self.asic.get().is_none() {
            return;
        }
        // in shutdown the LDOs are off: no stale values
        if shutdown {
            for i in 0..count {
                board_core::CHIP_TEMPS.set(i, 0.0);
            }
            return;
        }
        let mut hw = self.hw();
        match id {
            NerdQAxePlus | NerdOctaxePlus => {
                let now = crate::cutil::now_us();
                if hw.asic_temp_due_us == 0 {
                    hw.asic_temp_due_us = now + ASIC_TEMP_PERIOD_US;
                }
                if now >= hw.asic_temp_due_us {
                    while now >= hw.asic_temp_due_us {
                        hw.asic_temp_due_us += ASIC_TEMP_PERIOD_US;
                    }
                    drop(hw);
                    self.with_asic((), |a| a.run(|d, u| d.request_chip_temp(u)));
                }
            }
            NerdOctaxeGamma => {
                // two muxes with 4 chips each, the same select lines
                for mux in 0..2 {
                    if !hw.has_mux[mux] {
                        continue;
                    }
                    for ch in 0..4 {
                        let asic = (mux * 4 + ch) as c_int;
                        if asic >= count {
                            break;
                        }
                        let t = hw.mux[mux].temperature(&hw.io, ch as c_int);
                        if !t.is_nan() {
                            board_core::CHIP_TEMPS.set(asic, t);
                        }
                    }
                }
            }
            _ => {
                if !hw.has_mux[0] {
                    log!(Error, own_tag(id), "TMUX not detected.");
                    return;
                }
                for i in 0..count {
                    let t = hw.mux[0].temperature(&hw.io, i);
                    if !t.is_nan() {
                        board_core::CHIP_TEMPS.set(i, t);
                    }
                }
            }
        }
    }

    /// The regulator's PMBus fault (`Board::Error`, its status into the
    /// second value).
    pub fn fault(&self) -> (c_int, u32) {
        let hw = self.hw();
        let regs = |r: &dyn Fn(u8) -> u8| {
            board_core::pmbus_fault(
                r(STATUS_BYTE),
                r(STATUS_IOUT),
                r(STATUS_VOUT),
                r(STATUS_INPUT),
                r(STATUS_TEMPERATURE),
            )
        };
        match &hw.buck {
            Buck::Tps536x7(d) => regs(&|reg| d.status_reg(&mut SmBus, reg)),
            Buck::Rev7(d) => regs(&|reg| d.status_reg(&mut SmBus, reg)),
            Buck::Axe => (0, 0),
        }
    }

    pub fn has_ethernet(&self) -> bool {
        let id = self.id();
        let hw = self.hw();
        match id {
            Q1370 => true,
            Q1373 | NerdAxeGaia => hw.has_eth,
            NerdAxe | NerdAxeGamma => false,
            _ => hw.hat_detected,
        }
    }

    /// The W5500 pins of an ethernet HAT; None without one.
    pub fn eth_pins(&self) -> Option<EthPins> {
        let id = self.id();
        let hw = self.hw();
        match id {
            NerdAxeGaia => Some(GAIA_ETH),
            NerdAxe | NerdAxeGamma | Q1370 | Q1373 => None,
            // the CAN HAT uses GPIO16/21; the temperature muxes' select
            // lines are GPIO2/12 (SCLK/MOSI)
            _ if hw.has_can_ext => None,
            NerdQX if hw.has_mux[0] => None,
            NerdOctaxeGamma if hw.has_mux[0] || hw.has_mux[1] => None,
            _ => Some(QAXE_HAT),
        }
    }

    /// `detectEthernet()`: probes the HAT once everything sharing its pins
    /// is known.
    pub fn detect_ethernet(&self) {
        if is_axe(self.id()) {
            return;
        }
        let pins = self.eth_pins();
        let found = pins.is_some_and(|p| eth_probe(&p));
        self.hw().hat_detected = found;
    }

    pub fn has_can_extension(&self) -> bool {
        match self.id() {
            Q1370 | Q1373 => true,
            id if is_axe(id) => false,
            _ => self.hw().has_can_ext,
        }
    }

    pub fn is_can_slave(&self) -> bool {
        let id = self.id();
        if is_axe(id) {
            return false;
        }
        let hw = self.hw();
        let ext = !is_q1370(id);
        if ext && !hw.has_can_ext {
            return false;
        }
        match hw.io.read_pin(&mut Master, EXP_CAN_SLAVE) {
            Ok(level) => {
                let what = if level { "master" } else { "slave" };
                if ext {
                    log!(
                        Info,
                        tag(id),
                        "CAN extension slave detect pin: {} ({what})",
                        level as u8
                    );
                } else {
                    log!(
                        Info,
                        tag(id),
                        "CAN slave detect pin: {} ({what})",
                        level as u8
                    );
                }
                !level
            }
            Err(_) => {
                if ext {
                    log!(
                        Error,
                        tag(id),
                        "CAN extension: failed to read slave detect pin"
                    );
                } else {
                    log!(Error, tag(id), "failed to read CAN slave detect pin");
                }
                false
            }
        }
    }

    /// The CAN transceiver pins (TX, RX); -1 without.
    pub fn can_pins(&self) -> (c_int, c_int) {
        match self.id() {
            Q1370 | Q1373 => (1, 10),
            id if is_axe(id) => (-1, -1),
            _ => (21, 16),
        }
    }
}

/// Q1370/Q1373: drive GPIO3 low to release the FXL6408 from reset.
fn q1370_release_expander() {
    gpio_output(Q1370_IO_EXP_RESET, false);
    delay_ms(2);
}

/// Q1373: the straps on the FXL6408 and the regulator's device code; (six
/// phases, ethernet, TPS53667). Defaults without straps: 4 phases,
/// ethernet.
fn q1373_detect(io: &Fxl6408) -> (bool, bool, bool) {
    let t = c"q1373";
    let (mut six, mut eth) = (false, true);
    // I2C and the expander are needed here already; init() initializes
    // them again later
    if crate::idf::i2c::init() != 0 {
        log!(
            Error,
            t,
            "I2C init failed; keeping defaults (4 phases, ethernet)"
        );
        return (six, eth, false);
    }
    if !io.init(&mut Master, &mut std::thread::sleep) {
        log!(
            Error,
            t,
            "FXL6408 init failed; keeping defaults (4 phases, ethernet)"
        );
        return (six, eth, false);
    }
    io.enable_pull_up(&mut Master, Q1373_PHASE_DETECT);
    io.enable_pull_up(&mut Master, Q1373_ETH_DETECT);
    // let the pins settle
    delay_ms(1);
    match io.read_pin(&mut Master, Q1373_PHASE_DETECT) {
        Ok(level) => six = !level,
        Err(_) => log!(
            Error,
            t,
            "failed to read phase detect pin; assuming 4 phases"
        ),
    }
    match io.read_pin(&mut Master, Q1373_ETH_DETECT) {
        Ok(level) => eth = level,
        Err(_) => log!(Error, t, "failed to read ETH detect pin; assuming ethernet"),
    }
    // the regulator by its device code, independent of the phase strap
    // (BOM variants fit the TPS53667 with 4 phases)
    let code = Tps536x7::device_code(&mut SmBus);
    let tps53667 = code == TPS53667_DEVICE_CODE;
    log!(
        Info,
        t,
        "detection: VR device code {code:04x} ({}), {} phases, ethernet {}",
        if tps53667 { "TPS53667" } else { "TPS53647" },
        if six { 6 } else { 4 },
        if eth { "populated" } else { "not populated" }
    );
    (six, eth, tps53667)
}

/// The number of chip temperature sensors (TMP1075 0, 2, 3; 1 is the VR's).
fn detect_num_temp_sensors(t: &CStr) -> c_int {
    let mut found = 0;
    for i in 0..4 {
        if i == VR_TMP1075 {
            continue;
        }
        if crate::tmp1075::read_temperature(i) == 0.0 {
            break;
        }
        log!(Info, t, "found asic temp sensor {i}");
        found += 1;
    }
    found
}

/// The board for the other modules (one board, created at boot).
pub mod api {
    use super::*;

    pub(crate) fn b() -> &'static Board {
        BOARD.get().expect("board not created")
    }

    /// Creates the board (`BOARD_*`); false for an unknown id.
    pub fn board_create(id: c_int) -> bool {
        let Some(id) = BoardId::from_int(id) else {
            return false;
        };
        let _ = BOARD.set(Board::new(id));
        // the board's ASIC chain on the UART
        if let Some(asic) = Asics::new(board_asic_chip(), crate::serial::uart()) {
            let _ = b().asic.set(Box::leak(Box::new(asic)));
        }
        true
    }

    /// The board's `asic_chip_t`.
    pub fn board_asic_chip() -> c_int {
        match b().id() {
            NerdAxe => 0,
            NerdQAxePlus | NerdOctaxePlus => 1,
            NerdAxeGaia | Q1373 => 3,
            _ => 2,
        }
    }

    pub fn board_init() -> bool {
        b().init()
    }

    pub fn board_init_asics() -> bool {
        b().init_asics()
    }

    pub fn board_load_settings() {
        b().load_settings()
    }

    pub fn board_shutdown() {
        b().shutdown()
    }

    pub fn board_set_voltage(volts: f32) -> bool {
        b().set_voltage(volts)
    }

    pub fn board_set_asic_frequency(mhz: f32) -> bool {
        b().set_asic_frequency(mhz)
    }

    pub fn board_set_vr_frequency(hz: u32) {
        b().set_vr_frequency(hz)
    }

    pub fn board_set_fan_speed_ch(ch: c_int, perc: f32) {
        b().set_fan_speed_ch(ch, perc)
    }

    pub fn board_fan_rpm(ch: c_int) -> u16 {
        b().fan_rpm(ch)
    }

    pub fn board_set_fan_polarity(invert: bool) {
        b().set_fan_polarity(invert)
    }

    pub fn board_temperature(index: c_int) -> f32 {
        b().temperature(index)
    }

    pub fn board_vr_temp() -> f32 {
        b().vr_temp()
    }

    pub fn board_vr_temp_internal() -> f32 {
        b().vr_temp_internal()
    }

    /// 0 vin, 1 iin, 2 pin, 3 vout, 4 iout, 5 pout
    pub fn board_power_value(what: c_int) -> f32 {
        b().power(what)
    }

    pub fn board_request_buck_telemetry() {
        b().request_buck_telemetry()
    }

    pub fn board_request_chip_temps() {
        b().request_chip_temps()
    }

    pub fn board_has_ethernet() -> bool {
        b().has_ethernet()
    }

    pub fn board_detect_ethernet() {
        b().detect_ethernet()
    }

    pub fn board_has_can_extension() -> bool {
        b().has_can_extension()
    }

    pub fn board_is_can_slave() -> bool {
        b().is_can_slave()
    }

    pub fn board_can_tx_pin() -> c_int {
        b().can_pins().0
    }

    pub fn board_can_rx_pin() -> c_int {
        b().can_pins().1
    }

    /// The constants and settings the getters read.
    pub struct Info {
        pub device_model: &'static CStr,
        pub mining_agent: &'static CStr,
        pub asic_model: &'static CStr,
        pub swarm_color: &'static CStr,
        pub default_theme: &'static CStr,
        pub fan_labels: [&'static CStr; 2],
        pub version: c_int,
        pub asic_count: c_int,
        pub chips_detected: c_int,
        pub num_temp_sensors: c_int,
        pub num_fans: c_int,
        pub job_interval_ms: c_int,
        pub asic_frequency: c_int,
        pub asic_voltage_mv: c_int,
        pub default_frequency: c_int,
        pub default_voltage_mv: c_int,
        pub abs_max_frequency: c_int,
        pub abs_min_voltage_mv: c_int,
        pub abs_max_voltage_mv: c_int,
        pub eco_frequency: c_int,
        pub eco_voltage_mv: c_int,
        pub fan_mode: [c_int; 2],
        pub pid: [Pid; 2],
        pub vr_frequency: u32,
        pub default_vr_frequency: u32,
        pub min_difficulty: u32,
        pub min_difficulty_dual_pool: u32,
        pub max_difficulty: u32,
        pub vr_max_temp: f32,
        pub fan_perc: f32,
        pub max_pin: f32,
        pub min_pin: f32,
        pub max_vin: f32,
        pub min_vin: f32,
        pub min_current_a: f32,
        pub max_current_a: f32,
        pub fan_invert: bool,
        pub flip_screen: bool,
        pub has_hash_counter: bool,
        pub initialized: bool,
        pub buck_initialized: bool,
        pub shutdown: bool,
        pub frequencies: &'static [u32],
        pub voltages: &'static [u32],
    }

    /// A snapshot of the board's values.
    pub(crate) fn info() -> Info {
        let s = b().state();
        let p = &s.p;
        Info {
            device_model: p.device_model,
            mining_agent: p.mining_agent,
            asic_model: p.asic_model,
            swarm_color: p.swarm_color,
            default_theme: p.default_theme,
            fan_labels: p.fan_labels,
            version: p.version,
            asic_count: p.asic_count,
            chips_detected: s.chips_detected,
            num_temp_sensors: s.num_temp_sensors,
            num_fans: p.num_fans,
            job_interval_ms: s.job_interval_ms,
            asic_frequency: s.asic_frequency,
            asic_voltage_mv: s.asic_voltage_mv,
            default_frequency: p.default_frequency,
            default_voltage_mv: p.default_voltage_mv,
            abs_max_frequency: p.abs_max_frequency,
            abs_min_voltage_mv: p.abs_min_voltage_mv,
            abs_max_voltage_mv: p.abs_max_voltage_mv,
            eco_frequency: p.eco_frequency,
            eco_voltage_mv: p.eco_voltage_mv,
            fan_mode: s.fan_mode,
            pid: s.pid,
            vr_frequency: s.vr_frequency,
            default_vr_frequency: p.vr_frequency,
            min_difficulty: p.min_difficulty,
            min_difficulty_dual_pool: p.min_difficulty_dual_pool,
            max_difficulty: p.max_difficulty,
            vr_max_temp: p.vr_max_temp,
            fan_perc: s.fan_perc,
            max_pin: p.max_pin,
            min_pin: p.min_pin,
            max_vin: p.max_vin,
            min_vin: p.min_vin,
            min_current_a: p.min_current_a,
            max_current_a: p.max_current_a,
            fan_invert: s.fan_invert,
            flip_screen: s.flip_screen,
            has_hash_counter: p.has_hash_counter,
            initialized: s.initialized,
            buck_initialized: s.buck_initialized,
            shutdown: s.shutdown,
            frequencies: p.frequencies,
            voltages: p.voltages,
        }
    }

    pub fn board_is_initialized() -> bool {
        BOARD.get().is_some_and(|b| b.state().initialized)
    }

    pub fn board_is_axe() -> bool {
        is_axe(b().id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(id: BoardId) -> Board {
        Board::new(id)
    }

    #[test]
    fn defaults_from_the_profile() {
        let b = board(Q1373);
        let s = b.state();
        assert_eq!(s.fan_mode[1], 3);
        assert_eq!(s.asic_frequency, s.p.default_frequency);
        assert_eq!(s.asic_voltage_mv, s.p.default_voltage_mv);
        assert_eq!(s.vr_frequency, s.p.vr_frequency);
    }

    #[test]
    fn rev7_values() {
        let b = board(NerdQAxePlus2);
        let mut s = b.state();
        s.apply_rev7();
        assert_eq!(s.p.version, 502);
        assert_eq!(s.p.frequencies.len(), 23);
        assert_eq!(s.p.voltages.len(), 19);
        assert_eq!(s.asic_voltage_mv, 1200);
        assert_eq!(s.p.abs_max_frequency, 1000);
    }

    #[test]
    fn pins_and_options() {
        assert_eq!(board(Q1373).can_pins(), (1, 10));
        assert_eq!(board(NerdQAxePlus).can_pins(), (21, 16));
        assert_eq!(board(NerdAxeGamma).can_pins(), (-1, -1));
        assert_eq!(board(NerdQAxePlus).eth_pins(), Some(QAXE_HAT));
        assert_eq!(board(Q1370).eth_pins(), None);
        assert_eq!(board(NerdAxeGaia).eth_pins(), Some(GAIA_ETH));
        assert!(board(Q1370).has_ethernet());
        assert!(board(Q1373).has_can_extension());
        assert!(!board(NerdAxe).has_can_extension());
    }

    #[test]
    fn variants() {
        for (id, v) in [
            (NerdAxe, Variant::Axe),
            (NerdAxeGamma, Variant::AxeGamma),
            (NerdAxeGaia, Variant::AxeGaia),
            (Q1373, Variant::Classic),
        ] {
            let b = board(id);
            let hw = b.hw();
            assert_eq!(b.state().variant(&hw), v);
        }
    }

    #[test]
    fn plus2_family_estimates_chip_temps() {
        assert!(is_plus2_family(NerdQX) && is_plus2_family(NerdOctaxeGamma));
        assert!(!is_plus2_family(NerdOctaxePlus) && !is_plus2_family(Q1370));
    }
}
