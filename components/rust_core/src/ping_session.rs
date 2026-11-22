//! One round of ICMP pings with `esp_ping` (port of `ping_perform` in
//! `main/tasks/ping_task.cpp`); the loop and the statistics are in
//! `ping.rs`.

use crate::idf::idf_const;
#[cfg(not(target_os = "espidf"))]
use core::ffi::c_char;
use core::ffi::{c_int, c_void, CStr};
use std::ffi::CString;
use std::sync::Mutex;
use std::time::Duration;

#[cfg(not(target_os = "espidf"))]
use crate::idf::EspErr;
use crate::idf::ESP_OK;
use crate::log::log;
use crate::ping::{Round, COUNT};

const INTERVAL_MS: u32 = 1000;
const TIMEOUT_MS: u32 = 1000;

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::ip_addr_t as IpAddr;
#[cfg(not(target_os = "espidf"))]
/// `ip_addr_t` (dual stack: the union of an IPv6 address with its zone,
/// padded to 20 bytes, then the type); only passed through to lwIP
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct IpAddr {
    u_addr: [u32; 5],
    ty: u8,
}

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_ping_config_t as Config;
#[cfg(not(target_os = "espidf"))]
/// `esp_ping_config_t`
#[repr(C)]
pub struct Config {
    count: u32,
    interval_ms: u32,
    timeout_ms: u32,
    data_size: u32,
    tos: c_int,
    ttl: c_int,
    target_addr: IpAddr,
    task_stack_size: u32,
    task_prio: u32,
    interface: u32,
}

#[cfg(not(target_os = "espidf"))]
type Cb = unsafe extern "C" fn(hdl: *mut c_void, args: *mut c_void);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_ping_callbacks_t as Callbacks;
#[cfg(not(target_os = "espidf"))]
/// `esp_ping_callbacks_t`
#[repr(C)]
pub struct Callbacks {
    cb_args: *mut c_void,
    on_ping_success: Option<Cb>,
    on_ping_timeout: Option<Cb>,
    on_ping_end: Option<Cb>,
}

idf_const!(PROF_SEQNO: u32 = esp_ping_profile_t_ESP_PING_PROF_SEQNO, 0);
idf_const!(PROF_TTL: u32 = esp_ping_profile_t_ESP_PING_PROF_TTL, 2);
idf_const!(PROF_REPLY: u32 = esp_ping_profile_t_ESP_PING_PROF_REPLY, 4);
idf_const!(PROF_IPADDR: u32 = esp_ping_profile_t_ESP_PING_PROF_IPADDR, 5);
idf_const!(PROF_SIZE: u32 = esp_ping_profile_t_ESP_PING_PROF_SIZE, 6);
idf_const!(PROF_TIMEGAP: u32 = esp_ping_profile_t_ESP_PING_PROF_TIMEGAP, 7);
/// The stack of the ping session's task, which runs the reply callbacks
/// (they format and log; with the web socket log on, the vprintf hook runs
/// there too). `ESP_TASK_PING_STACK` (2560 bytes) overflowed after hours:
/// measured peak 2432 bytes, 2528 with the web socket log on.
const PING_STACK: u32 = 4096;
idf_const!(DEFAULT_TTL: c_int = IP_DEFAULT_TTL, 64);

/// The address as text (not lwIP's `ipaddr_ntoa()`: its one static
/// buffer would be shared by both pools' ping callbacks).
#[cfg(target_os = "espidf")]
fn ip_text(a: &IpAddr) -> String {
    idf_const!(IPADDR_TYPE_V4: u32 = lwip_ip_addr_type_IPADDR_TYPE_V4, 0);
    if a.type_ as u32 == IPADDR_TYPE_V4 {
        // SAFETY: the type says the IPv4 member is the active one
        let v = unsafe { a.u_addr.ip4.addr }.to_ne_bytes();
        format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3])
    } else {
        // SAFETY: not IPv4: the IPv6 member is the active one
        let w = unsafe { a.u_addr.ip6.addr };
        let b: Vec<u8> = w.iter().flat_map(|x| x.to_ne_bytes()).collect();
        let b: [u8; 16] = b.try_into().unwrap_or([0; 16]);
        std::net::Ipv6Addr::from(b).to_string()
    }
}

#[cfg(not(target_os = "espidf"))]
fn ip_text(_: &IpAddr) -> String {
    "?".into()
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_ping_delete_session, esp_ping_get_profile, esp_ping_new_session, esp_ping_start,
        esp_ping_stop, ipaddr_aton,
    };
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn ipaddr_aton(_: *const c_char, _: *mut IpAddr) -> c_int {
        0
    }
    pub unsafe fn esp_ping_new_session(
        _: *const Config,
        _: *const Callbacks,
        _: *mut *mut c_void,
    ) -> EspErr {
        crate::idf::ESP_FAIL
    }
    pub unsafe fn esp_ping_delete_session(_: *mut c_void) -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_ping_start(_: *mut c_void) -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_ping_stop(_: *mut c_void) -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_ping_get_profile(_: *mut c_void, _: u32, _: *mut c_void, _: u32) -> EspErr {
        crate::idf::ESP_FAIL
    }
}

/// The replies of a round, filled by the ping thread's callback.
struct Replies {
    replies: u16,
    total_time_ms: u32,
    min_rtt: f64,
    max_rtt: f64,
    hostname: String,
    header_shown: bool,
}

impl Replies {
    const fn new() -> Self {
        Replies {
            replies: 0,
            total_time_ms: 0,
            min_rtt: 1e6,
            max_rtt: 0.0,
            hostname: String::new(),
            header_shown: false,
        }
    }
}

