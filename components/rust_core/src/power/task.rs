//! The power management task (port of
//! `main/tasks/power_management_task.cpp`): every 2 s, under the hardware
//! lock, it applies changed core voltage / ASIC frequency / version rolling
//! frequency settings, requests and logs the chip temperatures, reads and
//! publishes the buck telemetry (switching the ASIC voltage off on a buck
//! fault), picks the temperature for the fan control, runs the fan
//! controller and switches the ASIC voltage off when a channel overheats.
//! The board, the fan controller and the system error are the caller's
//! hooks. The cycle is checked step by step against
//! the C++ sequence by the tests below.
//!
//! The overheat status word is built from the temperatures with saturating
//! conversions (negative and NaN give 0).

use core::ffi::{c_int, CStr};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::log::log;
use crate::util::cutil::fixed;

const TAG: &CStr = c"power_management";
pub const POLL_MS: u64 = 2000;
const TELEMETRY_PERIOD_US: u64 = 15_000_000;

pub const ERROR_NONE: c_int = 0;
pub const ERROR_TEMP_FAULT: c_int = 1;
pub const ERROR_VREG_TEMP_FAULT: c_int = 2;

/// The board, the fan controller and the outputs of one cycle.
pub trait Env {
    fn lock(&mut self);
    fn unlock(&mut self);
    fn is_shutdown(&mut self) -> bool;
    fn is_initialized(&mut self) -> bool;
    fn is_buck_initialized(&mut self) -> bool;
    fn asic_voltage_millis(&mut self) -> u16;
    fn set_voltage(&mut self, volts: f32);
    fn asic_frequency(&mut self) -> u16;
    fn set_asic_frequency(&mut self, mhz: f32) -> bool;
    fn vr_frequency(&mut self) -> u32;
    fn set_vr_frequency(&mut self, hz: u32);
    fn request_chip_temps(&mut self);
    fn max_chip_temp(&mut self) -> f32;
    fn asic_count(&mut self) -> c_int;
    fn chip_temp(&mut self, i: c_int) -> f32;
    fn request_buck_telemetry(&mut self);
    /// 0 vin, 1 iin, 2 pin, 3 pout, 4 vout, 5 iout, 6 VR temp, 7 VR temp
    /// (internal)
    fn power(&mut self, what: c_int) -> f32;
    /// (error, status)
    fn fault(&mut self) -> (c_int, u32);
    fn set_board_error(&mut self, error: c_int, status: u32);
    fn num_temp_sensors(&mut self) -> c_int;
    fn temperature(&mut self, i: c_int) -> f32;
    fn fan_update(&mut self, chip_temp_max: f32, vr_temp: f32);
    /// `what` as `fan_control_get()`: 0 RPM, 1 %, 2 overheated, 3 threshold
    fn fan_get(&mut self, ch: c_int, what: c_int) -> u16;
    fn influx_pwr(&mut self, v: [f32; 6]);
    fn influx_temperature(&mut self, chip: f32, vr: f32);
    fn influx_fan(&mut self, v: [f32; 4]);
    fn now_us(&mut self) -> u64;
}

/// The values the other tasks read (single-word atomics).
#[derive(Default)]
struct Published {
    power: AtomicU32,
    voltage: AtomicU32,
    current: AtomicU32,
    chip_temp_max: AtomicU32,
    vr_temp: AtomicU32,
    vr_temp_int: AtomicU32,
}

fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}

#[derive(Default)]
struct Last {
    core_voltage: u16,
    asic_frequency: u16,
    vr_frequency: u32,
    /// the 15 s buck telemetry request (due at the first telemetry read)
    telemetry_due: Option<u64>,
    /// the regulator fault last logged
    fault: c_int,
}

pub struct PowerTask {
    /// NerdQAxe+: the PID uses max(board sensors, chip temps)
    use_max_temp: bool,
    last: Mutex<Last>,
    out: Published,
}

/// `std::max(a, b)`: `b` only if `a < b` (a NaN `b` is ignored).
fn cmax(a: f32, b: f32) -> f32 {
    if a < b {
        b
    } else {
        a
    }
}

fn truthy(x: f32) -> bool {
    x != 0.0
}

impl PowerTask {
    pub fn new(use_max_temp: bool) -> Self {
        Self {
            use_max_temp,
            last: Mutex::new(Last::default()),
            out: Published::default(),
        }
    }

    /// 0 power (W), 1 input voltage (mV), 2 input current (mA), 3 chip
    /// temperature for the fan control, 4 VR temperature, 5 VR internal
    /// temperature.
    pub fn get(&self, what: c_int) -> f32 {
        let o = &self.out;
        load(match what {
            0 => &o.power,
            1 => &o.voltage,
            2 => &o.current,
            3 => &o.chip_temp_max,
            4 => &o.vr_temp,
            _ => &o.vr_temp_int,
        })
    }

    fn apply_asic_settings(&self, env: &mut impl Env, last: &mut Last) {
        if env.is_shutdown() || !env.is_initialized() {
            return;
        }
        let mv = env.asic_voltage_millis();
        if mv != last.core_voltage {
            log!(Info, TAG, "setting new vcore voltage to {mv}mV");
            env.set_voltage((mv as f64 / 1000.0) as f32);
            last.core_voltage = mv;
        }
        let mhz = env.asic_frequency();
        if mhz != last.asic_frequency {
            log!(Info, TAG, "setting new asic frequency to {mhz}MHz");
            if !env.set_asic_frequency(mhz as f32) {
                log!(Error, TAG, "pll setting not found for {mhz}MHz");
            }
            last.asic_frequency = mhz;
        }
        let hz = env.vr_frequency();
        if hz != last.vr_frequency {
            env.set_vr_frequency(hz);
            log!(Info, TAG, "setting version rolling frequency to {hz}Hz");
            last.vr_frequency = hz;
        }
    }

