//! Hashrate and temperature history (port of `History`, `HistoryAvg` and
//! `NonceDistribution` in `main/history.cpp`): a day of 5 s samples in a
//! ring buffer, the 1 min / 10 min / 1 h / 1 d sliding averages (with a
//! "ramping" display value while a window is not filled yet), the export
//! of a time range for the charts, and the share count per chip.
//!
//!
//! Behaviour notes:
//! - the current averages and timestamp are read under the lock (doubles and
//!   a `u64` written by the system task and read by the HTTP tasks, which
//!   are not atomic on the 32-bit CPU);
//! - float to integer conversions saturate (an out-of-range rate or chart
//!   value gives the nearest representable one).

use core::ffi::{c_int, CStr};
use std::sync::Mutex;

use crate::cutil::fixed;
use crate::log::log;

const TAG: &CStr = c"history";

/// Sample period in ms (`HR_INTERVAL`).
pub const PERIOD_MS: u64 = 5000;
/// A day of samples, rounded up to a power of two (`HISTORY_MAX_SAMPLES`).
pub const SAMPLES: usize = (86_400_000 / PERIOD_MS as usize).next_power_of_two();

fn wrap(i: i64) -> usize {
    (i as usize) & (SAMPLES - 1)
}

/// A sliding average over a time window (`HistoryAvg`).
struct Avg {
    first: i64,
    /// -1: empty
    last: i64,
    timespan: u64,
    samples: i64,
    /// sum of the rates in the window, Q10
    sum: i64,
    avg: f64,
    /// ramps up with the window's coverage while it is not filled
    display: f64,
    timestamp: u64,
    preliminary: bool,
}

impl Avg {
    const fn new(timespan_ms: u64) -> Self {
        Self {
            first: 0,
            last: -1,
            timespan: timespan_ms,
            samples: 0,
            sum: 0,
            avg: 0.0,
            display: 0.0,
            timestamp: 0,
            preliminary: true,
        }
    }

    /// Takes in the new samples and drops the ones older than the window.
    fn update(&mut self, n: i64, rates: &[u32], timestamps: &[u64]) {
        while self.last + 1 < n {
            self.last += 1;
            self.samples += 1;
            self.sum += rates[wrap(self.last)] as i64;
        }
        if self.last < 0 || self.samples <= 0 {
            self.timestamp = 0;
            self.avg = 0.0;
            self.display = 0.0;
            self.preliminary = true;
            return;
        }

        let last_ts = timestamps[wrap(self.last)];
        // keep (last - first) < timespan whenever possible
        while self.first < self.last
            && last_ts.wrapping_sub(timestamps[wrap(self.first)]) >= self.timespan
        {
            self.sum -= rates[wrap(self.first)] as i64;
            self.samples -= 1;
            self.first += 1;
        }

        let first_ts = timestamps[wrap(self.first)];
        let covered = last_ts.saturating_sub(first_ts);
        // filled once the coverage reaches the window (one period tolerance)
        let filled = covered + PERIOD_MS >= self.timespan;
        self.preliminary = !filled;

        self.avg = self.sum as f64 / self.samples.max(1) as f64 / 1024.0;
        self.display = if self.preliminary {
            let fill = ((covered + PERIOD_MS) as f64 / self.timespan as f64).min(1.0);
            self.avg * fill
        } else {
            self.avg
        };
        self.timestamp = last_ts;
    }
}

/// One exported sample: the averages and temperatures * 100, the time
/// relative to the range start.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Row {
    pub hashrate_1m: c_int,
    pub hashrate_10m: c_int,
    pub hashrate_1h: c_int,
    pub hashrate_1d: c_int,
    pub vreg_temp: c_int,
    pub asic_temp: c_int,
    pub timestamp: i64,
}

pub struct History {
    /// samples pushed so far (the ring holds the last SAMPLES)
    n: i64,
    /// rates in GH/s, Q10
    rates: Vec<u32>,
    timestamps: Vec<u64>,
    /// the four averages' display values per sample
    averages: [Vec<f32>; 4],
    vreg_temps: Vec<f32>,
    asic_temps: Vec<f32>,
    avgs: [Avg; 4],
    shares: Vec<u32>,
}

