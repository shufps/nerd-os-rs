//! TMP451 remote/local temperature sensor (port of
//! `main/boards/drivers/tmp451.cpp`): shutdown mode with one-shot
//! conversions, a discarded first conversion after a multiplexer switch,
//! the 0.0625 °C format and the per-channel calibration. The multiplexer
//! (GPIO pins or the FXL6408) is selected through the caller's callback.
//! Verified against the C++ driver by
//! `tests/sensors.rs`.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::i2c::RegBus;
use crate::log::log;

const TAG: &CStr = c"Tmp451";
const REG_LOCAL_MSB: u8 = 0x00;
const REG_REMOTE_MSB: u8 = 0x01;
const REG_STATUS: u8 = 0x02;
const REG_CONFIG1_W: u8 = 0x09;
const REG_ONE_SHOT_W: u8 = 0x0f;
const REG_REMOTE_LSB: u8 = 0x10;
const REG_RTOFFS_MSB: u8 = 0x11;
const REG_RTOFFS_LSB: u8 = 0x12;
const REG_LOCAL_LSB: u8 = 0x15;
const REG_NFACTOR_W: u8 = 0x23;

pub const ESP_OK: c_int = 0;
pub const ESP_FAIL: c_int = -1;
pub const ESP_ERR_NOT_FOUND: c_int = 0x105;
pub const ESP_ERR_TIMEOUT: c_int = 0x107;

/// Waiting and the clock (injected by the tests).
pub trait Clock {
    fn sleep(&mut self, d: Duration);
    fn now(&mut self) -> Duration;
}

pub struct RealClock(pub Instant);

impl Clock for RealClock {
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn now(&mut self) -> Duration {
        self.0.elapsed()
    }
}

struct State {
    cal_scale: f32,
    cal_off: [f32; 4],
    last_raw: [f32; 4],
    wait_after_switch: Duration,
    wait_before_read: Duration,
}

pub struct Tmp451 {
    addr: u8,
    s: Mutex<State>,
}

fn temp(msb: u8, lsb: u8) -> f32 {
    msb as i8 as f32 + ((lsb >> 4) & 0x0f) as f32 * 0.0625
}

impl Tmp451 {
    pub fn new(addr: u8) -> Self {
        Self {
            addr,
            s: Mutex::new(State {
                cal_scale: 1.09,
                cal_off: [-29.5; 4],
                last_raw: [f32::NAN; 4],
                wait_after_switch: Duration::from_millis(50),
                wait_before_read: Duration::from_millis(75),
            }),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.s.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn set_cal(&self, scale: f32, off: [f32; 4]) {
        self.with(|s| {
            s.cal_scale = scale;
            s.cal_off = off;
        });
    }

    pub fn set_settling(&self, after_switch_ms: u32, before_read_ms: u32) {
        self.with(|s| {
            s.wait_after_switch = Duration::from_millis(after_switch_ms as u64);
            s.wait_before_read = Duration::from_millis(before_read_ms as u64);
        });
    }

    /// Shutdown mode (one-shot conversions), default ideality, no offset.
    pub fn init(&self, bus: &mut impl RegBus) -> c_int {
        if bus.read(self.addr, REG_STATUS).is_none() {
            log!(Error, TAG, "TMP451 not responding at 0x{:02X}", self.addr);
            return ESP_ERR_NOT_FOUND;
        }
        for (reg, value, what) in [
            (REG_CONFIG1_W, 1u8 << 6, "config1 write"),
            (REG_NFACTOR_W, 0x00, "ideality"),
            (REG_RTOFFS_MSB, 0x00, "offset msb"),
            (REG_RTOFFS_LSB, 0x00, "offset lsb"),
        ] {
            if !bus.write(self.addr, reg, value) {
                log!(Error, TAG, "{what}");
                return ESP_FAIL;
            }
        }
        ESP_OK
    }

    /// Starts a conversion and polls the busy bit (5 ms steps, 200 ms).
    fn one_shot(&self, bus: &mut impl RegBus, clock: &mut impl Clock) -> c_int {
        if !bus.write(self.addr, REG_ONE_SHOT_W, 0xff) {
            log!(Error, TAG, "one_shot");
            return ESP_FAIL;
        }
        let t0 = clock.now();
        loop {
            let Some(s) = bus.read(self.addr, REG_STATUS) else {
                return ESP_FAIL;
            };
            if s & 0x80 == 0 {
                return ESP_OK;
            }
            if clock.now() - t0 > Duration::from_millis(200) {
                log!(Error, TAG, "one-shot timeout, status={s:02x}");
                return ESP_ERR_TIMEOUT;
            }
            clock.sleep(Duration::from_millis(5));
        }
    }

    /// One conversion of the remote (or local) sensor; NaN on an error.
    fn read(&self, bus: &mut impl RegBus, clock: &mut impl Clock, remote: bool) -> f32 {
        if self.one_shot(bus, clock) != ESP_OK {
            return f32::NAN;
        }
        let (msb_reg, lsb_reg) = if remote {
            (REG_REMOTE_MSB, REG_REMOTE_LSB)
        } else {
            (REG_LOCAL_MSB, REG_LOCAL_LSB)
        };
        let Some(msb) = bus.read(self.addr, msb_reg) else {
            return f32::NAN;
        };
        let Some(lsb) = bus.read(self.addr, lsb_reg) else {
            return f32::NAN;
        };
        temp(msb, lsb)
    }

    pub fn local(&self, bus: &mut impl RegBus, clock: &mut impl Clock) -> f32 {
        self.read(bus, clock, false)
    }

    /// The calibrated temperature of a multiplexer channel 0..3 (-1: the
    /// local sensor); `select` switches the multiplexer.
    pub fn temperature(
        &self,
        bus: &mut impl RegBus,
        clock: &mut impl Clock,
        channel: c_int,
        select: &mut impl FnMut(c_int) -> bool,
    ) -> f32 {
        if channel == -1 {
            return self.local(bus, clock);
        }
        if !(0..=3).contains(&channel) || !select(channel) {
            return f32::NAN;
        }
        let (after, before) = self.with(|s| (s.wait_after_switch, s.wait_before_read));
        clock.sleep(after);
        // the first conversion after a switch is thrown away
        let _ = self.read(bus, clock, true);
        clock.sleep(before);
        let raw = self.read(bus, clock, true);
        self.with(|s| {
            s.last_raw[channel as usize] = raw;
            ((raw - 30.0) * s.cal_scale + 30.0) + s.cal_off[channel as usize]
        })
    }

    pub fn raw(&self, channel: c_int) -> f32 {
        if !(0..=3).contains(&channel) {
            return f32::NAN;
        }
        self.with(|s| s.last_raw[channel as usize])
    }

    pub fn status(&self, bus: &mut impl RegBus) -> Option<u8> {
        bus.read(self.addr, REG_STATUS)
    }
}

mod ffi {}
