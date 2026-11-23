//! The heartbeat: every N hours a webhook message with mining statistics
//! (a keepalive for home automation / monitoring). The text is a template
//! with placeholders like `{hashrate}`; unknown placeholders stay as they
//! are. The system task calls [`tick`]; the message goes through the alert
//! queue to the configured webhook.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::log::log;

const TAG: &CStr = c"heartbeat";

/// The longest interval (a week).
pub const MAX_HOURS: u16 = 168;
/// The template is cut to this many bytes.
pub const TEMPLATE_MAX: usize = 500;
pub const DEFAULT_TEMPLATE: &str = "{hostname}: {hashrate} GH/s, {temp} °C, {power} W, \
shares {accepted}/{rejected}, best {bestdiff}, uptime {uptime}";
/// The placeholders, for the help text and the tests.
pub const PLACEHOLDERS: &[&str] = &[
    "hostname",
    "ip",
    "hashrate",
    "hashrate_1h",
    "hashrate_1d",
    "temp",
    "vrtemp",
    "power",
    "efficiency",
    "accepted",
    "rejected",
    "bestdiff",
    "sessiondiff",
    "blocks",
    "uptime",
];

/// The values for the placeholders.
#[derive(Clone, Debug, Default)]
pub struct Values {
    pub hostname: String,
    pub ip: String,
    /// GH/s
    pub hashrate: f64,
    pub hashrate_1h: f64,
    pub hashrate_1d: f64,
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

impl Values {
    fn get(&self, name: &str) -> Option<String> {
        let diff = |d| crate::suffix::suffix_string(d, 0);
        Some(match name {
            "hostname" => self.hostname.clone(),
            "ip" => self.ip.clone(),
            "hashrate" => format!("{:.1}", self.hashrate),
            "hashrate_1h" => format!("{:.1}", self.hashrate_1h),
            "hashrate_1d" => format!("{:.1}", self.hashrate_1d),
            "temp" => format!("{:.1}", self.temp),
            "vrtemp" => format!("{:.1}", self.vrtemp),
            "power" => format!("{:.1}", self.power),
            // J/TH
            "efficiency" if self.hashrate > 0.0 => {
                format!("{:.1}", self.power as f64 / (self.hashrate / 1000.0))
            }
            "efficiency" => "-".into(),
            "accepted" => self.accepted.to_string(),
            "rejected" => self.rejected.to_string(),
            "bestdiff" => diff(self.best_diff),
            "sessiondiff" => diff(self.session_diff),
            "blocks" => self.blocks.to_string(),
            "uptime" => uptime(self.uptime_s),
            _ => return None,
        })
    }
}

/// The template with its placeholders replaced.
pub fn render(template: &str, v: &Values) -> String {
    let mut out = String::with_capacity(template.len() + 64);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        match after
            .find('}')
            .and_then(|close| Some((close, v.get(&after[..close])?)))
        {
            Some((close, value)) => {
                out.push_str(&value);
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The template as stored: cut to [`TEMPLATE_MAX`], the default if empty.
pub fn template_or_default(t: &str) -> String {
    let t = t.trim();
    if t.is_empty() {
        return DEFAULT_TEMPLATE.into();
    }
    let mut end = t.len().min(TEMPLATE_MAX);
    while !t.is_char_boundary(end) {
        end -= 1;
    }
    t[..end].into()
}

/// When the next heartbeat is due.
#[derive(Default)]
struct Schedule {
    hours: u16,
    next_us: i64,
}

impl Schedule {
    /// Whether a heartbeat is due at `now_us` with `hours` (0: off); a
    /// changed interval starts over from now.
    fn due(&mut self, hours: u16, now_us: i64) -> bool {
        if hours == 0 {
            self.hours = 0;
            return false;
        }
        let period = hours as i64 * 3_600_000_000;
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

/// Every tick of the system task: queues the heartbeat when it is due.
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
    let msg = render(&c.heartbeat_template, &values(now_us));
    if !crate::alerts::enqueue_plain(&msg) {
        log!(Warn, TAG, "alert queue full, heartbeat dropped");
    }
}

#[cfg(target_os = "espidf")]
fn values(now_us: i64) -> Values {
    use crate::history::api::history_current;
    use crate::power_mgmt::get;
    let s = crate::pool_manager::get()
        .map(|m| m.summary())
        .unwrap_or_default();
    Values {
        hostname: crate::system::hostname().to_string_lossy().into_owned(),
        ip: crate::network::preferred_ip().map_or_else(|| "0.0.0.0".into(), |(ip, _)| ip),
        hashrate: crate::system::current_hashrate() as f64,
        hashrate_1h: history_current(2),
        hashrate_1d: history_current(3),
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

    fn sample() -> Values {
        Values {
            hostname: "rig".into(),
            ip: "10.0.0.2".into(),
            hashrate: 1234.56,
            hashrate_1h: 1200.0,
            hashrate_1d: 1190.04,
            temp: 55.25,
            vrtemp: 61.0,
            power: 20.0,
            accepted: 42,
            rejected: 1,
            best_diff: 4_200_000_000,
            session_diff: 1_234_567,
            blocks: 0,
            uptime_s: 2 * 86_400 + 3 * 3600 + 7 * 60 + 5,
        }
    }

    #[test]
    fn renders_placeholders() {
        assert_eq!(
            render(DEFAULT_TEMPLATE, &sample()),
            "rig: 1234.6 GH/s, 55.2 °C, 20.0 W, shares 42/1, best 4.2G, uptime 2d 03h 07m"
        );
        assert_eq!(
            render(
                "{ip} {hashrate_1h} {hashrate_1d} {vrtemp} {sessiondiff} {blocks}",
                &sample()
            ),
            "10.0.0.2 1200.0 1190.0 61.0 1.23M 0"
        );
        assert_eq!(render("{efficiency} J/TH", &sample()), "16.2 J/TH");
        assert_eq!(render("{efficiency}", &Values::default()), "-");
        assert_eq!(uptime(59 * 60), "00h 59m");
        for p in PLACEHOLDERS {
            assert!(sample().get(p).is_some(), "{p}");
        }
    }

    #[test]
    fn keeps_unknown_and_unbalanced_braces() {
        let v = sample();
        assert_eq!(render("{nope} {temp", &v), "{nope} {temp");
        assert_eq!(render("{{hostname}}", &v), "{rig}");
        assert_eq!(render("}{", &v), "}{");
        assert_eq!(render("ä{hostname}ö", &v), "ärigö");
        assert_eq!(render("", &v), "");
    }

    #[test]
    fn template_is_cut_and_defaulted() {
        assert_eq!(template_or_default("  "), DEFAULT_TEMPLATE);
        let t = template_or_default(&"ä".repeat(400));
        assert!(t.len() <= TEMPLATE_MAX && t.len() >= TEMPLATE_MAX - 1);
    }

    #[test]
    fn schedule() {
        let h = 3_600_000_000;
        let mut s = Schedule::default();
        assert!(!s.due(0, 0));
        // first one an interval after it was enabled
        assert!(!s.due(2, 10));
        assert!(!s.due(2, 2 * h));
        assert!(s.due(2, 2 * h + 10));
        assert!(!s.due(2, 3 * h));
        assert!(s.due(2, 4 * h + 10));
        // a long stall: one heartbeat, no burst
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