/// The averages' windows: 1 min, 10 min, 1 h, 1 d.
const WINDOWS_MS: [u64; 4] = [60_000, 600_000, 3_600_000, 86_400_000];

/// A zeroed buffer; None if the memory is not there (no PSRAM).
fn buffer<T: Clone + Default>() -> Option<Vec<T>> {
    let mut v = Vec::new();
    v.try_reserve_exact(SAMPLES).ok()?;
    v.resize(SAMPLES, T::default());
    Some(v)
}

impl History {
    pub fn new(chips: usize) -> Option<Self> {
        Some(Self {
            n: 0,
            rates: buffer()?,
            timestamps: buffer()?,
            averages: [buffer()?, buffer()?, buffer()?, buffer()?],
            vreg_temps: buffer()?,
            asic_temps: buffer()?,
            avgs: WINDOWS_MS.map(Avg::new),
            shares: vec![0; chips],
        })
    }

    /// A measured hashrate (GH/s) with the temperatures at `timestamp` (ms).
    pub fn push(&mut self, rate: f32, vreg_temp: f32, asic_temp: f32, timestamp: u64) {
        let rate = if rate.is_finite() && rate >= 0.0 {
            rate
        } else {
            0.0
        };
        let i = wrap(self.n);
        // Q22.10; saturates for absurd rates
        self.rates[i] = (rate as f64 * 1024.0) as u32;
        self.vreg_temps[i] = vreg_temp;
        self.asic_temps[i] = asic_temp;
        self.timestamps[i] = timestamp;
        self.n += 1;
        for (avg, values) in self.avgs.iter_mut().zip(&mut self.averages) {
            avg.update(self.n, &self.rates, &self.timestamps);
            values[i] = avg.display as f32;
        }
    }

    /// The log line of a push.
    fn summary(&self) -> String {
        let a = |i: usize| {
            let avg = &self.avgs[i];
            let mark = if avg.preliminary { '*' } else { ' ' };
            format!("{}GH{mark}", fixed(avg.avg as f32 as f64, 3))
        };
        format!("hashrate: 1m:{} 10m:{} 1h:{} 1d:{}", a(0), a(1), a(2), a(3))
    }

    /// Counts a share of chip `asic`; the log line with all counts.
    pub fn push_share(&mut self, asic: c_int) -> Option<String> {
        if self.shares.is_empty() {
            return None;
        }
        if let Some(c) = usize::try_from(asic)
            .ok()
            .and_then(|i| self.shares.get_mut(i))
        {
            *c += 1;
        }
        let counts: Vec<String> = self.shares.iter().map(u32::to_string).collect();
        Some(format!("nonce distribution: {}", counts.join("/")))
    }

    /// The display average of window 0..3 (1m, 10m, 1h, 1d).
    pub fn current(&self, window: usize) -> f64 {
        self.avgs.get(window).map_or(0.0, |a| a.display)
    }

    /// The timestamp of the newest sample.
    pub fn current_timestamp(&self) -> u64 {
        self.avgs[1].timestamp
    }

    /// Index of the sample nearest to `timestamp` (the C++ bisection over
    /// the unwrapped indices, with its exact result), or -1.
    fn search(&self, timestamp: i64) -> i64 {
        let mut lo = (self.n - SAMPLES as i64).max(0);
        let mut hi = self.n - 1;
        let mut current;
        loop {
            // C integer division (truncating; hi can be -1)
            current = (hi + lo) / 2;
            // one element (or none) left
            if hi - lo < 1 {
                break;
            }
            let stored = self.timestamps[wrap(current)] as i64;
            match stored.cmp(&timestamp) {
                core::cmp::Ordering::Greater => hi = current - 1,
                core::cmp::Ordering::Less => lo = current + 1,
                core::cmp::Ordering::Equal => return current,
            }
        }
        if current < 0 || current >= self.n {
            -1
        } else {
            current
        }
    }

