//! Hashrate from the chips' counters (port of `HashrateMonitor` in
//! `hashrate_monitor_task.cpp`): every 5 s the nonce counter (0x90) and the
//! hardware error counter (0x4c) of each chip are read; the counter deltas
//! over time give a rate per chip, the total goes through a 5-tap median
//! and a smoothing step.
//!
//! Behaviour notes:
//! - the chip table is created under the lock (the result task may already
//!   deliver replies);
//! - the log line is built without a fixed buffer, so it holds every chip
//!   on boards with many chips;
//! - a reply at the same microsecond as the previous one is ignored.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::cutil::{fixed, now_us};
use crate::log::log;

const TAG: &CStr = c"hashrate_monitor";
const PERIOD: Duration = Duration::from_millis(5000);
pub const REG_NONCE_TOTAL_CNT: u8 = 0x90;
pub const REG_HW_ERROR_CNT: u8 = 0x4c;

/// A cumulative chip counter and its rate.
#[derive(Clone, Copy, Default)]
struct Counter {
    /// (time in µs, counter) of the last reply
    prev: Option<(i64, u32)>,
    /// GH/s
    rate: f32,
}

impl Counter {
    /// Each count stands for 2^32 hashes.
    fn update(&mut self, counter: u32, now_us: i64) {
        if let Some((t, c)) = self.prev {
            let dt = now_us - t;
            if dt <= 0 {
                return;
            }
            let delta = counter.wrapping_sub(c);
            self.rate = (delta as f64 * 4294967296.0 / dt as f64 / 1000.0) as f32;
        }
        self.prev = Some((now_us, counter));
    }
}

#[derive(Clone, Copy, Default)]
struct Chip {
    hashes: Counter,
    errors: Counter,
}

/// 5-tap median (`Median<5>`), starting at 0.
#[derive(Default)]
struct Median {
    buf: [f32; 5],
    idx: usize,
}

impl Median {
    fn update(&mut self, value: f32) -> f32 {
        self.buf[self.idx] = value;
        self.idx = (self.idx + 1) % 5;
        let mut tmp = self.buf;
        tmp.sort_by(f32::total_cmp);
        tmp[2]
    }
}

#[derive(Default)]
pub struct Monitor {
    chips: Vec<Chip>,
    median: Median,
    hashrate: f32,
    smoothed: f32,
    /// CAN slaves' hashrate (set by the CAN master)
    external: f32,
}

impl Monitor {
    const fn new() -> Self {
        Self {
            chips: Vec::new(),
            median: Median {
                buf: [0.0; 5],
                idx: 0,
            },
            hashrate: 0.0,
            smoothed: 0.0,
            external: 0.0,
        }
    }

    fn chip(&mut self, asic: c_int) -> Option<&mut Chip> {
        let i = usize::try_from(asic).ok()?;
        self.chips.get_mut(i)
    }

    /// Register 0x90 of chip `asic`.
    pub fn on_counter(&mut self, asic: c_int, counter: u32, now_us: i64) {
        match self.chip(asic) {
            Some(c) => c.hashes.update(counter, now_us),
            None => log!(Error, TAG, "respnse for invalid asic {asic}"),
        }
    }

    /// Register 0x4c of chip `asic`. The error counter counts in the same
    /// unit as the nonce counter, so its rate is the "error hashrate".
    pub fn on_errors(&mut self, asic: c_int, counter: u32, now_us: i64) {
        match self.chip(asic) {
            Some(c) => c.errors.update(counter, now_us),
            None => log!(Error, TAG, "error response for invalid asic {asic}"),
        }
    }

    /// After both counters were read: total (median filtered) and smoothed
    /// hashrate; the log line.
    pub fn cycle(&mut self) -> String {
        let rates: Vec<String> = self
            .chips
            .iter()
            .map(|c| format!("{}GH/s", fixed(c.hashes.rate as f64, 2)))
            .collect();
        let total: f32 = self.chips.iter().map(|c| c.hashes.rate).sum();
        self.hashrate = self.median.update(total);
        if self.smoothed == 0.0 {
            self.smoothed = self.hashrate;
        }
        self.smoothed = 0.5 * self.smoothed + 0.5 * self.hashrate;
        format!(
            "chip hashrates: {} (total: {}GH/s)",
            rates.join(" / "),
            fixed(self.hashrate as f64, 3)
        )
    }
}

static MONITOR: Mutex<Monitor> = Mutex::new(Monitor::new());

