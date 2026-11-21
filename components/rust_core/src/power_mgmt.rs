//! The power management (port of `main/tasks/power_management_task.cpp` and
//! `main/fan_controller.cpp`): the hardware lock (a FreeRTOS recursive
//! mutex shared by everything that touches the ASICs and the I2C devices),
//! shutdown and restart, the task that runs the power loop (`power_task.rs`)
//! with the fan control (`fan_control.rs`) on the board (`board_api.rs`), and
//! the values the other tasks read. C API: `power_mgmt.h`.

use core::ffi::{c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use crate::config;
use crate::fan_control::{ChannelConfig, FanController, Fans, MAX_FANS};
use crate::log::log;
use crate::power_task::{Env, PowerTask, POLL_MS};
use crate::sdkconfig;

const TAG: &CStr = c"power_management";

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::board_api::*;
    pub use crate::idf::raw::{
        xQueueCreateMutex, xQueueGiveMutexRecursive, xQueueTakeMutexRecursive,
    };
}

/// On the host: no board, the lock is a no-op.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn xQueueCreateMutex(_: u8) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn xQueueTakeMutexRecursive(_: *mut c_void, _: u32) -> i32 {
        1
    }
    pub unsafe fn xQueueGiveMutexRecursive(_: *mut c_void) -> i32 {
        1
    }
    pub fn bapi_is_initialized() -> bool {
        false
    }
    pub fn bapi_is_buck_initialized() -> bool {
        false
    }
    pub fn bapi_asic_voltage_millis() -> u16 {
        0
    }
    pub fn bapi_set_voltage(_: f32) {}
    pub fn bapi_asic_frequency() -> u16 {
        0
    }
    pub fn bapi_set_asic_frequency(_: f32) -> bool {
        false
    }
    pub fn bapi_vr_frequency() -> u32 {
        0
    }
    pub fn bapi_set_vr_frequency(_: u32) {}
    pub fn bapi_request_chip_temps() {}
    pub fn bapi_max_chip_temp() -> f32 {
        0.0
    }
    pub fn bapi_asic_count() -> c_int {
        0
    }
    pub fn bapi_chip_temp(_: c_int) -> f32 {
        0.0
    }
    pub fn bapi_request_buck_telemetry() {}
    pub fn bapi_power(_: c_int) -> f32 {
        0.0
    }
    pub fn bapi_fault() -> (c_int, u32) {
        (0, 0)
    }
    pub fn bapi_num_temp_sensors() -> c_int {
        0
    }
    pub fn bapi_temperature(_: c_int) -> f32 {
        0.0
    }
    pub fn bapi_shutdown() {}
    pub fn bapi_invert_fan_polarity() -> bool {
        false
    }
    pub fn bapi_set_fan_polarity(_: bool) {}
    pub fn bapi_num_fans() -> c_int {
        0
    }
    pub fn bapi_set_fan_speed(_: c_int, _: f32) {}
    pub fn bapi_fan_rpm(_: c_int) -> u16 {
        0
    }
    pub fn bapi_pid_settings(_: c_int) -> [u16; 4] {
        [0; 4]
    }
    pub fn bapi_fan_mode(_: c_int) -> c_int {
        0
    }
    pub fn bapi_set_board_error(_: c_int, _: u32) {}
}

/// `queueQUEUE_TYPE_RECURSIVE_MUTEX`, `portMAX_DELAY`
const RECURSIVE_MUTEX: u8 = 4;
const MAX_DELAY: u32 = u32::MAX;

struct Lock(*mut c_void);
// the FreeRTOS mutex handle is used from every task
unsafe impl Send for Lock {}
unsafe impl Sync for Lock {}

static LOCK: OnceLock<Lock> = OnceLock::new();
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// the task runs (the former `m_board` was set): shutdown acts
static BOARD_SET: AtomicBool = AtomicBool::new(false);
static FANS: OnceLock<FanController> = OnceLock::new();
static TASK: OnceLock<PowerTask> = OnceLock::new();

fn mutex() -> *mut c_void {
    LOCK.get_or_init(|| Lock(unsafe { sys::xQueueCreateMutex(RECURSIVE_MUTEX) }.cast()))
        .0
}

/// Takes the hardware lock (recursive).
pub fn lock() {
    unsafe { sys::xQueueTakeMutexRecursive(mutex().cast(), MAX_DELAY) };
}

pub fn unlock() {
    unsafe { sys::xQueueGiveMutexRecursive(mutex().cast()) };
}

