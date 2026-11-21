//! Wall clock and SNTP (port of `main/sntp.cpp` and `now()`/`now_ms()`/
//! `is_time_synced()` of `main/main.cpp`): SNTP in poll mode with
//! de.pool.ntp.org and pool.ntp.org, a 6 h resync interval, immediate time
//! updates, the time zone Europe/Berlin; the time counts as synchronized
//! after the first update and when it is past 2024-08-06.

use core::ffi::CStr;
#[cfg(not(target_os = "espidf"))]
use core::ffi::{c_char, c_int};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::log::log;

const TAG: &CStr = c"time";

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::timeval as Timeval;
#[cfg(not(target_os = "espidf"))]
/// `struct timeval`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i32,
}

static SYNCED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_sntp_init, esp_sntp_setoperatingmode, esp_sntp_setservername, gettimeofday, setenv,
        sntp_set_sync_interval, sntp_set_sync_mode, sntp_set_time_sync_notification_cb, tzset,
    };
}

/// On the host: the system clock, no SNTP.
#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn gettimeofday(tv: *mut Timeval, _: *mut core::ffi::c_void) -> c_int {
        let d = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        *tv = Timeval {
            tv_sec: d.as_secs() as i64,
            tv_usec: d.subsec_micros() as i32,
        };
        0
    }
    pub unsafe fn esp_sntp_setoperatingmode(_: u32) {}
    pub unsafe fn esp_sntp_setservername(_: u8, _: *const c_char) {}
    pub unsafe fn sntp_set_sync_interval(_: u32) {}
    pub unsafe fn sntp_set_sync_mode(_: u32) {}
    pub unsafe fn sntp_set_time_sync_notification_cb(
        _: Option<unsafe extern "C" fn(*mut Timeval)>,
    ) {
    }
    pub unsafe fn esp_sntp_init() {}
    pub unsafe fn setenv(_: *const c_char, _: *const c_char, _: c_int) -> c_int {
        0
    }
    pub unsafe fn tzset() {}
}

/// Milliseconds since the epoch.
pub fn now_ms() -> u64 {
    let mut tv = Timeval::default();
    // SAFETY: `tv` is a live local; the timezone argument may be null
    unsafe { sys::gettimeofday(&mut tv, core::ptr::null_mut()) };
    tv.tv_sec as u64 * 1000 + tv.tv_usec as u64 / 1000
}

/// Seconds since the epoch.
pub fn now() -> u32 {
    (now_ms() / 1000) as u32
}

/// SNTP has set the clock (and it is past 2024-08-06).
pub fn is_time_synced() -> bool {
    SYNCED.load(Ordering::Acquire) && now() >= 1722927653
}

unsafe extern "C" fn on_sync(_: *mut Timeval) {
    log!(Info, TAG, "Time synchronized");
    SYNCED.store(true, Ordering::Release);
}

/// Starts SNTP.
pub fn start() {
    crate::idf::idf_const!(OPMODE_POLL: u32 = esp_sntp_operatingmode_t_ESP_SNTP_OPMODE_POLL, 0);
    crate::idf::idf_const!(SYNC_MODE_IMMED: u32 = sntp_sync_mode_t_SNTP_SYNC_MODE_IMMED, 0);
    // SAFETY: the server names and TZ strings are static NUL-terminated literals
    // (SNTP keeps the name pointers) and `on_sync` is a valid extern "C" fn
    unsafe {
        sys::esp_sntp_setoperatingmode(OPMODE_POLL);
        // the server names are kept as pointers: static strings
        sys::esp_sntp_setservername(0, c"de.pool.ntp.org".as_ptr());
        sys::esp_sntp_setservername(1, c"pool.ntp.org".as_ptr());
        sys::sntp_set_sync_interval(6 * 60 * 60 * 1000);
        sys::sntp_set_sync_mode(SYNC_MODE_IMMED);
        sys::sntp_set_time_sync_notification_cb(Some(on_sync));
        sys::setenv(c"TZ".as_ptr(), c"CET-1CEST,M3.5.0,M10.5.0/3".as_ptr(), 1);
        sys::tzset();
        sys::esp_sntp_init();
    }
    log!(Info, TAG, "SNTP started");
}
