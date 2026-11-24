//! Fan control (port of `main/fan_controller.cpp` and the PID library
//! `main/pid/PID_v1_bc.cpp`): up to two fan channels, each in manual, PID or
//! linked mode, the PID with back-calculation anti-windup in float32 as
//! before, and the overheat detection (fan to 100 %, the caller switches the
//! ASIC voltage off). The fans are driven through the caller's board hooks;
//! the settings come from the C++ side (`Config` and board defaults).
//! Verified against the C++ code by
//! `tests/fan_control.rs`.
//!
//! The PID is a port of the Arduino PID Library 1.2.x with back-calculation
//! anti-windup (Brett Beauregard, David Forrest; MIT license), as vendored
//! in `main/pid/PID_v1_bc.cpp` before.
//!
//! Behaviour notes:
//! - settings reloads (HTTP and CAN tasks) and the control cycle (power
//!   management task) are serialized by a lock, so `loadSettings()` never
//!   changes the channel configuration or the PID tunings during `update()`;
//! - a NaN temperature drives the channel to 100 % and leaves the PID state
//!   untouched, so the PID keeps working once the readings are valid again.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;

use crate::cutil::fixed;
use crate::log::log;

const TAG: &CStr = c"fan_ctrl";
pub const MAX_FANS: usize = 2;

pub const MODE_MANUAL: u8 = 0;
pub const MODE_PID: u8 = 2;
pub const MODE_LINKED: u8 = 3;

/// `PID_v1_bc` as used here: proportional on error, back-calculation
/// anti-windup, the sample time ignored by `Compute()`. The operations and
/// their order are those of the C++ code (float32).
#[derive(Clone, Copy, Debug, Default)]
pub struct Pid {
    kp: f32,
    ki: f32,
    kd: f32,
    reverse: bool,
    in_auto: bool,
    sample_time: f32,
    out_min: f32,
    out_max: f32,
    output_sum: f32,
    last_input: f32,
    pub target: f32,
    pub output: f32,
}

impl Pid {
    /// `new PID(...)`, `SetSampleTime()`, `SetOutputLimits(15, 100)`,
    /// `SetMode(AUTOMATIC)`, `SetControllerDirection(REVERSE)`,
    /// `Initialize()` as in `FanController::init()`; `input` and `output`
    /// are the (zero) values the PID reads at that point.
    pub fn new_reverse(p: f32, i: f32, d: f32, target: f32, sample_ms: c_int) -> Self {
        let mut pid = Self {
            out_min: 0.0,
            out_max: 255.0,
            sample_time: 100.0,
            target,
            ..Self::default()
        };
        pid.set_tunings(p, i, d);
        if sample_ms > 0 {
            let ratio = sample_ms as f32 / pid.sample_time;
            pid.ki *= ratio;
            pid.kd /= ratio;
            pid.sample_time = sample_ms as f32;
        }
        pid.out_min = 15.0;
        pid.out_max = 100.0;
        // SetMode(AUTOMATIC)
        pid.initialize(0.0);
        pid.in_auto = true;
        // SetControllerDirection(REVERSE)
        pid.kp = -pid.kp;
        pid.ki = -pid.ki;
        pid.kd = -pid.kd;
        pid.reverse = true;
        pid.initialize(0.0);
        pid
    }

    fn initialize(&mut self, input: f32) {
        self.output_sum = self.output;
        self.last_input = input;
        if self.output_sum > self.out_max {
            self.output_sum = self.out_max;
        } else if self.output_sum < self.out_min {
            self.output_sum = self.out_min;
        }
    }

    pub fn set_tunings(&mut self, p: f32, i: f32, d: f32) {
        if p < 0.0 || i < 0.0 || d < 0.0 {
            return;
        }
        let sample_time_in_sec = self.sample_time / 1000.0;
        self.kp = p;
        self.ki = i * sample_time_in_sec;
        self.kd = d / sample_time_in_sec;
        if self.reverse {
            self.kp = -self.kp;
            self.ki = -self.ki;
            self.kd = -self.kd;
        }
    }

