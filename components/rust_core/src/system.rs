//! The system module (port of `main/system.cpp`): the device state the
//! HTTP API and the display read (start time, WiFi status, access point
//! state, host name, IP address, reset reason, board error), the current
//! hashrate, and the system task: every 5 s it updates the display (IP,
//! found block overlay, error overlay, values) and pushes a history sample.
//! The display is reached through [`crate::display`], the board through
//! `board/board_api.rs`.
//!
//! The strings that change at runtime (WiFi status, SSID, IP address) are
//! copied out under a lock, so a reader never sees a half-written string.

use core::ffi::{c_int, CStr};
use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use crate::log::log;

const TAG: &CStr = c"SystemModule";
/// The task period (`HR_INTERVAL`).
const PERIOD_US: i64 = 5_000_000;

/// `Board::Error`
pub mod error {
    use core::ffi::c_int;
    pub const NONE: c_int = 0;
    pub const TEMP_FAULT: c_int = 1;
    pub const VREG_TEMP_FAULT: c_int = 2;
    pub const PSU_FAULT: c_int = 3;
    pub const IOUT_OC_FAULT: c_int = 4;
    pub const VOUT_FAULT: c_int = 5;
    pub const COINBASE_VERIFY_FAULT: c_int = 6;
}

/// `Board::errorToStr()`
pub fn error_text(e: c_int) -> &'static CStr {
    match e {
        error::NONE => c"",
        error::TEMP_FAULT => c"MINER OVERHEATED",
        error::VREG_TEMP_FAULT => c"VREG OVERHEATED",
        error::PSU_FAULT => c"PSU FAULT",
        error::IOUT_OC_FAULT => c"CURRENT PROTECTION",
        error::VOUT_FAULT => c"VOLTAGE PROTECTION",
        error::COINBASE_VERIFY_FAULT => c"VERIFY FAILED",
        _ => c"INVALID ERROR",
    }
}

/// `esp_reset_reason_t`
mod rst {
    use crate::idf::idf_const;
    use core::ffi::c_int;

    idf_const!(pub UNKNOWN: c_int = esp_reset_reason_t_ESP_RST_UNKNOWN, 0);
    idf_const!(pub POWERON: c_int = esp_reset_reason_t_ESP_RST_POWERON, 1);
    idf_const!(pub EXT: c_int = esp_reset_reason_t_ESP_RST_EXT, 2);
    idf_const!(pub SW: c_int = esp_reset_reason_t_ESP_RST_SW, 3);
    idf_const!(pub PANIC: c_int = esp_reset_reason_t_ESP_RST_PANIC, 4);
    idf_const!(pub INT_WDT: c_int = esp_reset_reason_t_ESP_RST_INT_WDT, 5);
    idf_const!(pub TASK_WDT: c_int = esp_reset_reason_t_ESP_RST_TASK_WDT, 6);
    idf_const!(pub WDT: c_int = esp_reset_reason_t_ESP_RST_WDT, 7);
    idf_const!(pub DEEPSLEEP: c_int = esp_reset_reason_t_ESP_RST_DEEPSLEEP, 8);
    idf_const!(pub BROWNOUT: c_int = esp_reset_reason_t_ESP_RST_BROWNOUT, 9);
    idf_const!(pub SDIO: c_int = esp_reset_reason_t_ESP_RST_SDIO, 10);
}

/// `esp_reset_reason()` as the web UI's translation key.
pub fn reset_reason_text(reason: c_int) -> &'static CStr {
    match reason {
        rst::UNKNOWN => c"SYSTEM.RESET_UNKNOWN",
        rst::POWERON => c"SYSTEM.RESET_POWERON",
        rst::EXT => c"SYSTEM.RESET_EXTERNAL",
        rst::SW => c"SYSTEM.RESET_SOFTWARE",
        rst::PANIC => c"SYSTEM.RESET_PANIC",
        rst::INT_WDT => c"SYSTEM.RESET_INT_WATCHDOG",
        rst::TASK_WDT => c"SYSTEM.RESET_TASK_WATCHDOG",
        rst::WDT => c"SYSTEM.RESET_OTHER_WATCHDOG",
        rst::DEEPSLEEP => c"SYSTEM.RESET_DEEPSLEEP",
        rst::BROWNOUT => c"SYSTEM.RESET_BROWNOUT",
        rst::SDIO => c"SYSTEM.RESET_SDIO",
        _ => c"SYSTEM.RESET_NOT_SPECIFIED",
    }
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::board::board_api::*;
    pub use crate::idf::raw::esp_reset_reason;
}

