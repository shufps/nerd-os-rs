//! The WiFi station's decisions (port of the event handler and scan logic of
//! `main/network/connect.cpp`): connect on start, reconnect after a
//! disconnect with a 1 s backoff, five retries before "Connect Failed!" (and
//! then on in the background), no reconnect without an SSID, during a scan
//! or while WiFi is paused (Ethernet has the IP), the IP address, the
//! connected/failed event bits, the status shown and the hooks of the
//! network manager. The C++ side carries out the returned actions (`esp_wifi_*`,
//! event bits, status, hooks, the backoff timer) in their bit order, the
//! order of the former code.
//!
//! Behaviour notes:
//! - the backoff is a timer (`act::BACKOFF`, then `backoff_done()`), not a
//!   delay inside the handler, so the default event loop (every WiFi,
//!   Ethernet and IP event) keeps running;
//! - after the backoff the reconnect is also skipped when a scan started
//!   meanwhile, and the end of a scan does not reconnect while WiFi is
//!   paused (WiFi is stopped then);
//! - the state is behind a lock (it is written by the event loop, the HTTP
//!   scan handler and the network manager); starting a scan is
//!   check-and-set, so two scan requests start only one.

use core::ffi::{c_char, c_int};
use std::sync::Mutex;

/// Actions, carried out in this bit order.
pub mod act {
    pub const CLEAR_CONNECTED_BIT: u32 = 1 << 0;
    /// the network manager's disconnect hook
    pub const HOOK_DISCONNECTED: u32 = 1 << 1;
    pub const SET_FAIL_BIT: u32 = 1 << 2;
    /// status "Connect Failed!"
    pub const STATUS_FAILED: u32 = 1 << 3;
    /// `esp_wifi_connect()`
    pub const CONNECT: u32 = 1 << 4;
    /// status "Retrying: n" (n: `retries()`)
    pub const STATUS_RETRYING: u32 = 1 << 5;
    pub const SET_CONNECTED_BIT: u32 = 1 << 6;
    /// status "Connected!"
    pub const STATUS_CONNECTED: u32 = 1 << 7;
    /// the network manager's got-IP hook
    pub const HOOK_GOT_IP: u32 = 1 << 8;
    /// status "LAN Connected!"
    pub const STATUS_PAUSED: u32 = 1 << 9;
    /// start the 1 s backoff timer, then `backoff_done()`
    pub const BACKOFF: u32 = 1 << 10;
}

/// Retries before "Connect Failed!" (`WIFI_MAXIMUM_RETRY`).
pub const MAX_RETRY: u16 = 5;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub has_ssid: bool,
    pub retries: u16,
    pub paused: bool,
    pub scanning: bool,
    pub scan_suppress: bool,
    backoff: bool,
    /// the address while connected
    pub ip: Option<[u8; 4]>,
}

impl State {
    /// `WIFI_EVENT_STA_START`
    pub fn sta_start(&mut self) -> u32 {
        if self.has_ssid && !self.paused {
            act::CONNECT
        } else {
            0
        }
    }

    /// `WIFI_EVENT_STA_DISCONNECTED`
    pub fn disconnected(&mut self) -> u32 {
        self.ip = None;
        let a = act::CLEAR_CONNECTED_BIT | act::HOOK_DISCONNECTED;
        // no SSID configured, scan in progress or WiFi paused: no reconnect
        if !self.has_ssid || self.scan_suppress || self.paused || self.backoff {
            return a;
        }
        self.backoff = true;
        a | act::BACKOFF
    }

    /// The backoff after a disconnect has passed.
    pub fn backoff_done(&mut self) -> u32 {
        self.backoff = false;
        if !self.has_ssid || self.scan_suppress || self.paused || self.ip.is_some() {
            return 0;
        }
        if self.retries < MAX_RETRY {
            self.retries += 1;
            act::CONNECT | act::STATUS_RETRYING
        } else {
            // signal the initial failure, keep reconnecting in the background
            self.retries = 0;
            act::SET_FAIL_BIT | act::STATUS_FAILED | act::CONNECT
        }
    }