pub fn is_shutdown() -> bool {
    SHUTDOWN.load(Ordering::Acquire)
}

/// Switches the ASICs and regulators off (once the task runs).
pub fn shutdown() {
    lock();
    if BOARD_SET.load(Ordering::Acquire) {
        SHUTDOWN.store(true, Ordering::Release);
        sys::bapi_shutdown();
    }
    unlock();
}

/// Shuts down under the lock (no I2C transfer interrupted) and restarts.
pub fn restart() -> ! {
    log!(Warn, TAG, "Shutdown requested ...");
    lock();
    log!(Warn, TAG, "HW lock acquired!");
    shutdown();
    log!(Warn, TAG, "restart");
    crate::idf::task::restart()
}

/// The fan settings with the board's defaults (as the former
/// `FanController::readConfig()`).
fn fan_config() -> ([ChannelConfig; MAX_FANS], bool) {
    let pid_use_max = config::get_u16(b"fan_pid_max", 1) != 0;
    let mut cfg = [ChannelConfig {
        mode: 3,
        manual_speed: 100,
        overheat_temp: 70,
        target_temp: 60,
        p: 600,
        i: 10,
        d: 1000,
    }; MAX_FANS];
    let n = (sys::bapi_num_fans().max(0) as usize).min(MAX_FANS);
    for (ch, c) in cfg.iter_mut().enumerate().take(n) {
        let pid = sys::bapi_pid_settings(ch as c_int);
        let key = |k0: &'static [u8], k1: &'static [u8]| if ch == 0 { k0 } else { k1 };
        c.mode = sys::bapi_fan_mode(ch as c_int) as u8;
        c.manual_speed = if ch == 0 {
            config::get_u16(b"fanspeed", sdkconfig::FAN_SPEED as u16)
        } else {
            config::get_u16(b"fan1speed", 100)
        };
        c.overheat_temp = config::get_u16(
            key(b"overheat_temp", b"fan1_overheat"),
            sdkconfig::OVERHEAT_TEMP as u16,
        );
        c.target_temp = config::get_u16(key(b"pid_temp", b"fan1_pid_temp"), pid[0]);
        c.p = config::get_u16(key(b"pid_p", b"fan1_pid_p"), pid[1]);
        c.i = config::get_u16(key(b"pid_i", b"fan1_pid_i"), pid[2]);
        c.d = config::get_u16(key(b"pid_d", b"fan1_pid_d"), pid[3]);
    }
    (cfg, pid_use_max)
}

/// Reloads the fan settings (the PIDs keep their state).
pub fn fan_load_settings() {
    if let Some(f) = FANS.get() {
        let (cfg, max) = fan_config();
        f.load(cfg, max);
    }
}

struct BoardFans;

impl Fans for BoardFans {
    fn rpm(&mut self, ch: usize) -> u16 {
        sys::bapi_fan_rpm(ch as c_int)
    }
    fn set_speed(&mut self, ch: usize, fraction: f32) {
        sys::bapi_set_fan_speed(ch as c_int, fraction)
    }
}

/// The power loop's view of the board, the lock and the fans.
struct Board;