/// On the host: no display, no board.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_reset_reason() -> c_int {
        1
    }
    pub fn bapi_is_can_slave() -> bool {
        false
    }
    pub fn bapi_has_hashrate_counter() -> bool {
        false
    }
}

struct State {
    start_us: i64,
    // without NUL
    ssid: Vec<u8>,
    wifi_status: Vec<u8>,
    ap_state: bool,
    startup_done: bool,
    ip: Vec<u8>,
    error: c_int,
    error_code: u32,
    reset_reason: &'static CStr,
}

static STATE: Mutex<State> = Mutex::new(State {
    start_us: 0,
    ssid: Vec::new(),
    wifi_status: Vec::new(),
    ap_state: false,
    startup_done: false,
    ip: Vec::new(),
    error: error::NONE,
    error_code: 0,
    reset_reason: c"SYSTEM.RESET_NOT_SPECIFIED",
});

/// The host name, loaded once at init (as the C++ code did).
static HOSTNAME: OnceLock<CString> = OnceLock::new();

fn state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|e| e.into_inner())
}

/// At most `max` bytes, cut at a NUL (as the C++ buffers did).
fn text(s: &[u8], max: usize) -> Vec<u8> {
    let s = crate::util::cutil::until_nul(s);
    s[..s.len().min(max)].to_vec()
}

fn cstring(v: &[u8]) -> CString {
    // no NUL inside (text() cuts there)
    CString::new(v).unwrap_or_default()
}

/// Start time, host name, the history for `asic_count` ASICs.
pub fn init(asic_count: c_int) {
    let hostname = crate::config::get_str(b"hostname")
        .map(|h| crate::util::cutil::until_nul(&h).to_vec())
        .unwrap_or_else(|| {
            crate::config::sdkconfig::LWIP_LOCAL_HOSTNAME
                .as_bytes()
                .to_vec()
        });
    let _ = HOSTNAME.set(CString::new(hostname).unwrap_or_default());
    {
        let mut s = state();
        s.start_us = crate::util::cutil::now_us();
        s.ip = b"0.0.0.0".to_vec();
    }
    if !crate::mining::history::api::history_init(asic_count) {
        log!(Error, TAG, "history couldn't be initialized!");
    }
}

pub fn hostname() -> &'static CStr {
    HOSTNAME.get().map_or(c"", CString::as_c_str)
}

pub fn start_us() -> i64 {
    state().start_us
}

pub fn set_wifi_status(status: &[u8]) {
    state().wifi_status = text(status, 19);
}

pub fn set_ssid(ssid: &[u8]) {
    state().ssid = text(ssid, 32);
}

pub fn set_ap_state(on: bool) {
    state().ap_state = on;
}

pub fn ap_state() -> bool {
    state().ap_state
}

pub fn set_startup_done() {
    state().startup_done = true;
}

pub fn startup_done() -> bool {
    state().startup_done
}

pub fn wifi_status() -> CString {
    cstring(&state().wifi_status)
}

pub fn ssid() -> CString {
    cstring(&state().ssid)
}

pub fn ip() -> CString {
    cstring(&state().ip)
}

fn set_ip(ip: &CStr) {
    let mut s = state();
    if s.ip != ip.to_bytes() {
        s.ip = ip.to_bytes().to_vec();
    }
}

/// A hardware fault (the power task): replaces any other error.
pub fn set_error(e: c_int, code: u32) {
    let mut s = state();
    s.error = e;
    s.error_code = code;
}

/// A non-hardware error (coinbase verification): only if there is none, so
/// it cannot hide (and later clear) a hardware fault.
pub fn set_error_if_none(e: c_int, code: u32) {
    let mut s = state();
    if s.error == error::NONE {
        s.error = e;
        s.error_code = code;
    }
}