    /// `IP_EVENT_STA_GOT_IP`
    pub fn got_ip(&mut self, ip: [u8; 4]) -> u32 {
        self.ip = Some(ip);
        self.retries = 0;
        act::SET_CONNECTED_BIT | act::STATUS_CONNECTED | act::HOOK_GOT_IP
    }

    /// WiFi stopped (or restarted) on purpose by the network manager.
    pub fn pause(&mut self, paused: bool) -> u32 {
        self.paused = paused;
        self.retries = 0;
        if paused {
            act::STATUS_PAUSED
        } else {
            0
        }
    }

    /// A scan is to start: false if one is running. Suppresses reconnects
    /// until `scan_finish()`.
    pub fn scan_begin(&mut self) -> bool {
        if self.scanning {
            return false;
        }
        self.scanning = true;
        self.scan_suppress = true;
        true
    }

    /// `WIFI_EVENT_SCAN_DONE`
    pub fn scan_done(&mut self) {
        self.scanning = false;
    }

    /// The scan is over (done, failed or timed out): reconnect.
    pub fn scan_finish(&mut self) -> u32 {
        self.scanning = false;
        self.scan_suppress = false;
        if self.has_ssid && !self.paused {
            act::CONNECT
        } else {
            0
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    has_ssid: false,
    retries: 0,
    paused: false,
    scanning: false,
    scan_suppress: false,
    backoff: false,
    ip: None,
});

fn with<R>(f: impl FnOnce(&mut State) -> R) -> R {
    f(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The address as text ("0.0.0.0" while not connected).
pub fn ip_str(ip: Option<[u8; 4]>) -> String {
    let [a, b, c, d] = ip.unwrap_or([0; 4]);
    format!("{a}.{b}.{c}.{d}")
}

pub(crate) mod ffi {
    use super::*;

    pub const EV_STA_START: c_int = 0;
    pub const EV_DISCONNECTED: c_int = 1;
    pub const EV_BACKOFF_DONE: c_int = 2;
    pub const EV_SCAN_DONE: c_int = 3;

    /// An event without data (`WIFI_EV_*`): the `WIFI_ACT_*` bits to carry
    /// out, in bit order.
    pub extern "C" fn wifi_state_event(ev: c_int) -> u32 {
        with(|s| match ev {
            EV_STA_START => s.sta_start(),
            EV_DISCONNECTED => s.disconnected(),
            EV_BACKOFF_DONE => s.backoff_done(),
            EV_SCAN_DONE => {
                s.scan_done();
                0
            }
            _ => 0,
        })
    }

    /// `IP_EVENT_STA_GOT_IP`; `ip`: the address bytes in order.
    ///
    /// # Safety
    /// `ip` points to 4 bytes.
    pub unsafe extern "C" fn wifi_state_got_ip(ip: *const u8) -> u32 {
        let mut a = [0u8; 4];
        a.copy_from_slice(core::slice::from_raw_parts(ip, 4));
        with(|s| s.got_ip(a))
    }

    pub extern "C" fn wifi_state_pause(paused: bool) -> u32 {
        with(|s| s.pause(paused))
    }

    pub extern "C" fn wifi_state_set_has_ssid(has_ssid: bool) {
        with(|s| s.has_ssid = has_ssid)
    }

    pub extern "C" fn wifi_state_retries() -> u16 {
        with(|s| s.retries)
    }

    pub extern "C" fn wifi_state_scan_begin() -> bool {
        with(|s| s.scan_begin())
    }

    pub extern "C" fn wifi_state_scanning() -> bool {
        with(|s| s.scanning)
    }

    pub extern "C" fn wifi_state_scan_finish() -> u32 {
        with(|s| s.scan_finish())
    }

    /// The address into `buf` (always terminated); true while connected.
    ///
    /// # Safety
    /// `buf` writable for `len` bytes or NULL.
    pub unsafe extern "C" fn wifi_state_ip(buf: *mut c_char, len: usize) -> bool {
        let ip = with(|s| s.ip);
        if !buf.is_null() && len > 0 {
            let t = ip_str(ip);
            let n = t.len().min(len - 1);
            core::ptr::copy_nonoverlapping(t.as_ptr(), buf as *mut u8, n);
            *buf.add(n) = 0;
        }
        ip.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::act::*;
    use super::*;

    fn configured() -> State {
        State {
            has_ssid: true,
            ..State::default()
        }
    }

    #[test]
    fn retries_then_failed_then_on() {
        let mut s = configured();
        assert_eq!(s.sta_start(), CONNECT);
        for n in 1..=MAX_RETRY {
            assert_eq!(
                s.disconnected(),
                CLEAR_CONNECTED_BIT | HOOK_DISCONNECTED | BACKOFF
            );
            assert_eq!(s.backoff_done(), CONNECT | STATUS_RETRYING);
            assert_eq!(s.retries, n);
        }
        s.disconnected();
        assert_eq!(s.backoff_done(), SET_FAIL_BIT | STATUS_FAILED | CONNECT);
        assert_eq!(s.retries, 0);
        s.disconnected();
        assert_eq!(s.backoff_done(), CONNECT | STATUS_RETRYING);
        assert_eq!(
            s.got_ip([192, 168, 0, 199]),
            SET_CONNECTED_BIT | STATUS_CONNECTED | HOOK_GOT_IP
        );
        assert_eq!(s.retries, 0);
        assert_eq!(ip_str(s.ip), "192.168.0.199");
        s.disconnected();
        assert_eq!(ip_str(s.ip), "0.0.0.0");
    }

    #[test]
    fn no_ssid_no_reconnect() {
        let mut s = State::default();
        assert_eq!(s.sta_start(), 0);
        assert_eq!(s.disconnected(), CLEAR_CONNECTED_BIT | HOOK_DISCONNECTED);
        assert_eq!(s.scan_finish(), 0);
    }

    #[test]
    fn paused() {
        let mut s = configured();
        assert_eq!(s.pause(true), STATUS_PAUSED);
        assert_eq!(s.disconnected(), CLEAR_CONNECTED_BIT | HOOK_DISCONNECTED);
        assert_eq!(s.sta_start(), 0);
        // paused during the backoff
        let mut s = configured();
        assert_eq!(s.disconnected() & BACKOFF, BACKOFF);
        s.pause(true);
        assert_eq!(s.backoff_done(), 0);
        assert_eq!(s.pause(false), 0);
        assert_eq!(s.sta_start(), CONNECT);
    }

    #[test]
    fn one_backoff_at_a_time() {
        let mut s = configured();
        assert_eq!(s.disconnected() & BACKOFF, BACKOFF);
        assert_eq!(s.disconnected() & BACKOFF, 0);
        assert_eq!(s.backoff_done(), CONNECT | STATUS_RETRYING);
        assert_eq!(s.disconnected() & BACKOFF, BACKOFF);
    }

    #[test]
    fn scan() {
        let mut s = configured();
        assert!(s.scan_begin());
        assert!(!s.scan_begin());
        // the scan's disconnect does not reconnect
        assert_eq!(s.disconnected(), CLEAR_CONNECTED_BIT | HOOK_DISCONNECTED);
        s.scan_done();
        assert!(!s.scanning);
        assert_eq!(s.scan_finish(), CONNECT);
        assert!(s.scan_begin());
        s.pause(true);
        assert_eq!(s.scan_finish(), 0);
        // a scan started during the backoff
        let mut s = configured();
        s.disconnected();
        s.scan_begin();
        assert_eq!(s.backoff_done(), 0);
    }
}
