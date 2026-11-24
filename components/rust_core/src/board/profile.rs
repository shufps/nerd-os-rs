//! The board profiles (port of the constructors of the `Board` classes in
//! `main/boards/`): device and ASIC model, frequency/voltage options,
//! defaults and limits, buck converter phases and current limits, fan and PID
//! defaults, power gauge ranges, pool difficulties, colours and themes, and
//! the version rolling frequency, per board as the constructor chain left
//! them, with the hardware detections as inputs (Q1373 straps and VR device
//! code, NerdOCTAXE-γ VR detect pin). Verified
//! against the former constructors by `tests/board_profile.rs`.
//!
//! Note: the Q1370/Q1373 keep the BM1368's default version rolling frequency
//! (register 0x15a4) that the NerdQAxe+ constructor set before they replaced
//! the ASIC driver; the profile keeps it explicitly.

use core::ffi::{c_int, CStr};

use crate::mining::asic::Chip;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pid {
    pub target_temp: u16,
    pub p: u16,
    pub i: u16,
    pub d: u16,
}

const PID_DEFAULT: Pid = Pid {
    target_temp: 65,
    p: 600,
    i: 10,
    d: 1000,
};

const fn pid(target_temp: u16) -> Pid {
    Pid {
        target_temp,
        ..PID_DEFAULT
    }
}

/// The constants of a board.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    pub device_model: &'static CStr,
    pub mining_agent: &'static CStr,
    pub version: c_int,
    pub asic_model: &'static CStr,
    pub asic_count: c_int,
    pub job_interval_ms: c_int,
    pub frequencies: &'static [u32],
    pub voltages: &'static [u32],
    pub default_frequency: c_int,
    pub default_voltage_mv: c_int,
    pub abs_max_frequency: c_int,
    pub abs_min_voltage_mv: c_int,
    pub abs_max_voltage_mv: c_int,
    pub eco_frequency: c_int,
    pub eco_voltage_mv: c_int,
    pub init_voltage_mv: c_int,
    pub num_phases: c_int,
    pub imax: c_int,
    pub ifault: f32,
    pub fan_invert_polarity: bool,
    pub fan_perc: f32,
    pub flip_screen: bool,
    pub num_temp_sensors: c_int,
    pub vr_max_temp: f32,
    pub pid: [Pid; 2],
    /// -1: the Kconfig default
    pub fan_mode0: c_int,
    pub num_fans: c_int,
    pub fan_labels: [&'static CStr; 2],
    pub max_pin: f32,
    pub min_pin: f32,
    pub max_vin: f32,
    pub min_vin: f32,
    pub min_current_a: f32,
    pub max_current_a: f32,
    pub max_difficulty: u32,
    pub min_difficulty: u32,
    pub min_difficulty_dual_pool: u32,
    pub swarm_color: &'static CStr,
    pub default_theme: &'static CStr,
    pub vr_frequency: u32,
    pub has_hash_counter: bool,
    /// a TPS53667 instead of the TPS53647 (Q1373, NerdOCTAXE-γ, NerdEKO)
    pub tps53667: bool,
}

/// The boards (`BOARD_*` in `board_profile.h`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardId {
    NerdAxe = 0,
    NerdAxeGamma,
    NerdAxeGaia,
    NerdQAxePlus,
    NerdOctaxePlus,
    NerdQAxePlus2,
    NerdQX,
    NerdHaxeGamma,
    NerdEko,
    NerdOctaxeGamma,
    Q1370,
    Q1373,
}

impl BoardId {
    pub fn from_int(v: c_int) -> Option<BoardId> {
        use BoardId::*;
        Some(match v {
            0 => NerdAxe,
            1 => NerdAxeGamma,
            2 => NerdAxeGaia,
            3 => NerdQAxePlus,
            4 => NerdOctaxePlus,
            5 => NerdQAxePlus2,
            6 => NerdQX,
            7 => NerdHaxeGamma,
            8 => NerdEko,
            9 => NerdOctaxeGamma,
            10 => Q1370,
            11 => Q1373,
            _ => return None,
        })
    }
}

/// The detected hardware options.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Detect {
    /// the VR is a TPS53667 (Q1373: device code, NerdOCTAXE-γ: GPIO3)
    pub tps53667: bool,
    /// Q1373: the 6-phase strap
    pub six_phase: bool,
}