    pub fn compute(&mut self, input: f32) {
        if !self.in_auto {
            return;
        }
        let error = self.target - input;
        let d_input = input - self.last_input;
        self.output_sum += self.ki * error;
        if self.output_sum > self.out_max {
            self.output_sum = self.out_max;
        } else if self.output_sum < self.out_min {
            self.output_sum = self.out_min;
        }
        let mut output = self.kp * error;
        output += self.output_sum - self.kd * d_input;
        if output > self.out_max {
            self.output_sum -= output - self.out_max;
            output = self.out_max;
        } else if output < self.out_min {
            self.output_sum += self.out_min - output;
            output = self.out_min;
        }
        self.output = output;
        self.last_input = input;
    }
}

/// `fan_channel_config_t`: one channel's settings.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelConfig {
    /// 0 manual, 2 PID, 3 linked to channel 0 (channel 1 only)
    pub mode: u8,
    /// %
    pub manual_speed: u16,
    /// °C; 0 disables the overheat check
    pub overheat_temp: u16,
    pub target_temp: u16,
    /// gains ×100
    pub p: u16,
    pub i: u16,
    pub d: u16,
}

impl Default for ChannelConfig {
    fn default() -> Self {
        Self {
            mode: MODE_LINKED,
            manual_speed: 100,
            overheat_temp: 70,
            target_temp: 60,
            p: 600,
            i: 10,
            d: 1000,
        }
    }
}

/// The board's fans.
pub trait Fans {
    fn rpm(&mut self, ch: usize) -> u16;
    /// `fraction` 0..1
    fn set_speed(&mut self, ch: usize, fraction: f32);
}

#[derive(Default)]
struct State {
    config: [ChannelConfig; MAX_FANS],
    pid: [Option<Pid>; MAX_FANS],
    pid_use_max: bool,
    rpm: [u16; MAX_FANS],
    perc: [u16; MAX_FANS],
    overheated: [bool; MAX_FANS],
}

pub struct FanController {
    channels: usize,
    s: Mutex<State>,
}

fn gains(c: &ChannelConfig) -> (f32, f32, f32) {
    (c.p as f32 / 100.0, c.i as f32 / 100.0, c.d as f32 / 100.0)
}