fn with<T>(f: impl FnOnce(&mut Monitor) -> T) -> T {
    f(&mut MONITOR.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Sets up the chip table (`HashrateMonitor::start()`).
pub fn init(asic_count: usize) {
    with(|m| m.chips = vec![Chip::default(); asic_count]);
}

/// The CAN slaves' total hashrate (GH/s), set by the CAN master.
pub fn set_external(ghs: f32) {
    with(|m| m.external = ghs);
}

pub fn external() -> f32 {
    with(|m| m.external)
}

/// Tests that use the global monitor hold this lock.
#[cfg(test)]
pub(crate) fn tests_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn on_counter(asic: c_int, counter: u32) {
    with(|m| m.on_counter(asic, counter, now_us()));
}

pub fn on_errors(asic: c_int, counter: u32) {
    with(|m| m.on_errors(asic, counter, now_us()));
}

/// What the measurement loop needs from the firmware.
pub trait Env {
    fn suspend_if_shutdown(&mut self);
    /// Broadcast reset / read of a counter register (the replies come back
    /// through the result task).
    fn reset_counter(&mut self, reg: u8);
    fn read_counter(&mut self, reg: u8);
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// The measurement loop (`HashrateMonitor::taskLoop()`).
pub fn run(env: &mut impl Env) -> ! {
    env.sleep(Duration::from_millis(4000));
    env.reset_counter(REG_NONCE_TOTAL_CNT);
    let mut next = Instant::now();
    loop {
        env.suspend_if_shutdown();
        // two read-all bursts must not overlap (the second one truncates the
        // replies of the first): each gets its own settle time
        env.read_counter(REG_NONCE_TOTAL_CNT);
        env.sleep(Duration::from_millis(500));
        env.read_counter(REG_HW_ERROR_CNT);
        env.sleep(Duration::from_millis(500));

        let line = with(Monitor::cycle);
        log!(Info, TAG, "{line}");

        // as vTaskDelayUntil(): fixed period, no catching up
        next += PERIOD;
        let now = Instant::now();
        if next > now {
            env.sleep(next - now);
        } else {
            next = now;
        }
    }
}

pub(crate) mod ffi {
    use super::*;

    pub extern "C" fn hashrate_monitor_init(asic_count: c_int) {
        init(usize::try_from(asic_count).unwrap_or(0));
    }

    pub extern "C" fn hashrate_monitor_chip(asic: c_int) -> f32 {
        with(|m| m.chip(asic).map_or(0.0, |c| c.hashes.rate))
    }

    pub extern "C" fn hashrate_monitor_chip_errors(asic: c_int) -> f32 {
        with(|m| m.chip(asic).map_or(0.0, |c| c.errors.rate))
    }

    pub extern "C" fn hashrate_monitor_total() -> f32 {
        with(|m| m.hashrate)
    }

    pub extern "C" fn hashrate_monitor_smoothed() -> f32 {
        with(|m| m.smoothed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor(chips: usize) -> Monitor {
        Monitor {
            chips: vec![Chip::default(); chips],
            ..Default::default()
        }
    }

    #[test]
    fn rate_from_counter_deltas() {
        let mut m = monitor(2);
        // first reply: baseline only
        m.on_counter(0, 1000, 1_000_000);
        assert_eq!(m.chips[0].hashes.rate, 0.0);
        // 1000 counts in 5 s: 1000 * 2^32 / 5e6 µs / 1000 = 858.99 GH/s
        m.on_counter(0, 2000, 6_000_000);
        assert!((m.chips[0].hashes.rate - 858.9935).abs() < 0.01);
        // the counter wraps: from 2^32 - 500 to 500 are 1000 counts
        m.on_counter(1, u32::MAX - 499, 1_000_000);
        m.on_counter(1, 500, 6_000_000);
        assert!((m.chips[1].hashes.rate - 858.9935).abs() < 0.01);
        m.on_counter(0, 3000, 11_000_000);
        assert!((m.chips[0].hashes.rate - 858.9935).abs() < 0.01);
        // same timestamp: ignored
        m.on_counter(0, 5000, 11_000_000);
        assert!((m.chips[0].hashes.rate - 858.9935).abs() < 0.01);
        // errors the same way, separately
        m.on_errors(1, 0, 0);
        m.on_errors(1, 10, 5_000_000);
        assert!((m.chips[1].errors.rate - 8.589935).abs() < 0.001);
        assert_eq!(m.chips[0].errors.rate, 0.0);
        // invalid chip: ignored
        m.on_counter(2, 1, 1);
        m.on_counter(-1, 1, 1);
    }

    #[test]
    fn median_and_smoothing() {
        let mut m = monitor(1);
        m.chips[0].hashes.rate = 100.0;
        // the median starts with zeros: two cycles until it follows
        m.cycle();
        assert_eq!(m.hashrate, 0.0);
        m.cycle();
        assert_eq!(m.hashrate, 0.0);
        let line = m.cycle();
        assert_eq!(m.hashrate, 100.0);
        assert_eq!(line, "chip hashrates: 100.00GH/s (total: 100.000GH/s)");
        // smoothing starts at the first non-zero value
        assert_eq!(m.smoothed, 100.0);
        // a single outlier is filtered
        m.chips[0].hashes.rate = 10_000.0;
        m.cycle();
        assert_eq!(m.hashrate, 100.0);
        m.chips[0].hashes.rate = 200.0;
        m.cycle();
        m.cycle();
        m.cycle();
        assert_eq!(m.hashrate, 200.0);
        assert!(m.smoothed > 100.0 && m.smoothed <= 200.0);
    }

    #[test]
    fn log_line_for_many_chips_and_large_values() {
        // 12 chips with large rates: longer than 256 bytes
        let mut m = monitor(12);
        for c in &mut m.chips {
            c.hashes.rate = 3.4e9;
        }
        let line = m.cycle();
        assert_eq!(line.matches("GH/s / ").count(), 11);
        assert!(line.len() > 256);
    }
}