impl Env for Board {
    fn lock(&mut self) {
        lock()
    }
    fn unlock(&mut self) {
        unlock()
    }
    fn is_shutdown(&mut self) -> bool {
        is_shutdown()
    }
    fn is_initialized(&mut self) -> bool {
        sys::bapi_is_initialized()
    }
    fn is_buck_initialized(&mut self) -> bool {
        sys::bapi_is_buck_initialized()
    }
    fn asic_voltage_millis(&mut self) -> u16 {
        sys::bapi_asic_voltage_millis()
    }
    fn set_voltage(&mut self, volts: f32) {
        sys::bapi_set_voltage(volts)
    }
    fn asic_frequency(&mut self) -> u16 {
        sys::bapi_asic_frequency()
    }
    fn set_asic_frequency(&mut self, mhz: f32) -> bool {
        sys::bapi_set_asic_frequency(mhz)
    }
    fn vr_frequency(&mut self) -> u32 {
        sys::bapi_vr_frequency()
    }
    fn set_vr_frequency(&mut self, hz: u32) {
        sys::bapi_set_vr_frequency(hz)
    }
    fn request_chip_temps(&mut self) {
        sys::bapi_request_chip_temps()
    }
    fn max_chip_temp(&mut self) -> f32 {
        sys::bapi_max_chip_temp()
    }
    fn asic_count(&mut self) -> c_int {
        sys::bapi_asic_count()
    }
    fn chip_temp(&mut self, i: c_int) -> f32 {
        sys::bapi_chip_temp(i)
    }
    fn request_buck_telemetry(&mut self) {
        sys::bapi_request_buck_telemetry()
    }
    fn power(&mut self, what: c_int) -> f32 {
        sys::bapi_power(what)
    }
    fn fault(&mut self) -> (c_int, u32) {
        sys::bapi_fault()
    }
    fn set_board_error(&mut self, error: c_int, status: u32) {
        sys::bapi_set_board_error(error, status)
    }
    fn num_temp_sensors(&mut self) -> c_int {
        sys::bapi_num_temp_sensors()
    }
    fn temperature(&mut self, i: c_int) -> f32 {
        sys::bapi_temperature(i)
    }
    fn fan_update(&mut self, chip_temp_max: f32, vr_temp: f32) {
        if let Some(f) = FANS.get() {
            f.update(&mut BoardFans, chip_temp_max, vr_temp);
        }
    }
    fn fan_get(&mut self, ch: c_int, what: c_int) -> u16 {
        fan_get(ch, what)
    }
    fn influx_pwr(&mut self, v: [f32; 6]) {
        crate::influx::ffi::influx_task_set_pwr(v[0], v[1], v[2], v[3], v[4], v[5]);
    }
    fn influx_temperature(&mut self, chip: f32, vr: f32) {
        crate::influx::ffi::influx_task_set_temperature(chip, vr);
    }
    fn influx_fan(&mut self, v: [f32; 4]) {
        crate::influx::ffi::influx_set_fan(v[0], v[1], v[2], v[3]);
    }
    fn now_us(&mut self) -> u64 {
        crate::cutil::now_us() as u64
    }
}

/// `what`: 0 RPM, 1 %, 2 overheated, 3 overheat threshold.
pub fn fan_get(ch: c_int, what: c_int) -> u16 {
    let Some(f) = FANS.get() else {
        return 0;
    };
    match what {
        0 => f.rpm(ch),
        1 => f.speed_perc(ch),
        2 => f.overheated(ch) as u16,
        _ => f.overheat_temp(ch),
    }
}

/// The task: fan polarity and controller, then the power loop.
pub fn run() -> ! {
    BOARD_SET.store(true, Ordering::Release);
    sys::bapi_set_fan_polarity(sys::bapi_invert_fan_polarity());
    let n = sys::bapi_num_fans().max(0) as usize;
    let (cfg, max) = fan_config();
    let _ = FANS.set(FanController::new(n, POLL_MS as c_int, cfg, max));
    // NQ+: the chips' internal temperature sensors are too slow for the PID,
    // the max of the board's sensors and the chips is used
    let use_max_temp = sdkconfig::BOARD == "NERDQAXEPLUS";
    let task = TASK.get_or_init(|| PowerTask::new(use_max_temp));
    task.run(&mut Board)
}

/// 0 power, 1 voltage, 2 current, 3 chip temperature, 4 VR temperature, 5
/// VR temperature (internal); 0 before the task runs.
pub fn get(what: c_int) -> f32 {
    TASK.get().map_or(0.0, |t| t.get(what))
}

pub(crate) mod ffi {
    use super::*;

    #[no_mangle]
    pub unsafe extern "C" fn pm_task(_: *mut c_void) {
        run()
    }
    #[no_mangle]
    pub extern "C" fn pm_lock() {
        lock()
    }
    #[no_mangle]
    pub extern "C" fn pm_unlock() {
        unlock()
    }
    #[no_mangle]
    pub extern "C" fn pm_is_shutdown() -> bool {
        is_shutdown()
    }
    #[no_mangle]
    pub extern "C" fn pm_shutdown() {
        shutdown()
    }
    #[no_mangle]
    pub extern "C" fn pm_restart() {
        restart()
    }
    #[no_mangle]
    pub extern "C" fn pm_get(what: c_int) -> f32 {
        get(what)
    }
    #[no_mangle]
    pub extern "C" fn pm_fan_get(ch: c_int, what: c_int) -> u16 {
        fan_get(ch, what)
    }
    #[no_mangle]
    pub extern "C" fn pm_fan_load_settings() {
        fan_load_settings()
    }
    #[no_mangle]
    pub extern "C" fn pm_fan_pid_use_max() -> bool {
        FANS.get().is_some_and(|f| f.pid_use_max())
    }
}
