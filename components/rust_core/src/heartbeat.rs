//! The metrics message: every 1 to 24 hours (0: off) a webhook message
//! with the mining statistics (a keepalive for home automation /
//! monitoring). The system task calls [`tick`]; the message goes through
//! the alert queue to the configured webhook.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::log::log;

const TAG: &CStr = c"heartbeat";

const HOUR_US: i64 = 3_600_000_000;
/// The longest interval (a day).
pub const MAX_HOURS: u16 = 24;

/// The values of the message.
#[derive(Clone, Debug, Default)]
pub struct Values {
    pub hostname: String,
    pub ip: String,
    /// GH/s
    pub hashrate: f64,
    pub hashrate_1h: f64,
    /// °C
    pub temp: f32,
    pub vrtemp: f32,
    /// W
    pub power: f32,
    pub accepted: u64,
    pub rejected: u64,
    pub best_diff: u64,
    pub session_diff: u64,
    pub blocks: u32,
    pub uptime_s: u64,
}

/// `3d 04h 12m`, `04h 12m` below a day.
fn uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s / 3600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d}d {h:02}h {m:02}m")
    } else {
        format!("{h:02}h {m:02}m")
    }
}

/// The message.
pub fn message(v: &Values) -> String {
    let diff = |d| crate::suffix::suffix_string(d, 0);
    let efficiency = if v.hashrate > 0.0 {
        format!("{:.1} J/TH", v.power as f64 / (v.hashrate / 1000.0))
    } else {
        "-".into()
    };
    format!(
        ":bar_chart: Metrics\n```\n\
Hostname: {}\n\
IP:       {}\n\
Hashrate: {:.1} GH/s (1h: {:.1} GH/s)\n\
Temp:     {:.1} °C (VR: {:.1} °C)\n\
Power:    {:.1} W ({efficiency})\n\
Shares:   {} accepted / {} rejected\n\
Best:     {} (session: {})\n\
Blocks:   {}\n\
Uptime:   {}\n```",
        v.hostname,
        v.ip,
        v.hashrate,
        v.hashrate_1h,
        v.temp,
        v.vrtemp,
        v.power,
        v.accepted,
        v.rejected,
        diff(v.best_diff),
        diff(v.session_diff),
        v.blocks,
        uptime(v.uptime_s),
    )
}

/// When the next message is due.
#[derive(Default)]
struct Schedule {
    hours: u16,
    next_us: i64,
}

impl Schedule {
    /// Whether a message is due at `now_us` with `hours` (0: off); a
    /// changed interval starts over from now.
    fn due(&mut self, hours: u16, now_us: i64) -> bool {
        if hours == 0 {
            self.hours = 0;
            return false;
        }
        let period = hours as i64 * HOUR_US;
        if hours != self.hours {
            self.hours = hours;
            self.next_us = now_us + period;
            return false;
        }
        if now_us < self.next_us {
            return false;
        }
        self.next_us += period;
        if self.next_us <= now_us {
            // no catching up after a long stall
            self.next_us = now_us + period;
        }
        true
    }
}

static SCHEDULE: Mutex<Schedule> = Mutex::new(Schedule {
    hours: 0,
    next_us: 0,
});

/// Every tick of the system task: queues the message when it is due.
pub fn tick(now_us: i64) {
    let c = crate::alerts::config();
    let hours = if c.webhook.is_empty() {
        0
    } else {
        c.heartbeat_hours
    };
    if !SCHEDULE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .due(hours, now_us)
    {
        return;
    }
    if !crate::alerts::enqueue_plain(&message(&values(now_us))) {
        log!(Warn, TAG, "alert queue full, metrics message dropped");
    }
}

#[cfg(target_os = "espidf")]
fn values(now_us: i64) -> Values {
    use crate::power_mgmt::get;
    let s = crate::pool::manager::get()
        .map(|m| m.summary())
        .unwrap_or_default();
    Values {
        hostname: crate::system::hostname().to_string_lossy().into_owned(),
        ip: crate::network::preferred_ip().map_or_else(|| "0.0.0.0".into(), |(ip, _)| ip),
        hashrate: crate::system::current_hashrate() as f64,
        hashrate_1h: crate::mining::history::api::history_current(2),
        temp: get(3),
        vrtemp: get(4),
        power: get(0),
        accepted: s.accepted,
        rejected: s.rejected,
        best_diff: s.total_best,
        session_diff: s.best_session,
        blocks: s.total_found_blocks,
        uptime_s: ((now_us - crate::system::start_us()) / 1_000_000).max(0) as u64,
    }
}

#[cfg(not(target_os = "espidf"))]
fn values(_now_us: i64) -> Values {
    Values::default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_text() {
        let v = Values {
            hostname: "rig".into(),
            ip: "10.0.0.2".into(),
            hashrate: 1234.56,
            hashrate_1h: 1200.0,
            temp: 55.25,
            vrtemp: 61.0,
            power: 20.0,
            accepted: 42,
            rejected: 1,
            best_diff: 4_200_000_000,
            session_diff: 1_234_567,
            blocks: 0,
            uptime_s: 2 * 86_400 + 3 * 3600 + 7 * 60 + 5,
        };
        assert_eq!(
            message(&v),
            ":bar_chart: Metrics\n```\nHostname: rig\nIP:       10.0.0.2\n\
Hashrate: 1234.6 GH/s (1h: 1200.0 GH/s)\nTemp:     55.2 °C (VR: 61.0 °C)\n\
Power:    20.0 W (16.2 J/TH)\nShares:   42 accepted / 1 rejected\n\
Best:     4.2G (session: 1.23M)\nBlocks:   0\nUptime:   2d 03h 07m\n```"
        );
        assert!(message(&Values::default()).contains("Power:    0.0 W (-)"));
        assert_eq!(uptime(59 * 60), "00h 59m");
        // fits the alert queue's cut
        assert!(message(&v).len() < 1000);
    }

    #[test]
    fn schedule() {
        let h = HOUR_US;
        let mut s = Schedule::default();
        assert!(!s.due(0, 0));
        // the first one an interval after it was enabled
        assert!(!s.due(2, 10));
        assert!(!s.due(2, 2 * h));
        assert!(s.due(2, 2 * h + 10));
        assert!(!s.due(2, 3 * h));
        assert!(s.due(2, 4 * h + 10));
        // a long stall: one message, no burst
        assert!(s.due(2, 20 * h));
        assert!(!s.due(2, 20 * h + 1));
        // a changed interval starts over
        assert!(!s.due(1, 21 * h));
        assert!(s.due(1, 22 * h));
        // off and on again starts over
        assert!(!s.due(0, 23 * h));
        assert!(!s.due(1, 23 * h));
        assert!(s.due(1, 24 * h));
    }
}