fn s(c: &'static CStr) -> &'static CStr {
    c
}

static F_NQP: [u32; 9] = [400, 425, 450, 475, 490, 500, 525, 550, 575];
static V_NQP: [u32; 6] = [1100, 1150, 1200, 1250, 1300, 1350];
static F_BM1370: [u32; 7] = [500, 515, 525, 550, 575, 590, 600];
static V_BM1370: [u32; 9] = [1120, 1130, 1140, 1150, 1160, 1170, 1180, 1190, 1200];
static F_NQX: [u32; 22] = [
    495, 500, 525, 550, 575, 600, 625, 650, 675, 700, 725, 750, 777, 800, 825, 850, 875, 900, 925,
    950, 975, 1000,
];
static V_NQX: [u32; 24] = [
    1085, 1120, 1130, 1140, 1150, 1160, 1170, 1180, 1190, 1200, 1220, 1230, 1240, 1250, 1260, 1270,
    1280, 1290, 1300, 1310, 1320, 1330, 1340, 1350,
];
static F_NOG_6PH: [u32; 12] = [525, 550, 575, 600, 625, 650, 675, 700, 725, 750, 775, 800];
static V_NOG_6PH: [u32; 15] = [
    1120, 1130, 1140, 1150, 1160, 1170, 1180, 1190, 1200, 1210, 1220, 1230, 1240, 1250, 1260,
];
static F_Q1373: [u32; 11] = [250, 275, 300, 325, 350, 375, 400, 425, 475, 500, 550];
static V_Q1373: [u32; 11] = [
    980, 990, 1000, 1010, 1020, 1030, 1040, 1050, 1060, 1070, 1080,
];
static F_AXE: [u32; 9] = [400, 425, 450, 475, 485, 500, 525, 550, 575];
static V_AXE: [u32; 5] = [1100, 1150, 1200, 1250, 1300];
static F_GAMMA: [u32; 5] = [500, 515, 525, 550, 575];
static F_GAIA: [u32; 9] = [300, 320, 340, 350, 360, 380, 400, 420, 440];
static V_GAIA: [u32; 8] = [900, 920, 940, 960, 980, 1000, 1020, 1040];

// the NerdQX limits, computed as the NerdQX constructor does (decode_m_*,
// spelled as there)
fn nqx_ifault(phases: c_int) -> c_int {
    let ix_tech = (500 / 10) - (6 * 7) + (81 / 9);
    let pleb_base = (72 / 6) - (8 / 4);
    let phil31 = ((56 / 2) / 4) + (6 / 3) - (8 / 4);
    let shufps = (36 / 6) - (8 / 4);
    phases * 60 - (ix_tech + pleb_base + phil31 + shufps)
}

fn nqx_abs_max_frequency(imax: c_int) -> c_int {
    let fourteen = (84 / 7) + (8 / 4);
    let ten = ((27 / 9) + (8 / 4)) * 2;
    imax * fourteen - ten
}

#[allow(clippy::eq_op)]
fn nqx_abs_max_voltage(imax: c_int) -> c_int {
    let bv = (84 / 7) + (36 / 18);
    let cv = 49 / 7;
    let av = (85 / 5) - (9 / 9) + (8 / 8);
    imax * bv + cv * bv + av
}

/// The NerdQAxe+ profile, the base of its family.
fn nerdqaxeplus() -> Profile {
    let (f, v): (&'static [u32], &'static [u32]) = (&F_NQP, &V_NQP);
    Profile {
        device_model: s(c"NerdQAxe+"),
        mining_agent: s(c"NerdQAxe+"),
        version: 501,
        asic_model: s(c"BM1368"),
        asic_count: 4,
        job_interval_ms: 1200,
        frequencies: f,
        voltages: v,
        default_frequency: 490,
        default_voltage_mv: 1250,
        abs_max_frequency: 800,
        abs_min_voltage_mv: 1050,
        abs_max_voltage_mv: 1400,
        eco_frequency: 0,
        eco_voltage_mv: 0,
        init_voltage_mv: 1250,
        num_phases: 2,
        imax: 60,
        ifault: 55.0,
        fan_invert_polarity: false,
        fan_perc: 100.0,
        flip_screen: false,
        num_temp_sensors: 0,
        vr_max_temp: 0.0,
        pid: [pid(55), PID_DEFAULT],
        fan_mode0: -1,
        num_fans: 2,
        fan_labels: [s(c"M2"), s(c"M1")],
        max_pin: 70.0,
        min_pin: 30.0,
        max_vin: 13.0,
        min_vin: 11.0,
        min_current_a: 0.0,
        max_current_a: 6.0,
        max_difficulty: 1024,
        min_difficulty: 256,
        min_difficulty_dual_pool: 128,
        swarm_color: s(c"#e700d8"),
        default_theme: s(c"cosmic"),
        vr_frequency: Chip::Bm1368.default_vr_frequency(),
        has_hash_counter: true,
        tps53667: false,
    }
}

/// The NerdQAxe++ profile (BM1370), the base of the later boards.
fn nerdqaxeplus2() -> Profile {
    let (f, v): (&'static [u32], &'static [u32]) = (&F_BM1370, &V_BM1370);
    Profile {
        device_model: s(c"NerdQAxe++"),
        mining_agent: s(c"NerdQAxe++"),
        asic_model: s(c"BM1370"),
        asic_count: 4,
        num_phases: 3,
        imax: 90,
        ifault: 95.0,
        job_interval_ms: 500,
        frequencies: f,
        voltages: v,
        default_frequency: 600,
        default_voltage_mv: 1150,
        abs_max_frequency: 800,
        abs_max_voltage_mv: 1400,
        init_voltage_mv: 1200,
        pid: [pid(55), PID_DEFAULT],
        max_pin: 100.0,
        min_pin: 52.0,
        max_vin: 13.0,
        min_vin: 11.0,
        min_current_a: 0.0,
        max_current_a: 8.0,
        max_difficulty: 2048,
        min_difficulty: 512,
        min_difficulty_dual_pool: 256,
        vr_frequency: Chip::Bm1370.default_vr_frequency(),
        ..nerdqaxeplus()
    }
}

fn nerdaxe() -> Profile {
    let (f, v): (&'static [u32], &'static [u32]) = (&F_AXE, &V_AXE);
    Profile {
        device_model: s(c"NerdAxe"),
        mining_agent: s(c"NerdAxe"),
        version: 204,
        asic_model: s(c"BM1366"),
        asic_count: 1,
        job_interval_ms: 1500,
        frequencies: f,
        voltages: v,
        default_frequency: 485,
        default_voltage_mv: 1200,
        abs_max_frequency: 0,
        abs_min_voltage_mv: 0,
        abs_max_voltage_mv: 0,
        eco_frequency: 0,
        eco_voltage_mv: 0,
        init_voltage_mv: 0,
        num_phases: 0,
        imax: 0,
        ifault: 0.0,
        fan_invert_polarity: false,
        fan_perc: 100.0,
        flip_screen: true,
        num_temp_sensors: 1,
        vr_max_temp: 0.0,
        pid: [
            Pid {
                target_temp: 55,
                p: 400,
                i: 10,
                d: 1000,
            },
            PID_DEFAULT,
        ],
        fan_mode0: -1,
        num_fans: 1,
        fan_labels: [s(c"Fan 1"), s(c"Fan 2")],
        max_pin: 15.0,
        min_pin: 5.0,
        max_vin: 5.5,
        min_vin: 4.5,
        min_current_a: 0.0,
        max_current_a: 5.0,
        max_difficulty: 256,
        min_difficulty: 64,
        min_difficulty_dual_pool: 32,
        swarm_color: s(c"#e7cf00"),
        default_theme: s(c"cosmic"),
        vr_frequency: Chip::Bm1366.default_vr_frequency(),
        has_hash_counter: true,
        tps53667: false,
    }
}

/// The profile of `board` with the detected options.
pub fn profile(board: BoardId, d: &Detect) -> Profile {
    use BoardId::*;
    match board {
        NerdAxe => nerdaxe(),
        NerdAxeGamma => {
            let (f, v): (&'static [u32], &'static [u32]) = (&F_GAMMA, &V_BM1370);
            Profile {
                device_model: s(c"NerdAxeGamma"),
                mining_agent: s(c"NerdAxe"),
                asic_model: s(c"BM1370"),
                version: 200,
                frequencies: f,
                voltages: v,
                default_frequency: 515,
                default_voltage_mv: 1150,
                init_voltage_mv: 1150,
                vr_max_temp: 105.0,
                pid: [pid(60), PID_DEFAULT],
                max_pin: 25.0,
                max_current_a: 6.0,
                max_difficulty: 2048,
                min_difficulty: 512,
                min_difficulty_dual_pool: 256,
                vr_frequency: Chip::Bm1370.default_vr_frequency(),
                ..nerdaxe()
            }
        }
        NerdAxeGaia => {
            let (f, v): (&'static [u32], &'static [u32]) = (&F_GAIA, &V_GAIA);
            Profile {
                device_model: s(c"NerdAxeGaia"),
                mining_agent: s(c"NerdAxe"),
                asic_model: s(c"BM1373"),
                version: 200,
                frequencies: f,
                voltages: v,
                default_frequency: 350,
                default_voltage_mv: 940,
                abs_min_voltage_mv: 800,
                abs_max_voltage_mv: 1300,
                init_voltage_mv: 1000,
                flip_screen: false,
                vr_max_temp: 105.0,
                pid: [pid(60), PID_DEFAULT],
                max_pin: 40.0,
                max_vin: 13.2,
                min_vin: 10.8,
                max_current_a: 6.0,
                max_difficulty: 2048,
                min_difficulty: 512,
                min_difficulty_dual_pool: 256,
                default_theme: s(c"gaia"),
                vr_frequency: Chip::Bm1373.default_vr_frequency(),
                ..nerdaxe()
            }
        }
        NerdQAxePlus => nerdqaxeplus(),
        NerdOctaxePlus => Profile {
            device_model: s(c"NerdOCTAXE+"),
            mining_agent: s(c"NerdOCTAXE+"),
            asic_count: 8,
            num_phases: 3,
            imax: 90,
            ifault: 85.0,
            max_difficulty: 2048,
            min_difficulty: 512,
            min_difficulty_dual_pool: 256,
            max_pin: 130.0,
            min_pin: 70.0,
            max_current_a: 15.0,
            fan_labels: [
                s(c"M2 (Use a Y-splitter cable for multiple ASIC fans.)"),
                s(c"M1"),
            ],
            swarm_color: s(c"#11d51e"),
            ..nerdqaxeplus()
        },
        NerdQAxePlus2 => nerdqaxeplus2(),
        NerdQX => {
            let (f, v): (&'static [u32], &'static [u32]) = (&F_NQX, &V_NQX);
            let imax = 90;
            Profile {
                device_model: s(c"NerdQX"),
                mining_agent: s(c"NerdQX"),
                num_phases: 3,
                imax,
                ifault: nqx_ifault(3) as f32,
                frequencies: f,
                voltages: v,
                abs_max_frequency: nqx_abs_max_frequency(imax),
                default_frequency: 777,
                default_voltage_mv: 1200,
                eco_frequency: 495,
                eco_voltage_mv: 1085,
                abs_max_voltage_mv: nqx_abs_max_voltage(imax),
                init_voltage_mv: 1200,
                pid: [pid(58), PID_DEFAULT],
                flip_screen: true,
                max_pin: 240.0,
                min_pin: 50.0,
                max_current_a: 20.0,
                max_difficulty: 4096,
                min_difficulty: 1024,
                min_difficulty_dual_pool: 256,
                swarm_color: s(c"#7300e7"),
                default_theme: s(c"default"),
                vr_frequency: 35000,
                ..nerdqaxeplus2()
            }
        }
        NerdHaxeGamma => Profile {
            device_model: s(c"NerdHaxe-\xce\xb3"),
            mining_agent: s(c"NerdHaxe-\xce\xb3"),
            asic_count: 6,
            num_phases: 4,
            imax: 120,
            ifault: 105.0,
            init_voltage_mv: 0,
            max_pin: 250.0,
            min_pin: 75.0,
            max_current_a: 15.0,
            max_difficulty: 4096,
            min_difficulty: 1024,
            min_difficulty_dual_pool: 256,
            swarm_color: s(c"#00e7e2"),
            ..nerdqaxeplus2()
        },
        NerdEko => Profile {
            device_model: s(c"NerdEKO"),
            mining_agent: s(c"NerdEKO"),
            asic_count: 12,
            num_phases: 6,
            imax: 240,
            ifault: 235.0,
            init_voltage_mv: 0,
            max_pin: 350.0,
            min_pin: 30.0,
            min_vin: 8.0,
            max_current_a: 25.0,
            max_difficulty: 8192,
            min_difficulty: 2048,
            min_difficulty_dual_pool: 256,
            swarm_color: s(c"#e76500"),
            tps53667: true,
            ..nerdqaxeplus2()
        },
        NerdOctaxeGamma => {
            let base = Profile {
                device_model: s(c"NerdOCTAXE-\xce\xb3"),
                mining_agent: s(c"NerdOCTAXE-\xce\xb3"),
                asic_count: 8,
                max_difficulty: 4096,
                min_difficulty: 1024,
                min_difficulty_dual_pool: 256,
                init_voltage_mv: 0,
                fan_labels: [
                    s(c"M2 (Use a Y-splitter cable for multiple ASIC fans.)"),
                    s(c"M1"),
                ],
                swarm_color: s(c"#11d51e"),
                ..nerdqaxeplus2()
            };
            if d.tps53667 {
                // 6 phases, 24.9 kOhm: 240 A
                let (f, v): (&'static [u32], &'static [u32]) = (&F_NOG_6PH, &V_NOG_6PH);
                Profile {
                    num_phases: 6,
                    imax: 240,
                    ifault: 235.0,
                    max_pin: 300.0,
                    min_pin: 30.0,
                    max_current_a: 25.0,
                    tps53667: true,
                    frequencies: f,
                    abs_max_frequency: 850,
                    voltages: v,
                    default_frequency: 700,
                    default_voltage_mv: 1210,
                    ..base
                }
            } else {
                // 4 phases, 33.2 kOhm: 180 A
                Profile {
                    num_phases: 4,
                    imax: 180,
                    ifault: 160.0,
                    max_pin: 250.0,
                    min_pin: 50.0,
                    max_current_a: 20.0,
                    ..base
                }
            }
        }
        Q1370 => q1370(),
        Q1373 => {
            let (f, v): (&'static [u32], &'static [u32]) = (&F_Q1373, &V_Q1373);
            let base = Profile {
                device_model: s(c"Q1373"),
                mining_agent: s(c"Q1373"),
                asic_model: s(c"BM1373"),
                asic_count: 4,
                num_phases: 4,
                // TPS53647 datasheet: Rmon 48.7 kOhm
                imax: (0.85f64 / 48700.0 / 5.0e-3 * 35000.0).round() as c_int,
                ifault: 160.0,
                max_pin: 180.0,
                min_pin: 50.0,
                max_current_a: 15.0,
                frequencies: f,
                voltages: v,
                default_frequency: 425,
                default_voltage_mv: 1010,
                abs_max_frequency: 730,
                abs_min_voltage_mv: 900,
                abs_max_voltage_mv: 1200,
                init_voltage_mv: 1050,
                pid: [pid(60), PID_DEFAULT],
                // PID fan control out of the box
                fan_mode0: 2,
                max_difficulty: 4096,
                min_difficulty: 1024,
                min_difficulty_dual_pool: 512,
                ..q1370()
            };
            if d.tps53667 {
                // TPS53667 datasheet: Rmon 33.2 kOhm
                let phases = if d.six_phase { 6 } else { 4 };
                let max_pin = phases as f32 * 45.0;
                Profile {
                    tps53667: true,
                    imax: (0.85f64 / 33200.0 / 5.0e-3 * 35000.0).round() as c_int,
                    num_phases: phases,
                    ifault: phases as f32 * 40.0,
                    max_pin,
                    max_current_a: max_pin / 12.0,
                    ..base
                }
            } else {
                // 6 phases need the TPS53667
                base
            }
        }
    }
}

fn q1370() -> Profile {
    Profile {
        device_model: s(c"Q1370"),
        mining_agent: s(c"Q1370"),
        asic_model: s(c"BM1370"),
        asic_count: 4,
        num_phases: 4,
        imax: 123,
        ifault: 160.0,
        job_interval_ms: 500,
        default_frequency: 600,
        default_voltage_mv: 1150,
        abs_max_frequency: 1200,
        abs_max_voltage_mv: 1400,
        init_voltage_mv: 1200,
        pid: [pid(55), PID_DEFAULT],
        max_pin: 150.0,
        min_pin: 52.0,
        max_current_a: 20.0,
        max_difficulty: 2048,
        min_difficulty: 512,
        min_difficulty_dual_pool: 256,
        // (the NerdQAxe+ constructor's BM1368 value, see the module doc)
        vr_frequency: Chip::Bm1368.default_vr_frequency(),
        ..{
            let (f, v): (&'static [u32], &'static [u32]) = (&F_BM1370, &V_BM1370);
            Profile {
                frequencies: f,
                voltages: v,
                ..nerdqaxeplus()
            }
        }
    }
}