/// Per pool, not on `perform`'s stack: the ping thread can still run a
/// callback after `esp_ping_stop()` / `esp_ping_delete_session()` returned.
static REPLIES: [Mutex<Replies>; 2] = [Mutex::new(Replies::new()), Mutex::new(Replies::new())];

fn tag(pool: usize) -> &'static CStr {
    if pool == 1 {
        c"ping task (sec)"
    } else {
        c"ping task (pri)"
    }
}

/// A `u32` profile value.
unsafe fn profile(hdl: *mut c_void, p: u32) -> Option<u32> {
    let mut v = 0u32;
    (sys::esp_ping_get_profile(hdl, p, &mut v as *mut u32 as *mut c_void, 4) == ESP_OK).then_some(v)
}

/// `on_ping_success`; `args` is the pool index.
unsafe extern "C" fn on_success(hdl: *mut c_void, args: *mut c_void) {
    let pool = args as usize & 1;
    let Some(time_ms) = profile(hdl, PROF_TIMEGAP) else {
        return;
    };
    let seq = profile(hdl, PROF_SEQNO);
    let ttl = profile(hdl, PROF_TTL);
    let size = profile(hdl, PROF_SIZE);
    let mut addr = IpAddr::default();
    let ok_ip = sys::esp_ping_get_profile(
        hdl,
        PROF_IPADDR,
        &mut addr as *mut IpAddr as *mut c_void,
        core::mem::size_of::<IpAddr>() as u32,
    ) == ESP_OK;
    let ip = if ok_ip {
        ip_text(&addr)
    } else {
        "?.?.?.?".into()
    };

    let mut s = REPLIES[pool].lock().unwrap_or_else(|e| e.into_inner());
    let tag = tag(pool);
    if let (false, true, Some(size)) = (s.header_shown, ok_ip, size) {
        log!(Info, tag, "PING {} ({ip}): {size} data bytes", s.hostname);
        s.header_shown = true;
    }
    s.total_time_ms += time_ms;
    s.replies += 1;
    let t = time_ms as f64;
    if t < s.min_rtt {
        s.min_rtt = t;
    }
    if t > s.max_rtt {
        s.max_rtt = t;
    }
    log!(
        Info,
        tag,
        "{} bytes from {ip}: icmp_seq={} ttl={} time={time_ms} ms",
        size.unwrap_or(64),
        seq.unwrap_or(s.replies as u32),
        ttl.unwrap_or(64)
    );
}

fn sleep_ms(ms: u32) {
    std::thread::sleep(Duration::from_millis(ms as u64));
}

/// A round of `COUNT` pings to `ip` (IPv4 or IPv6 text); `host` is for
/// the log.
pub fn perform(pool: usize, ip: &str, host: &str) -> Round {
    let pool = pool & 1;
    let tag = tag(pool);
    let mut result = Round {
        min_rtt_ms: 1e6,
        ..Default::default()
    };

    let mut target = IpAddr::default();
    let Ok(cip) = CString::new(ip) else {
        return result;
    };
    // SAFETY: `cip` is a live NUL-terminated CString and `target` a live local
    if unsafe { sys::ipaddr_aton(cip.as_ptr(), &mut target) } == 0 {
        log!(
            Error,
            tag,
            "Invalid IP string passed to perform_ping ({ip})"
        );
        return result;
    }
    {
        let mut s = REPLIES[pool].lock().unwrap_or_else(|e| e.into_inner());
        *s = Replies::new();
        s.hostname = host.chars().take(63).collect();
    }

    // ESP_PING_DEFAULT_CONFIG() with the round's count and timing
    let config = Config {
        count: COUNT as u32,
        interval_ms: INTERVAL_MS,
        timeout_ms: TIMEOUT_MS,
        data_size: 64,
        tos: 0,
        ttl: DEFAULT_TTL,
        target_addr: target,
        task_stack_size: PING_STACK,
        task_prio: 2,
        interface: 0,
    };
    let cbs = Callbacks {
        cb_args: pool as *mut c_void,
        on_ping_success: Some(on_success),
        on_ping_timeout: None,
        on_ping_end: None,
    };

    let mut hdl: *mut c_void = core::ptr::null_mut();
    // SAFETY: `config`/`cbs` are live locals that esp_ping_new_session copies; `hdl`
    // comes from a successful esp_ping_new_session and is deleted once at the end
    unsafe {
        if sys::esp_ping_new_session(&config, &cbs, &mut hdl) != ESP_OK {
            log!(Error, tag, "Error creating ping session");
            if !hdl.is_null() {
                sys::esp_ping_delete_session(hdl);
            }
            return result;
        }
        if sys::esp_ping_start(hdl) != ESP_OK {
            log!(Error, tag, "Error starting ping session");
            sys::esp_ping_delete_session(hdl);
            return result;
        }

        // until all replies are in or the time is up
        let max_wait_ms = COUNT as u32 * (TIMEOUT_MS + 100);
        let mut wait = 0;
        while wait < max_wait_ms {
            if profile(hdl, PROF_REPLY).unwrap_or(0) >= COUNT as u32 {
                // let the last callback finish
                sleep_ms(50);
                break;
            }
            sleep_ms(100);
            wait += 100;
        }
        sys::esp_ping_stop(hdl);
        // the ping thread sees the stop flag only at the top of its loop;
        // let a receive in flight (up to TIMEOUT_MS) and its callback end
        sleep_ms(TIMEOUT_MS + 200);
        sys::esp_ping_delete_session(hdl);
    }

    let s = REPLIES[pool].lock().unwrap_or_else(|e| e.into_inner());
    if s.replies > 0 {
        result.success = true;
        result.replies = s.replies;
        result.avg_rtt_ms = s.total_time_ms as f64 / s.replies as f64;
        result.min_rtt_ms = s.min_rtt;
        result.max_rtt_ms = s.max_rtt;
    }
    result
}
