//! The board for the Rust modules that reached it through C (port of
//! `main/boards/board_api.cpp`, the bridge to the C++ `Board`, which by now
//! only forwarded to `board.rs`): the same `bapi_*` names, so the modules'
//! firmware `sys` blocks re-export these instead of declaring externs (the
//! host keeps its stubs with the same signatures).

use core::ffi::{c_int, CStr};

use crate::board::api::{self as b, info};
use crate::board_core::CHIP_TEMPS;

pub fn bapi_is_initialized() -> bool {
    b::board_is_initialized()
}
pub fn bapi_is_buck_initialized() -> bool {
    info().buck_initialized
}
pub fn bapi_asic_voltage_millis() -> u16 {
    info().asic_voltage_mv as u16
}
pub fn bapi_set_voltage(volts: f32) {
    b::board_set_voltage(volts);
}
pub fn bapi_asic_frequency() -> u16 {
    info().asic_frequency as u16
}
pub fn bapi_set_asic_frequency(mhz: f32) -> bool {
    b::board_set_asic_frequency(mhz)
}
pub fn bapi_vr_frequency() -> u32 {
    info().vr_frequency
}
pub fn bapi_set_vr_frequency(hz: u32) {
    b::board_set_vr_frequency(hz)
}
pub fn bapi_request_chip_temps() {
    b::board_request_chip_temps()
}
pub fn bapi_max_chip_temp() -> f32 {
    CHIP_TEMPS.max()
}
pub fn bapi_asic_count() -> c_int {
    info().asic_count
}
pub fn bapi_chip_temp(i: c_int) -> f32 {
    CHIP_TEMPS.get(i)
}
pub fn bapi_request_buck_telemetry() {
    b::board_request_buck_telemetry()
}

/// 0 Vin, 1 Iin, 2 Pin, 3 Pout, 4 Vout, 5 Iout, 6 VR temperature, 7 VR
/// temperature (internal); `board_power_value()` orders them differently.
pub fn bapi_power(what: c_int) -> f32 {
    match what {
        0 => b::board_power_value(0),
        1 => b::board_power_value(1),
        2 => b::board_power_value(2),
        3 => b::board_power_value(5),
        4 => b::board_power_value(3),
        5 => b::board_power_value(4),
        6 => b::board_vr_temp(),
        _ => b::board_vr_temp_internal(),
    }
}

/// The board error (0: none) and its PMBus status.
pub fn bapi_fault() -> (c_int, u32) {
    b::b().fault()
}
pub fn bapi_num_temp_sensors() -> c_int {
    info().num_temp_sensors
}
pub fn bapi_temperature(i: c_int) -> f32 {
    b::board_temperature(i)
}
pub fn bapi_shutdown() {
    b::board_shutdown()
}
pub fn bapi_invert_fan_polarity() -> bool {
    info().fan_invert
}
pub fn bapi_set_fan_polarity(invert: bool) {
    b::board_set_fan_polarity(invert)
}
pub fn bapi_num_fans() -> c_int {
    info().num_fans
}
pub fn bapi_set_fan_speed(ch: c_int, fraction: f32) {
    b::board_set_fan_speed_ch(ch, fraction)
}
pub fn bapi_fan_rpm(ch: c_int) -> u16 {
    b::board_fan_rpm(ch)
}

/// target temperature, P, I, D of channel `ch`
pub fn bapi_pid_settings(ch: c_int) -> [u16; 4] {
    let p = info().pid[(ch & 1) as usize];
    [p.target_temp, p.p, p.i, p.d]
}

pub fn bapi_fan_mode(ch: c_int) -> c_int {
    info().fan_mode[(ch & 1) as usize]
}
pub fn bapi_set_board_error(error: c_int, status: u32) {
    crate::system::set_error(error, status)
}
pub fn bapi_set_board_error_if_none(error: c_int, status: u32) {
    crate::system::set_error_if_none(error, status)
}
pub fn bapi_clear_board_error(error: c_int) {
    crate::system::clear_error(error)
}
pub fn bapi_is_can_slave() -> bool {
    b::board_is_can_slave()
}
pub fn bapi_has_hashrate_counter() -> bool {
    info().has_hash_counter
}
pub fn bapi_mining_agent() -> &'static CStr {
    info().mining_agent
}
pub fn bapi_asic_model() -> &'static CStr {
    info().asic_model
}
pub fn bapi_device_model() -> &'static CStr {
    info().device_model
}
pub fn bapi_asic_min_difficulty(dual_pool: bool) -> u32 {
    let i = info();
    if dual_pool {
        i.min_difficulty_dual_pool
    } else {
        i.min_difficulty
    }
}
pub fn bapi_asic_max_difficulty() -> u32 {
    info().max_difficulty
}
