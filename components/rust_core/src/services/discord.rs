//! The Discord alerter's setup and transport (port of `main/discord.cpp`):
//! the alert settings with their Kconfig defaults, the device's address,
//! the sender task (8 KB stack in PSRAM, priority 5) and the HTTPS POST
//! (5 s timeout, the certificate bundle). Texts, queue and sender loop are
//! in `services/alerts.rs`.

use core::ffi::{c_int, c_void, CStr};

use crate::config;
use crate::config::sdkconfig;
use crate::idf::http_client::{Client, METHOD_POST};
use crate::idf::{nvs::err_name, task, ESP_OK};
use crate::log::log;
use crate::services::alerts;
use crate::util::suffix::suffix_string;

const TAG: &CStr = c"discord";

/// The settings of `alert_*` (as the former `DiscordAlerter::loadConfig()`).
pub fn load_config() {
    let get_str = |key: &[u8], def: &str| {
        config::get_str(key)
            .map(|v| String::from_utf8_lossy(crate::util::cutil::until_nul(&v)).into_owned())
            .unwrap_or_else(|| def.to_string())
    };
    let on = |key: &[u8], def: i64| config::get_u16(key, def as u16) != 0;
    alerts::set_config(alerts::Config {
        webhook: get_str(b"alrt_disc_url", sdkconfig::ALERT_DISCORD_URL),
        host: get_str(b"hostname", sdkconfig::LWIP_LOCAL_HOSTNAME),
        watchdog: on(
            b"alrt_disc_en",
            sdkconfig::ALERT_DISCORD_WATCHDOG_ENABLE_VALUE,
        ),
        block_found: on(
            b"alrt_disc_bf_en",
            sdkconfig::ALERT_DISCORD_BLOCK_FOUND_ENABLE_VALUE,
        ),
        best_diff: on(
            b"alrt_disc_bd_en",
            sdkconfig::ALERT_DISCORD_BEST_DIFF_ENABLE_VALUE,
        ),
        coinbase_verify: on(b"alrt_disc_cb_en", 0),
        heartbeat_hours: config::get_u16(b"alrt_hb_intv", 0)
            .min(crate::services::heartbeat::MAX_HOURS),
    });
}

/// POSTs `body` to `url`; the HTTP status, -1 on an error.
fn http_post(url: &str, body: &str) -> c_int {
    let Ok(url) = std::ffi::CString::new(url) else {
        return -1;
    };
    let Some(client) = Client::new(&url, METHOD_POST, 5000) else {
        log!(Error, TAG, "Failed to init HTTP client");
        return -1;
    };
    client.set_method(METHOD_POST);
    client.set_header(c"Content-Type", c"application/json");
    let err = client.perform_with_body(body.as_bytes());
    if err != ESP_OK {
        log!(Error, TAG, "HTTP POST failed: {}", err_name(err));
        return -1;
    }
    client.status()
}

unsafe extern "C" fn sender_task(_: *mut c_void) {
    alerts::run(&mut http_post)
}

/// Reads the settings and starts the sender task.
pub fn start() {
    load_config();
    task::spawn_psram(sender_task, c"discord_task", 8192, 0, 5);
    log!(Info, TAG, "Discord task started");
}

/// The device's address ("0.0.0.0" without one) and MAC for a message
/// (only looked up for enabled alerts).
fn device() -> (String, String) {
    let ip = crate::net::network::preferred_ip().map_or_else(|| "0.0.0.0".into(), |(ip, _)| ip);
    let mac = crate::net::network::mac_addr()
        .to_string_lossy()
        .into_owned();
    (ip, mac)
}

/// Queues an alert (`alerts::send`), with the device block when enabled.
fn send(
    kind: c_int,
    diff: &str,
    network_diff: &str,
    pool: c_int,
    user: u64,
    total: u64,
    mode: c_int,
) -> bool {
    let (ip, mac) = if alerts::kind_enabled(kind) {
        device()
    } else {
        (String::new(), String::new())
    };
    alerts::send(kind, diff, network_diff, pool, user, total, mode, &ip, &mac)
}

/// The difficulty as the message shows it (`suffixString`, 3 digits).
fn diff_text(d: f64) -> String {
    suffix_string(d as u64, 0)
}

pub(crate) mod api {
    use super::*;
    use crate::services::alerts::kind;

    pub fn discord_send_test() -> bool {
        send(kind::TEST, "", "", 0, 0, 0, 0)
    }

    pub fn discord_send_watchdog() -> bool {
        send(kind::WATCHDOG, "", "", 0, 0, 0, 0)
    }

    pub fn discord_send_block_found(diff: f64, network_diff: f64) -> bool {
        send(
            kind::BLOCK_FOUND,
            &diff_text(diff),
            &diff_text(network_diff),
            0,
            0,
            0,
            0,
        )
    }

    pub fn discord_send_best_diff(diff: f64, network_diff: f64) -> bool {
        send(
            kind::BEST_DIFF,
            &diff_text(diff),
            &diff_text(network_diff),
            0,
            0,
            0,
            0,
        )
    }

    pub fn discord_send_coinbase_verify_failed(
        pool: c_int,
        user_sats: u64,
        total_sats: u64,
        mode: c_int,
    ) -> bool {
        send(
            kind::COINBASE_VERIFY,
            "",
            "",
            pool,
            user_sats,
            total_sats,
            mode,
        )
    }
}
