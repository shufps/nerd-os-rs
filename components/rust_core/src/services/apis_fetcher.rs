//! The mempool API fetcher (port of `main/tasks/apis_task.cpp`): Bitcoin
//! price, block height, network hashrate/difficulty and fees for the
//! display, from the configured mempool instance (default
//! `CONFIG_MEMPOOL_URL`). One fetch at start; while the display shows the
//! page (fetching enabled), every 60 s. The responses are parsed by
//! `services/apis.rs`.
//!
//! Behaviour notes: chunked response bodies are read too (custom mempool
//! instances behind a proxy that chunks); enabling while a fetch runs is not
//! lost (the wakeup has a condition); the values are read and written under
//! a lock.

use core::ffi::{c_int, c_void, CStr};
use std::sync::{Condvar, Mutex};

use crate::config;
use crate::config::sdkconfig;
use crate::idf::http_client::{Client, Event, EVENT_DISCONNECTED, EVENT_ERROR, EVENT_ON_DATA};
use crate::idf::{nvs::err_name, ESP_OK};
use crate::log::log;
use crate::services::apis::{parse, Api, Values};

const TAG: &CStr = c"APIsFetcher";
const BUFFER_SIZE: usize = 1024;
const HALVING_BLOCKS: u32 = 210_000;
const PERIOD_MS: u64 = 60_000;

const APIS: [(&str, Api); 4] = [
    ("/api/v1/prices", Api::Price),
    ("/api/blocks/tip/height", Api::BlockHeight),
    ("/api/v1/mining/hashrate/3d", Api::Hashrate),
    ("/api/v1/fees/recommended", Api::Fees),
];

static VALUES: Mutex<Values> = Mutex::new(Values {
    price_usd: 0,
    block_height: 0,
    net_hash: 0,
    net_difficulty: 0,
    hour_fee: 0,
    half_hour_fee: 0,
    fastest_fee: 0,
});

struct Enable {
    on: Mutex<bool>,
    wake: Condvar,
}

static ENABLE: Enable = Enable {
    on: Mutex::new(false),
    wake: Condvar::new(),
};

/// Starts fetching (wakes the task right away).
pub fn enable_fetching() {
    *ENABLE.on.lock().unwrap_or_else(|e| e.into_inner()) = true;
    ENABLE.wake.notify_one();
}