/// Clears the error only if it is `e`.
pub fn clear_error(e: c_int) {
    let mut s = state();
    if s.error == e {
        s.error = error::NONE;
        s.error_code = 0;
    }
}

/// The error and its code.
pub fn error() -> (c_int, u32) {
    let s = state();
    (s.error, s.error_code)
}

/// Reads and logs the reset reason (once at boot); the `esp_reset_reason_t`.
pub fn read_reset_reason() -> c_int {
    // SAFETY: esp_reset_reason takes no arguments and only reads the stored reason
    let reason = unsafe { sys::esp_reset_reason() } as c_int;
    let text = reset_reason_text(reason);
    state().reset_reason = text;
    log!(Info, TAG, "Reset reason: {}", text.to_string_lossy());
    reason
}

pub fn reset_reason() -> &'static CStr {
    state().reset_reason
}

/// The hashrate shown (GH/s): the ASICs' counters where the board has
/// them, else the 10 min average of the shares.
pub fn current_hashrate() -> f32 {
    if sys::bapi_has_hashrate_counter() {
        crate::mining::hashrate::api::hashrate_monitor_smoothed()
    } else {
        crate::mining::history::api::history_current(1) as f32
    }
}

/// The preferred interface's address ("0.0.0.0" without one) and whether
/// it is Ethernet.
fn preferred_ip() -> (bool, CString, bool) {
    match crate::net::network::preferred_ip() {
        Some((ip, eth)) if !ip.is_empty() => (true, CString::new(ip).unwrap_or_default(), eth),
        p => (p.is_some(), c"0.0.0.0".into(), p.is_some_and(|(_, e)| e)),
    }
}

/// The two temperature filters of the history samples.
#[derive(Default)]
struct Filters {
    vreg: f32,
    asic: f32,
}

impl Filters {
    /// Slight smoothing; each filter (re)starts from the value while it is
    /// 0 (no reading yet) or not finite.
    fn smooth(filtered: &mut f32, value: f32) {
        const ALPHA: f32 = 0.5;
        if *filtered == 0.0 || !filtered.is_finite() {
            *filtered = value;
        } else {
            *filtered = value * ALPHA + (1.0 - ALPHA) * *filtered;
        }
    }

    fn push(&mut self, vreg: f32, asic: f32) -> (f32, f32) {
        Self::smooth(&mut self.vreg, vreg);
        Self::smooth(&mut self.asic, asic);
        (self.vreg, self.asic)
    }
}

/// The error overlay as shown.
#[derive(Default)]
struct Overlay {
    shown: bool,
    code: u32,
}

impl Overlay {
    fn show(&mut self, text: &CStr, code: u32) {
        // an error is already shown
        if self.shown && self.code != 0 {
            return;
        }
        crate::display::show_error(text, code);
        self.shown = true;
        self.code = code;
    }

    /// Only hides an overlay that is shown: hiding every tick reset the
    /// display's shared overlay state, so the found block overlay could not
    /// be dismissed.
    fn hide(&mut self) {
        if !self.shown {
            return;
        }
        crate::display::hide_error();
        self.shown = false;
        self.code = 0;
    }
}

