//! Pool ping statistics (port of `PingTask` in `ping_task.cpp`): every
//! 60 s a round of 7 pings to the pool's resolved address, the average
//! round trip time and the packet loss over the last 15 minutes. The ICMP
//! session is `ping_session.rs`, the loop is run by the pool manager.
//!
//! The statistics lock is only held to read or update the numbers, never
//! across a ping round (about 9 s) and never together with the manager
//! lock, so `pool_mgr_load_settings()` (manager lock, then
//! `reset()`) never waits for a round.

use core::ffi::c_int;
use std::sync::Mutex;
use std::time::Duration;

use crate::cutil::fixed;

/// Pings per round.
pub const COUNT: u16 = 7;
const ROUND_DELAY: Duration = Duration::from_secs(60);
/// Rounds in the loss window (15 min).
const HISTORY: usize = 900 / 60;

/// The statistics of one pool.
#[derive(Clone, Copy)]
pub struct Stats {
    /// (sent, received) per round
    history: [(u16, u16); HISTORY],
    index: usize,
    count: usize,
    last_rtt_ms: f64,
}

impl Stats {
    const fn new() -> Self {
        Self {
            history: [(0, 0); HISTORY],
            index: 0,
            count: 0,
            last_rtt_ms: 0.0,
        }
    }

    pub fn record(&mut self, sent: u16, received: u16) {
        self.history[self.index] = (sent, received);
        self.index = (self.index + 1) % HISTORY;
        self.count = (self.count + 1).min(HISTORY);
    }

    /// Lost share (0..1) over the window.
    pub fn loss(&self) -> f64 {
        let (sent, recv) = self.history[..self.count]
            .iter()
            .fold((0u32, 0u32), |(s, r), &(hs, hr)| {
                (s + hs as u32, r + hr as u32)
            });
        if sent == 0 {
            0.0
        } else {
            sent.saturating_sub(recv) as f64 / sent as f64
        }
    }
}

static STATS: Mutex<[Stats; 2]> = Mutex::new([Stats::new(); 2]);

fn with<T>(pool: c_int, f: impl FnOnce(&mut Stats) -> T) -> Option<T> {
    let i = usize::try_from(pool).ok().filter(|&i| i < 2)?;
    Some(f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner())[i]))
}

/// Result of a ping round (`PingResult`).
#[derive(Clone, Copy, Debug, Default)]
pub struct Round {
    pub success: bool,
    pub replies: u16,
    pub avg_rtt_ms: f64,
    pub min_rtt_ms: f64,
    pub max_rtt_ms: f64,
}

/// What the ping loop needs from the firmware.
pub trait Env {
    /// True if the board is shut down (the loop resets and suspends).
    fn shutdown(&mut self) -> bool;
    fn suspend(&mut self);
    fn connected(&mut self) -> bool;
    /// The pool's host name and resolved address (None: not resolved).
    fn target(&mut self) -> Option<(String, String)>;
    /// A round of `COUNT` pings to `ip`.
    fn ping(&mut self, ip: &str, host: &str) -> Round;
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
    fn log(&mut self, line: &str);
}

/// One round: ping, update the statistics, log like the C++ task.
pub fn round(pool: c_int, env: &mut impl Env) {
    let Some((host, ip)) = env.target() else {
        env.log("No resolved IP for current hostname");
        return;
    };
    // no lock while pinging
    let r = env.ping(&ip, &host);
    let loss = with(pool, |s| {
        if r.success {
            s.last_rtt_ms = r.avg_rtt_ms;
        }
        s.record(COUNT, r.replies);
        s.loss()
    })
    .unwrap_or(0.0);

    env.log(&format!("--- {host} ping statistics ---"));
    let current = 100.0 * (COUNT.saturating_sub(r.replies)) as f64 / COUNT as f64;
    env.log(&format!(
        "{COUNT} packets transmitted, {} packets received, {}% packet loss",
        r.replies,
        fixed(current, 1)
    ));
    if r.success {
        env.log(&format!(
            "round-trip min/avg/max = {}/{}/{} ms",
            fixed(r.min_rtt_ms, 2),
            fixed(r.avg_rtt_ms, 2),
            fixed(r.max_rtt_ms, 2)
        ));
    }
    env.log(&format!(
        "Recent {}-min packet loss: {}%",
        HISTORY,
        fixed(100.0 * loss, 1)
    ));
}