/// The latest values.
pub fn values() -> Values {
    *VALUES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Blocks to the next halving (0 without a block height).
pub fn blocks_to_halving(height: u32) -> u32 {
    if height == 0 {
        return 0;
    }
    ((height / HALVING_BLOCKS) + 1) * HALVING_BLOCKS - height
}

/// The progress to the next halving in percent.
pub fn halving_percent(height: u32) -> u32 {
    (HALVING_BLOCKS - blocks_to_halving(height)) * 100 / HALVING_BLOCKS
}

/// The base URL with `path` (trailing slashes of the base dropped).
pub fn api_url(base: &str, path: &str) -> String {
    format!("{}{path}", base.trim_end_matches('/'))
}

/// The response body (at most BUFFER_SIZE - 1 bytes, as before).
struct Response {
    buf: Vec<u8>,
}

unsafe extern "C" fn on_event(evt: *mut Event) -> c_int {
    let evt = &*evt;
    match evt.event_id {
        EVENT_ERROR | EVENT_DISCONNECTED => {
            let errnum = Client::errno_of(evt.client);
            if errnum != 0 {
                let transport = if Client::is_ssl_of(evt.client) {
                    "SSL"
                } else {
                    "TCP"
                };
                log!(
                    Error,
                    TAG,
                    "HTTP connection error/disconnected, errno={errnum}, transport={transport}"
                );
            }
        }
        EVENT_ON_DATA if evt.data_len > 0 && !evt.user_data.is_null() => {
            let r = &mut *(evt.user_data as *mut Response);
            let room = (BUFFER_SIZE - 1).saturating_sub(r.buf.len());
            let n = (evt.data_len as usize).min(room);
            r.buf
                .extend_from_slice(core::slice::from_raw_parts(evt.data as *const u8, n));
        }
        _ => {}
    }
    ESP_OK
}

/// One API: request, parse, store; false on an error.
fn fetch(url: &str, api: Api) -> bool {
    let Ok(curl) = std::ffi::CString::new(url) else {
        return false;
    };
    let mut resp = Response {
        buf: Vec::with_capacity(BUFFER_SIZE),
    };
    let Some(client) =
        Client::with_handler(&curl, on_event, &mut resp as *mut Response as *mut c_void)
    else {
        log!(Error, TAG, "Failed to initialize HTTP client.");
        return false;
    };
    let err = client.perform();
    if err != ESP_OK {
        log!(Error, TAG, "HTTP GET request failed: {}", err_name(err));
        let transport = if client.is_ssl() { "SSL" } else { "TCP" };
        log!(
            Error,
            TAG,
            "HTTP request error details: errno={}, transport={transport}",
            client.errno()
        );
        return false;
    }
    log!(
        Info,
        TAG,
        "HTTP Status = {}, Content-Length = {}",
        client.status(),
        client.content_length()
    );
    drop(client);
    if resp.buf.is_empty() {
        log!(Error, TAG, "Empty response received!");
        return false;
    }
    log!(
        Info,
        TAG,
        "Received JSON: {}",
        String::from_utf8_lossy(&resp.buf)
    );
    let mut v = values();
    if !parse(api, &resp.buf, &mut v) {
        log!(Error, TAG, "JSON parsing failed!");
        return false;
    }
    *VALUES.lock().unwrap_or_else(|e| e.into_inner()) = v;
    match api {
        Api::Price => log!(Info, TAG, "Bitcoin price in USD: {}", v.price_usd),
        Api::BlockHeight => log!(Info, TAG, "Current block: {}", v.block_height),
        Api::Hashrate => {
            log!(Info, TAG, "Network hash: {} EH/s", v.net_hash);
            log!(Info, TAG, "Network difficulty: {} T", v.net_difficulty);
        }
        Api::Fees => log!(
            Info,
            TAG,
            "Network fees: {}, {}, {}",
            v.hour_fee,
            v.half_hour_fee,
            v.fastest_fee
        ),
    }
    true
}

/// The configured base URL (empty: not configured).
fn base_url() -> String {
    if config::get_u16(b"mempool_cust", 0) != 0 {
        config::get_str(b"mempool_url")
            .map(|v| String::from_utf8_lossy(crate::util::cutil::until_nul(&v)).into_owned())
            .unwrap_or_else(|| sdkconfig::MEMPOOL_URL.to_string())
    } else {
        sdkconfig::MEMPOOL_URL.to_string()
    }
}

fn fetch_all() {
    let base = base_url();
    if base.is_empty() {
        log!(Info, TAG, "Mempool URL not configured, skipping fetch");
        *VALUES.lock().unwrap_or_else(|e| e.into_inner()) = Values::default();
        return;
    }
    for (path, api) in APIS {
        fetch(&api_url(&base, path), api);
    }
}

fn enabled() -> bool {
    *ENABLE.on.lock().unwrap_or_else(|e| e.into_inner())
}

/// The task: one fetch, then every 60 s while enabled.
pub fn run() -> ! {
    log!(Info, TAG, "APIs Fetcher started...");
    fetch_all();
    loop {
        {
            let mut on = ENABLE.on.lock().unwrap_or_else(|e| e.into_inner());
            while !*on {
                on = ENABLE.wake.wait(on).unwrap_or_else(|e| e.into_inner());
            }
        }
        loop {
            fetch_all();
            std::thread::sleep(std::time::Duration::from_millis(PERIOD_MS));
            if !enabled() {
                break;
            }
        }
    }
}

pub(crate) mod api {
    use super::*;

    /// The task entry (`xTaskCreatePSRAM`).
    pub unsafe extern "C" fn apis_task(_: *mut c_void) {
        run()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halving() {
        assert_eq!(blocks_to_halving(0), 0);
        assert_eq!(halving_percent(0), 100);
        assert_eq!(blocks_to_halving(840_000), 210_000);
        assert_eq!(blocks_to_halving(924_123), 1_050_000 - 924_123);
        assert_eq!(
            halving_percent(924_123),
            (210_000 - (1_050_000 - 924_123)) * 100 / 210_000
        );
    }

    #[test]
    fn urls() {
        assert_eq!(
            api_url("https://mempool.space", "/api/x"),
            "https://mempool.space/api/x"
        );
        assert_eq!(
            api_url("http://m.local:8080//", "/api/x"),
            "http://m.local:8080/api/x"
        );
    }
}