/// The system task; never returns.
pub fn run() -> ! {
    // ticks every 5 s from now; a tick missed while the loop body ran is
    // dropped, as the FreeRTOS timer's signal was
    let start = crate::util::cutil::now_us();
    let mut next = start + PERIOD_US;
    log!(Info, TAG, "SYSTEM_task started");

    crate::display::wait_for_splashs();
    let can_slave = sys::bapi_is_can_slave();

    if !can_slave {
        loop {
            let (ok, ip, _) = preferred_ip();
            set_ip(&ip);
            if ok {
                break;
            }
            // not connected yet: show the access point
            let ssid = CString::new(crate::net::network::ap_ssid()).unwrap_or_default();
            crate::display::portal_screen(&ssid);
            std::thread::sleep(Duration::from_secs(5));
        }
        crate::display::update_ip(&ip());
    }

    crate::display::mining_screen();
    if can_slave {
        crate::display::set_can_icon();
    }

    let mut last_label = CString::default();
    let mut last_found = 0u32;
    let mut toggle = 1;
    let mut filters = Filters::default();
    let mut overlay = Overlay::default();

    loop {
        let now = crate::util::cutil::now_us();
        while next <= now {
            next += PERIOD_US;
        }
        std::thread::sleep(Duration::from_micros((next - now) as u64));

        if crate::power::mgmt::is_shutdown() {
            log!(Warn, TAG, "suspended");
            crate::idf::task::suspend_self();
        }
        crate::stack_monitor::tick(crate::util::cutil::now_us());
        crate::services::heartbeat::tick(crate::util::cutil::now_us());

        if can_slave {
            // the CAN ID in the IP label, every tick (the negotiation may
            // just have finished)
            let id = crate::can::slave::api::can_slave_id();
            let label = if id == crate::can::slave::UNASSIGNED {
                c"CAN: ...".into()
            } else {
                CString::new(format!("CAN ID: {id}")).unwrap_or_default()
            };
            if label != last_label {
                crate::display::update_ip(&label);
                last_label = label;
            }
        } else {
            let (has_ip, ip, eth) = preferred_ip();
            set_ip(&ip);
            if has_ip {
                if ip != last_label {
                    log!(Info, TAG, "ip address: {}", ip.to_string_lossy());
                    crate::display::update_ip(&ip);
                    crate::display::set_network_icon(eth);
                }
                last_label = ip;
            }

            let found = crate::pool::manager::get().map_or(0, |m| m.found_blocks());
            if found > last_found {
                crate::display::show_found_block();
            }
            last_found = found;
        }

        let (e, code) = error();
        if e != error::NONE {
            overlay.show(error_text(e), code);
        } else {
            overlay.hide();
        }

        toggle = 1 - toggle;
        crate::display::update(toggle);

        // a history sample with the filtered temperatures
        let timestamp_ms = (crate::util::cutil::now_us() / 1000) as u64;
        let (vreg, asic) = filters.push(crate::power::mgmt::get(4), crate::power::mgmt::get(3));
        crate::mining::history::api::history_push(
            crate::mining::hashrate::api::hashrate_monitor_total(),
            vreg,
            asic,
            timestamp_ms,
        );
    }
}

pub(crate) mod api {
    use super::*;

    /// The system task entry; never returns.
    pub unsafe extern "C" fn system_task(_: *mut core::ffi::c_void) {
        run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_levels() {
        // the tests share the state: one test for all error transitions
        set_error(error::NONE, 0);
        set_error_if_none(error::COINBASE_VERIFY_FAULT, 0);
        assert_eq!(error(), (error::COINBASE_VERIFY_FAULT, 0));
        // a hardware fault replaces it; clearing the coinbase error keeps it
        set_error(error::PSU_FAULT, 0x80);
        clear_error(error::COINBASE_VERIFY_FAULT);
        assert_eq!(error(), (error::PSU_FAULT, 0x80));
        set_error_if_none(error::COINBASE_VERIFY_FAULT, 0);
        assert_eq!(error().0, error::PSU_FAULT);
        clear_error(error::PSU_FAULT);
        assert_eq!(error(), (error::NONE, 0));
    }

    #[test]
    fn strings_are_cut_like_the_buffers() {
        set_wifi_status(b"Retrying: 65535 and more");
        assert_eq!(wifi_status().as_bytes(), b"Retrying: 65535 and");
        set_ssid(&[b'x'; 40]);
        assert_eq!(ssid().as_bytes().len(), 32);
        set_ssid(b"home\0junk");
        assert_eq!(ssid().as_bytes(), b"home");
    }

    #[test]
    fn texts() {
        assert_eq!(error_text(error::VOUT_FAULT), c"VOLTAGE PROTECTION");
        assert_eq!(error_text(42), c"INVALID ERROR");
        assert_eq!(reset_reason_text(4), c"SYSTEM.RESET_PANIC");
        assert_eq!(reset_reason_text(11), c"SYSTEM.RESET_NOT_SPECIFIED");
    }

    #[test]
    fn filters_start_from_the_first_value() {
        let mut f = Filters::default();
        assert_eq!(f.push(50.0, 60.0), (50.0, 60.0));
        assert_eq!(f.push(60.0, 60.0), (55.0, 60.0));
        f.vreg = f32::NAN;
        assert_eq!(f.push(40.0, 70.0), (40.0, 65.0));
    }
}