impl FanController {
    /// `channels` fans (at most 2), the PIDs created from `config`.
    pub fn new(
        channels: usize,
        sample_ms: c_int,
        config: [ChannelConfig; MAX_FANS],
        pid_use_max: bool,
    ) -> Self {
        let channels = channels.min(MAX_FANS);
        log!(Info, TAG, "initialising for {channels} fan channel(s)");
        let mut s = State {
            config,
            pid_use_max,
            ..State::default()
        };
        for (ch, &c) in config.iter().enumerate().take(channels) {
            let (p, i, d) = gains(&c);
            s.pid[ch] = Some(Pid::new_reverse(p, i, d, c.target_temp as f32, sample_ms));
            log!(
                Info,
                TAG,
                "ch{ch}: mode={} manual={}% overheat={}°C pid-target={}°C p={} i={} d={}",
                c.mode,
                c.manual_speed,
                c.overheat_temp,
                c.target_temp,
                fixed(p as f64, 2),
                fixed(i as f64, 2),
                fixed(d as f64, 2)
            );
        }
        Self {
            channels,
            s: Mutex::new(s),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.s.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// New settings at runtime: the PID keeps its state (bumpless).
    pub fn load(&self, config: [ChannelConfig; MAX_FANS], pid_use_max: bool) {
        self.with(|s| {
            s.pid_use_max = pid_use_max;
            for (ch, c) in config.iter().enumerate().take(self.channels) {
                s.config[ch] = *c;
                if let Some(pid) = s.pid[ch].as_mut() {
                    let (p, i, d) = gains(c);
                    pid.set_tunings(p, i, d);
                    pid.target = c.target_temp as f32;
                    log!(
                        Info,
                        TAG,
                        "ch{ch} PID: target={}°C p={} i={} d={}",
                        fixed(pid.target as f64, 1),
                        fixed(p as f64, 2),
                        fixed(i as f64, 2),
                        fixed(d as f64, 2)
                    );
                }
            }
        });
    }

    /// One control cycle: reads the RPMs, runs the PIDs, drives the fans.
    pub fn update(&self, fans: &mut impl Fans, chip_temp_max: f32, vr_temp: f32) {
        let channels = self.channels;
        self.with(|s| {
            let overheat_input = [chip_temp_max, vr_temp];
            let mut temp_input = [chip_temp_max, vr_temp];
            if (channels > 1 && s.config[1].mode == MODE_LINKED) || (channels == 1 && s.pid_use_max)
            {
                temp_input[0] = chip_temp_max.max(vr_temp);
            }
            for ch in 0..channels {
                s.rpm[ch] = fans.rpm(ch);
                let nan = temp_input[ch].is_nan() || overheat_input[ch].is_nan();
                if !nan {
                    if let Some(pid) = s.pid[ch].as_mut() {
                        pid.compute(temp_input[ch]);
                    }
                }
                let c = s.config[ch];
                if nan || (c.overheat_temp != 0 && overheat_input[ch] > c.overheat_temp as f32) {
                    s.overheated[ch] = !nan;
                    s.perc[ch] = 100;
                    fans.set_speed(ch, 1.0);
                    continue;
                }
                s.overheated[ch] = false;
                if c.mode == MODE_LINKED && ch > 0 {
                    s.perc[ch] = s.perc[0];
                    fans.set_speed(ch, s.perc[ch] as f32 / 100.0);
                    continue;
                }
                s.perc[ch] = match c.mode {
                    MODE_MANUAL => c.manual_speed,
                    MODE_PID => s.pid[ch].map_or(100, |p| p.output.round() as u16),
                    m => {
                        log!(Error, TAG, "ch{ch}: unknown mode {m}, defaulting to 100%");
                        100
                    }
                };
                fans.set_speed(ch, s.perc[ch] as f32 / 100.0);
            }
        });
    }

    fn channel<T: Default>(&self, ch: c_int, f: impl FnOnce(&State, usize) -> T) -> T {
        if ch < 0 || ch as usize >= self.channels {
            return T::default();
        }
        self.with(|s| f(s, ch as usize))
    }

    pub fn rpm(&self, ch: c_int) -> u16 {
        self.channel(ch, |s, c| s.rpm[c])
    }

    pub fn speed_perc(&self, ch: c_int) -> u16 {
        self.channel(ch, |s, c| s.perc[c])
    }

    pub fn overheated(&self, ch: c_int) -> bool {
        self.channel(ch, |s, c| s.overheated[c])
    }

    pub fn overheat_temp(&self, ch: c_int) -> u16 {
        self.channel(ch, |s, c| s.config[c].overheat_temp)
    }

    pub fn pid_use_max(&self) -> bool {
        self.with(|s| s.pid_use_max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct F {
        set: Vec<(usize, f32)>,
    }
    impl Fans for F {
        fn rpm(&mut self, ch: usize) -> u16 {
            1000 + ch as u16
        }
        fn set_speed(&mut self, ch: usize, fraction: f32) {
            self.set.push((ch, fraction));
        }
    }

    #[test]
    fn nan_and_overheat() {
        let cfg = [
            ChannelConfig {
                mode: MODE_PID,
                ..Default::default()
            },
            ChannelConfig::default(),
        ];
        let fc = FanController::new(2, 2000, cfg, true);
        let mut f = F { set: vec![] };
        fc.update(&mut f, 50.0, 40.0);
        fc.update(&mut f, 50.0, 40.0);
        assert!(fc.speed_perc(0) < 100);
        // NaN -> 100 %, the PID keeps working afterwards
        fc.update(&mut f, f32::NAN, 40.0);
        assert_eq!(fc.speed_perc(0), 100);
        assert!(!fc.overheated(0));
        fc.update(&mut f, 50.0, 40.0);
        assert!(fc.speed_perc(0) < 100);
        fc.update(&mut f, 71.0, 40.0);
        assert!(fc.overheated(0));
        assert_eq!(fc.rpm(1), 1001);
        assert_eq!(fc.rpm(2), 0);
    }
}