    fn log_chip_temps(&self, env: &mut impl Env) {
        if env.max_chip_temp() == 0.0 {
            return;
        }
        let temps: Vec<String> = (0..env.asic_count())
            .map(|i| format!("{}°C", fixed(env.chip_temp(i) as f64, 2)))
            .collect();
        log!(Info, TAG, "chip temperatures: {}", temps.join(" / "));
    }

    fn telemetry(&self, env: &mut impl Env, last: &mut Last) {
        if !env.is_buck_initialized() {
            return;
        }
        let now = env.now_us();
        let due = last.telemetry_due.get_or_insert(now);
        if now.wrapping_sub(*due) as i64 >= 0 {
            while now.wrapping_sub(*due) as i64 >= 0 {
                *due = due.wrapping_add(TELEMETRY_PERIOD_US);
            }
            env.request_buck_telemetry();
        }
        let [vin, iin, pin, pout, vout, iout] = [0, 1, 2, 3, 4, 5].map(|w| env.power(w));
        let vr_temp = env.power(6);
        let vr_temp_int = env.power(7);
        store(&self.out.vr_temp, vr_temp);
        store(&self.out.vr_temp_int, vr_temp_int);
        let f = |x: f32| fixed(x as f64, 2);
        log!(
            Info,
            TAG,
            "vin: {}, iin: {}, pin: {}, vout: {}, iout: {}, pout: {}, vr-temp: {}, vr-temp-int: {}",
            f(vin),
            f(iin),
            f(pin),
            f(vout),
            f(iout),
            f(pout),
            f(vr_temp),
            f(vr_temp_int)
        );
        env.influx_pwr([vin, iin, pin, vout, iout, pout]);
        let (error, status) = env.fault();
        if error != ERROR_NONE {
            if error != last.fault {
                log!(
                    Error,
                    TAG,
                    "voltage regulator fault {error} (status {status:08X}) - Shutting down asic voltage"
                );
            }
            env.set_board_error(error, status);
            env.set_voltage(0.0);
        }
        last.fault = error;
        store(&self.out.voltage, (vin as f64 * 1000.0) as f32);
        store(&self.out.current, (iin as f64 * 1000.0) as f32);
        store(&self.out.power, pin);
    }

    /// One cycle under the hardware lock.
    pub fn cycle(&self, env: &mut impl Env) {
        env.lock();
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());

        self.apply_asic_settings(env, &mut last);
        if env.is_initialized() {
            env.request_chip_temps();
        }
        self.log_chip_temps(env);
        self.telemetry(env, &mut last);

        // the hottest board sensor
        let mut sensors_max = 0.0f32;
        for i in 0..env.num_temp_sensors() {
            let t = env.temperature(i);
            if truthy(t) {
                log!(Info, TAG, "Temperature {i}: {} C", fixed(t as f64, 2));
            }
            sensors_max = cmax(sensors_max, t);
        }
        // the hottest chip (0 if not available)
        let chips_max = env.max_chip_temp();
        let chip_temp_max = if self.use_max_temp {
            // NQ+: the chips' internal sensors are too slow for the PID
            cmax(sensors_max, chips_max)
        } else if truthy(chips_max) {
            chips_max
        } else {
            sensors_max
        };
        store(&self.out.chip_temp_max, chip_temp_max);
        let vr_temp = load(&self.out.vr_temp);
        env.influx_temperature(chip_temp_max, vr_temp);

        env.fan_update(chip_temp_max, vr_temp);

        let over = [env.fan_get(0, 2) != 0, env.fan_get(1, 2) != 0];
        if over[0] || over[1] {
            let (oh0, oh1) = (env.fan_get(0, 3), env.fan_get(1, 3));
            let status = ((chip_temp_max as u32) << 24)
                | ((oh0 as u32) << 16)
                | ((vr_temp as u32) << 8)
                | oh1 as u32;
            // the ASIC takes priority over the VR
            let error = if over[0] {
                ERROR_TEMP_FAULT
            } else {
                ERROR_VREG_TEMP_FAULT
            };
            env.set_board_error(error, status);
            env.set_voltage(0.0);
            log!(
                Error,
                TAG,
                "System overheated (chip={}°C/thresh={oh0}°C vr={}°C/thresh={oh1}°C) - Shutting down asic voltage",
                fixed(chip_temp_max as f64, 1),
                fixed(vr_temp as f64, 2)
            );
        }
        let fan = [
            env.fan_get(0, 1) as f32,
            env.fan_get(0, 0) as f32,
            env.fan_get(1, 1) as f32,
            env.fan_get(1, 0) as f32,
        ];
        env.influx_fan(fan);
        drop(last);
        env.unlock();
    }

    /// The task: a cycle every 2 s (fixed rate; a late cycle skips the
    /// missed ticks).
    pub fn run(&self, env: &mut impl Env) -> ! {
        std::thread::sleep(Duration::from_millis(1000));
        let period = Duration::from_millis(POLL_MS);
        let mut next = Instant::now() + period;
        loop {
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            }
            self.cycle(env);
            next += period;
            let now = Instant::now();
            while next <= now {
                next += period;
            }
        }
    }
}

#[cfg(test)]
mod tests;