/// The ping loop of `pool`; never returns.
pub fn run(pool: c_int, env: &mut impl Env) -> ! {
    loop {
        if env.shutdown() {
            reset(pool);
            env.suspend();
        }
        if !env.connected() {
            env.sleep(Duration::from_secs(10));
            continue;
        }
        round(pool, env);
        env.sleep(ROUND_DELAY);
    }
}

/// Clears the statistics (settings changed, reconnect, shutdown).
pub fn reset(pool: c_int) {
    with(pool, |s| *s = Stats::new());
}

pub(crate) mod api {
    use super::*;

    /// Average round trip time of the last successful round (ms).
    pub fn ping_last_rtt(pool: c_int) -> f64 {
        with(pool, |s| s.last_rtt_ms).unwrap_or(0.0)
    }

    /// Lost share (0..1) over the last 15 minutes.
    pub fn ping_recent_loss(pool: c_int) -> f64 {
        with(pool, |s| s.loss()).unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_over_the_window() {
        let mut s = Stats::new();
        assert_eq!(s.loss(), 0.0);
        s.record(7, 7);
        s.record(7, 0);
        assert!((s.loss() - 0.5).abs() < 1e-9);
        // the window keeps 15 rounds
        for _ in 0..15 {
            s.record(7, 7);
        }
        assert_eq!(s.loss(), 0.0);
        // more received than sent (bogus): no underflow
        s.record(7, 9);
        assert_eq!(s.loss(), 0.0);
    }

    struct Fake {
        replies: u16,
        lines: Vec<String>,
        resolved: bool,
    }

    impl Env for Fake {
        fn shutdown(&mut self) -> bool {
            false
        }
        fn suspend(&mut self) {}
        fn connected(&mut self) -> bool {
            true
        }
        fn target(&mut self) -> Option<(String, String)> {
            self.resolved
                .then(|| ("pool.example".into(), "10.0.0.1".into()))
        }
        fn ping(&mut self, ip: &str, _host: &str) -> Round {
            // the statistics are not locked during the ping
            assert!(STATS.try_lock().is_ok());
            assert_eq!(ip, "10.0.0.1");
            Round {
                success: self.replies > 0,
                replies: self.replies,
                avg_rtt_ms: 41.5,
                min_rtt_ms: 40.0,
                max_rtt_ms: 43.0,
            }
        }
        fn log(&mut self, line: &str) {
            self.lines.push(line.into());
        }
    }

    #[test]
    fn a_round_updates_and_logs() {
        reset(1);
        let mut env = Fake {
            replies: 6,
            lines: Vec::new(),
            resolved: true,
        };
        round(1, &mut env);
        assert_eq!(
            env.lines,
            [
                "--- pool.example ping statistics ---",
                "7 packets transmitted, 6 packets received, 14.3% packet loss",
                "round-trip min/avg/max = 40.00/41.50/43.00 ms",
                "Recent 15-min packet loss: 14.3%",
            ]
        );
        assert_eq!(api::ping_last_rtt(1), 41.5);

        // a failed round keeps the last RTT
        env.replies = 0;
        env.lines.clear();
        round(1, &mut env);
        assert_eq!(env.lines.len(), 3);
        assert_eq!(api::ping_last_rtt(1), 41.5);
        assert!((api::ping_recent_loss(1) - 8.0 / 14.0).abs() < 1e-9);

        reset(1);
        assert_eq!(api::ping_last_rtt(1), 0.0);
        assert_eq!(api::ping_recent_loss(1), 0.0);

        env.resolved = false;
        env.lines.clear();
        round(1, &mut env);
        assert_eq!(env.lines, ["No resolved IP for current hostname"]);
    }
}