    /// The samples between `start` and `end` (client time, ms), with the
    /// client's `current` time and our `now_ms`; at most `limit` rows (0: no
    /// limit). Returns whether there are more.
    pub fn export(
        &self,
        start: u64,
        end: u64,
        current: u64,
        limit: u32,
        now_ms: u64,
        mut row: impl FnMut(Row),
    ) -> bool {
        // the timestamps come from the request: wrapping
        let sys_start = now_ms.wrapping_add(start.wrapping_sub(current)) as i64;
        let sys_end = now_ms.wrapping_add(end.wrapping_sub(current)) as i64;
        let first = self.search(sys_start);
        let last = self.search(sys_end);
        let mut count = last - first + 1;
        if first == -1 || last == -1 || count <= 0 {
            log!(
                Warn,
                TAG,
                "Invalid history indices or history not (yet) available"
            );
            count = 0;
        }

        let mut left = limit;
        let mut last_ts = 0i64;
        for i in first..first + count {
            if limit != 0 && left == 0 {
                log!(
                    Warn,
                    TAG,
                    "history response limited to {limit} points. Last timestamp {last_ts}"
                );
                return true;
            }
            let j = wrap(i);
            let ts = self.timestamps[j] as i64;
            if ts < sys_start {
                continue;
            }
            // as (int)(value * 100.0f), saturating
            let x100 = |v: f32| (v * 100.0) as c_int;
            row(Row {
                hashrate_1m: x100(self.averages[0][j]),
                hashrate_10m: x100(self.averages[1][j]),
                hashrate_1h: x100(self.averages[2][j]),
                hashrate_1d: x100(self.averages[3][j]),
                vreg_temp: x100(self.vreg_temps[j]),
                asic_temp: x100(self.asic_temps[j]),
                timestamp: ts.wrapping_sub(sys_start),
            });
            left = left.wrapping_sub(1);
            last_ts = ts;
        }
        false
    }
}

static HISTORY: Mutex<Option<History>> = Mutex::new(None);

/// The global history's export (see `History::export`); false (after a
/// warning) if the history is not available.
pub fn export(
    start: u64,
    end: u64,
    current: u64,
    limit: u32,
    now_ms: u64,
    row: impl FnMut(Row),
) -> bool {
    let more = with(|h| h.export(start, end, current, limit, now_ms, row));
    if more.is_none() {
        log!(
            Warn,
            TAG,
            "Invalid history indices or history not (yet) available"
        );
    }
    more.unwrap_or(false)
}

/// Allocates the history (about 1.2 MB, PSRAM); false without memory.
pub fn init(asic_count: usize) -> bool {
    log!(Info, TAG, "History size {SAMPLES} samples");
    let h = History::new(asic_count);
    let ok = h.is_some();
    *HISTORY.lock().unwrap_or_else(|e| e.into_inner()) = h;
    ok
}

/// A sample (every 5 s).
pub fn push(rate: f32, vreg_temp: f32, asic_temp: f32, timestamp_ms: u64) {
    let line = with(|h| {
        h.push(rate, vreg_temp, asic_temp, timestamp_ms);
        h.summary()
    });
    match line {
        Some(l) => log!(Info, TAG, "{l}"),
        None => log!(Warn, TAG, "PSRAM not initialized"),
    }
}

