//! The texts of the display (port of the formatting in
//! `main/displays/displayDriver.cpp`): the same strings as before, so the
//! screens look the same. Kept apart from the LVGL code to be testable on
//! the host. The C++ code formatted into 20-byte buffers; [`fit20`] keeps
//! that cut where a value could be longer.

/// `snprintf(buf, 20, ...)`: at most 19 bytes.
pub fn fit20(mut s: String) -> String {
    if s.len() > 19 {
        let mut n = 19;
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        s.truncate(n);
    }
    s
}

/// `formatHashrate()`: GH/s without decimals from 10000 on, else one.
pub fn hashrate(ghs: f32) -> String {
    if ghs >= 10000.0 {
        format!("{}", (ghs + 0.5) as i32)
    } else {
        format!("{:.1}", ghs)
    }
}

/// J/TH from watts and GH/s ("n/a" without hashrate).
pub fn efficiency(power: f32, ghs: f32) -> String {
    let th = ghs / 1000.0;
    let e = if th > 0.0 { power / th } else { 10000.0 };
    if e < 10000.0 {
        format!("{:.1}", e)
    } else {
        "n/a".into()
    }
}

/// `%.3fW`
pub fn watts(power: f32) -> String {
    format!("{:.3}W", power)
}

/// The uptime as "Xd Xh Xm Xs".
pub fn uptime(seconds: i64) -> String {
    let s = seconds.max(0);
    let (d, rest) = (s / 86400, s % 86400);
    fit20(format!(
        "{}d {}h {}m {}s",
        d,
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    ))
}

/// The fan setting of the settings page: AUTO, PID or the manual percent.
pub fn fan_setting(mode: i32, manual_speed: u16) -> String {
    match mode {
        1 => "AUTO".into(),
        2 => "PID".into(),
        _ => format!("{manual_speed}"),
    }
}

/// `%.0f` (the C rounding of the temperature).
pub fn degrees(t: f32) -> String {
    format!("{:.0}", t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_the_former_snprintf() {
        assert_eq!(hashrate(11172.4), "11172");
        assert_eq!(hashrate(11172.5), "11173");
        assert_eq!(hashrate(9968.62), "9968.6");
        assert_eq!(hashrate(0.0), "0.0");
        assert_eq!(efficiency(138.0, 10706.0), "12.9");
        assert_eq!(efficiency(5.0, 0.0), "n/a");
        assert_eq!(watts(137.0), "137.000W");
        assert_eq!(uptime(50), "0d 0h 0m 50s");
        assert_eq!(uptime(90061), "1d 1h 1m 1s");
        assert_eq!(fan_setting(1, 50), "AUTO");
        assert_eq!(fan_setting(2, 50), "PID");
        assert_eq!(fan_setting(0, 50), "50");
        assert_eq!(degrees(52.6), "53");
        assert_eq!(
            fit20("solo.ckpool.org.example.com".into()),
            "solo.ckpool.org.exa"
        );
    }
}