fn with<T>(f: impl FnOnce(&mut History) -> T) -> Option<T> {
    HISTORY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

pub(crate) mod api {
    use super::*;

    pub fn history_init(asic_count: c_int) -> bool {
        init(usize::try_from(asic_count).unwrap_or(0))
    }

    pub fn history_push(rate: f32, vreg_temp: f32, asic_temp: f32, timestamp_ms: u64) {
        push(rate, vreg_temp, asic_temp, timestamp_ms)
    }

    pub fn history_push_share(asic: c_int) {
        if let Some(Some(line)) = with(|h| h.push_share(asic)) {
            log!(Info, TAG, "{line}");
        }
    }

    /// The display average of window 0..3 (1m, 10m, 1h, 1d).
    pub fn history_current(window: c_int) -> f64 {
        with(|h| h.current(usize::try_from(window).unwrap_or(usize::MAX))).unwrap_or(0.0)
    }

    pub fn history_current_timestamp() -> u64 {
        with(|h| h.current_timestamp()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(SAMPLES, 32768);
    }

    #[test]
    fn averages_ramp_up_and_slide() {
        let mut h = History::new(4).unwrap();
        // 1000 GH/s every 5 s
        for i in 0..12u64 {
            h.push(1000.0, 50.0, 60.0, 10_000 + i * 5000);
        }
        // 1 min window: 12 samples cover 55 s + one period: filled
        assert!(!h.avgs[0].preliminary);
        assert!((h.current(0) - 1000.0).abs() < 0.01);
        // 10 min window: 60 s of 600 s: display ramps at 10 %
        assert!(h.avgs[1].preliminary);
        assert!((h.current(1) - 100.0).abs() < 0.01);
        assert!((h.avgs[1].avg - 1000.0).abs() < 0.01);
        assert_eq!(h.current_timestamp(), 10_000 + 11 * 5000);

        // the 1 min window drops old samples
        for i in 12..24u64 {
            h.push(2000.0, 50.0, 60.0, 10_000 + i * 5000);
        }
        assert!((h.current(0) - 2000.0).abs() < 0.01);
        assert!((h.avgs[1].avg - 1500.0).abs() < 0.01);
        assert!(h
            .summary()
            .starts_with("hashrate: 1m:2000.000GH  10m:1500.000GH*"));
    }

    #[test]
    fn bad_rates_are_zero_or_saturate() {
        let mut h = History::new(0).unwrap();
        h.push(f32::NAN, 0.0, 0.0, 1);
        h.push(-5.0, 0.0, 0.0, 2);
        h.push(1e12, 0.0, 0.0, 3);
        assert_eq!(&h.rates[..3], &[0, 0, u32::MAX]);
    }

    #[test]
    fn search_finds_nearest() {
        let mut h = History::new(0).unwrap();
        assert_eq!(h.search(5), -1);
        for i in 0..10u64 {
            h.push(1.0, 0.0, 0.0, 1000 + i * 5000);
        }
        assert_eq!(h.search(1000), 0);
        assert_eq!(h.search(1000 + 9 * 5000), 9);
        assert_eq!(h.search(16_000), 3);
        // outside: clamped to the ends
        assert_eq!(h.search(0), 0);
        assert_eq!(h.search(1_000_000), 9);
    }

    #[test]
    fn export_range_and_limit() {
        let mut h = History::new(0).unwrap();
        for i in 0..10u64 {
            h.push(1000.0, 45.5, 60.25, 1000 + i * 5000);
        }
        // client time = our time + 1e9
        let off = 1_000_000_000u64;
        let now = 50_000;
        let mut rows = Vec::new();
        let more = h.export(off + 11_000, off + 31_000, off + now, 0, now, |r| {
            rows.push(r)
        });
        assert!(!more);
        // 11 s to 31 s, both ends included
        assert_eq!(rows.len(), 5);
        assert_eq!(rows[0].timestamp, 0);
        assert_eq!(rows[1].timestamp, 5000);
        assert_eq!(rows[0].vreg_temp, 4550);
        assert_eq!(rows[0].asic_temp, 6025);

        rows.clear();
        assert!(h.export(off, off + 60_000, off + now, 3, now, |r| rows.push(r)));
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn share_distribution() {
        let mut h = History::new(3).unwrap();
        h.push_share(1);
        h.push_share(7);
        h.push_share(-1);
        assert_eq!(h.push_share(1).unwrap(), "nonce distribution: 0/2/0");
        assert_eq!(History::new(0).unwrap().push_share(0), None);
    }
}
