//! HTTP handlers (port of `main/http_server/handler_restart.cpp`,
//! `handler_shutdown.cpp`, `handler_influx.cpp`, `handler_alert.cpp`,
//! `handler_otp.cpp`, `v2/handler_v2_identify.cpp` and
//! `v2/handler_v2_system.cpp`) and the settings bodies of the C++ ones
//! (`apply_pool_settings()`, formerly `settings_apply.cpp` behind
//! `applySettingsBody()`). The routes and their checks are in
//! `http_server.rs`; the documents are built by `http_json.rs`.

use crate::idf::idf_const;
use core::ffi::{c_int, CStr};

use crate::ajson::Json;
use crate::config;
use crate::http_json;
use crate::http_routes::Handler;
use crate::http_server::read_body;
use crate::http_settings::{self, Write};
use crate::idf::httpd::{err, HandlerFn, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::sdkconfig as k;
use crate::settings_apply::{self, Applied};

idf_const!(
    /// `ESP_ERR_NOT_FINISHED`: an enrollment is running already
    ESP_ERR_NOT_FINISHED: c_int = ESP_ERR_NOT_FINISHED, 0x10c
);
idf_const!(CAP_SPIRAM: u32 = MALLOC_CAP_SPIRAM, 1 << 10);
idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);
/// the OTP session lifetime
const SESSION_TTL_S: u32 = 24 * 3600;

/// The Rust handler of `h`, if it is one.
pub fn handler(h: Handler) -> Option<HandlerFn> {
    Some(match h {
        Handler::Restart => restart,
        Handler::Shutdown => shutdown,
        Handler::V2Identify => v2_identify,
        Handler::V2System => v2_system,
        Handler::InfluxGet => influx_get,
        Handler::InfluxPatch => influx_patch,
        Handler::AlertGet => alert_get,
        Handler::AlertPatch => alert_patch,
        Handler::AlertTest => alert_test,
        Handler::OtpCreate => otp_create,
        Handler::OtpPatch => otp_patch,
        Handler::OtpStatus => otp_status,
        Handler::OtpSession => otp_session,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// helpers

fn send_err(req: &mut Request, code: crate::idf::httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

fn send_str(req: &mut Request, s: &CStr) -> EspErr {
    req.send(s.to_bytes())
}

/// The end of a chunked (empty) response.
fn end_chunks(req: &mut Request) -> EspErr {
    req.end_chunks()
}

fn set_json(req: &mut Request) {
    req.set_type(c"application/json");
}

/// A document in 2048 byte chunks (`sendJsonBuffer()`).
fn send_doc(req: &mut Request, doc: Json) -> EspErr {
    crate::http_server::send_json(req, &doc.serialize())
}

/// A config string with its default (`cfgGetStrAlloc()`), NUL-terminated.
fn cfg_str(key: &[u8], def: &str) -> std::ffi::CString {
    let v = config::get_str(key)
        .map(|v| crate::cutil::until_nul(&v).to_vec())
        .unwrap_or_else(|| def.as_bytes().to_vec());
    std::ffi::CString::new(v).unwrap_or_default()
}

fn cfg_flag(key: &[u8], def: i64) -> bool {
    config::get_u16(key, def as u16) != 0
}

/// Reads the body and applies its settings; 400 for invalid JSON (logged
/// with `tag`).
pub(crate) fn apply_settings(
    req: &mut Request,
    tag: &CStr,
    parse: impl Fn(&Json) -> Vec<Write>,
    applied: &mut Applied,
) -> EspErr {
    let body = match read_body(req) {
        Ok(b) => b,
        Err(e) => return e,
    };
    if !settings_apply::apply_body(body, parse, applied, &mut settings_apply::Config) {
        log!(Error, tag, "JSON parsing failed");
        send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
        return ESP_FAIL;
    }
    ESP_OK
}

// ---------------------------------------------------------------------------
// restart, shutdown (no OTP on purpose: home automation)

fn restart(req: &mut Request) -> EspErr {
    log!(
        Info,
        c"http_restart",
        "Restarting System because of API Request"
    );
    send_str(req, c"System will restart shortly.");
    // let the response go out
    crate::idf::task::delay_ms(1000);
    crate::power_mgmt::restart()
}

fn shutdown(req: &mut Request) -> EspErr {
    log!(
        Info,
        c"http_shutdown",
        "Shutting down System because of API Request"
    );
    send_str(req, c"System will shutdown shortly.");
    crate::idf::task::delay_ms(1000);
    crate::power_mgmt::shutdown();
    ESP_OK
}

// ---------------------------------------------------------------------------
// v2 identify and system

fn board_info() -> crate::board::ffi::Info {
    let mut i = core::mem::MaybeUninit::uninit();
    // SAFETY: board_info() writes a complete plain-data Info to the pointer,
    // so it is initialized afterwards
    unsafe {
        crate::board::ffi::board_info(i.as_mut_ptr());
        i.assume_init()
    }
}

fn v2_identify(req: &mut Request) -> EspErr {
    set_json(req);
    let b = board_info();
    let v = http_json::V2Identify {
        device_model: b.device_model,
        default_theme: b.default_theme,
        otp: cfg_flag(b"otp_enabled", 0),
        ap_active: crate::netmgr::ap_active(),
        core_dump_download: k::ESP_COREDUMP_ENABLE_TO_FLASH,
        can_enabled: cfg_flag(b"can_enabled", 0),
    };
    // SAFETY: static profile strings
    send_doc(req, unsafe { http_json::v2_identify(&v) })
}

fn v2_system(req: &mut Request) -> EspErr {
    set_json(req);
    let b = board_info();
    let hostname = cfg_str(b"hostname", k::LWIP_LOCAL_HOSTNAME);
    let ssid = cfg_str(b"wifissid", k::ESP_WIFI_SSID);
    let version = std::ffi::CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let ip = crate::system::ip();
    let wifi_status = crate::system::wifi_status();
    let v = http_json::V2System {
        device_model: b.device_model,
        asic_model: b.asic_model,
        version: version.as_ptr(),
        uptime_seconds: (crate::cutil::now_us() - crate::system::start_us()) / 1_000_000,
        last_reset_reason: crate::system::reset_reason().as_ptr(),
        hostname: hostname.as_ptr(),
        ssid: ssid.as_ptr(),
        mac_addr: crate::network::mac_addr().as_ptr(),
        ip_addr: ip.as_ptr(),
        wifi_status: wifi_status.as_ptr(),
        wifi_rssi: crate::network::wifi_rssi(),
        free_heap: crate::idf::task::free_heap(CAP_SPIRAM) as u64,
        free_heap_int: crate::idf::task::free_heap(CAP_INTERNAL) as u64,
    };
    // SAFETY: the strings are this function's live CStrings, the MAC string and
    // static profile literals
    send_doc(req, unsafe { http_json::v2_system(&v) })
}

// ---------------------------------------------------------------------------
// influx

fn influx_get(req: &mut Request) -> EspErr {
    set_json(req);
    let url = cfg_str(b"influx_url", k::INFLUX_URL);
    let bucket = cfg_str(b"influx_bucket", k::INFLUX_BUCKET);
    let org = cfg_str(b"influx_org", k::INFLUX_ORG);
    let prefix = cfg_str(b"influx_prefix", k::INFLUX_PREFIX);
    let i = http_json::InfluxInfo {
        url: url.as_ptr(),
        port: config::get_u16(b"influx_port", k::INFLUX_PORT as u16),
        bucket: bucket.as_ptr(),
        org: org.as_ptr(),
        prefix: prefix.as_ptr(),
        enabled: cfg_flag(b"influx_enable", k::INFLUX_ENABLE_VALUE),
    };
    // SAFETY: the strings are this function's live CStrings
    send_doc(req, unsafe { http_json::influx_info(&i) })
}

fn influx_patch(req: &mut Request) -> EspErr {
    let mut a = Applied::default();
    if apply_settings(req, c"http_utils", http_settings::settings_influx, &mut a) != ESP_OK {
        return ESP_FAIL;
    }
    crate::config_nvs::request_flush();
    end_chunks(req);
    ESP_OK
}

// ---------------------------------------------------------------------------
// alerts

fn alert_get(req: &mut Request) -> EspErr {
    set_json(req);
    let webhook = cfg_str(b"alrt_disc_url", k::ALERT_DISCORD_URL);
    let a = http_json::AlertInfo {
        has_webhook: !webhook.is_empty(),
        watchdog: cfg_flag(b"alrt_disc_en", k::ALERT_DISCORD_WATCHDOG_ENABLE_VALUE),
        block_found: cfg_flag(
            b"alrt_disc_bf_en",
            k::ALERT_DISCORD_BLOCK_FOUND_ENABLE_VALUE,
        ),
        best_diff: cfg_flag(b"alrt_disc_bd_en", k::ALERT_DISCORD_BEST_DIFF_ENABLE_VALUE),
        coinbase_verify: cfg_flag(b"alrt_disc_cb_en", 0),
        show_block_found_screen: cfg_flag(b"block_found_en", k::SHOW_BLOCK_FOUND_ENABLE_VALUE),
    };
    send_doc(req, http_json::alert_info(&a))
}

fn alert_patch(req: &mut Request) -> EspErr {
    let mut a = Applied::default();
    if apply_settings(req, c"http_utils", http_settings::settings_alert, &mut a) != ESP_OK {
        return ESP_FAIL;
    }
    crate::config_nvs::request_flush();
    end_chunks(req);
    // the alerter's and the display's settings
    crate::discord::load_config();
    crate::display::load_settings();
    ESP_OK
}

fn alert_test(req: &mut Request) -> EspErr {
    if crate::discord::ffi::discord_send_test() {
        send_str(req, c"ok");
    } else {
        send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Send failed");
    }
    ESP_OK
}

// ---------------------------------------------------------------------------
// OTP

fn otp_create(req: &mut Request) -> EspErr {
    use crate::otp::ffi::otp_is_enabled;
    if otp_is_enabled() {
        return send_err(req, err::E405_METHOD_NOT_ALLOWED, c"OTP is enabled");
    }
    // an enrollment already started: no new QR, no error
    match crate::otp_device::ffi::otp_dev_start_enrollment() {
        ESP_ERR_NOT_FINISHED => {
            end_chunks(req);
            return ESP_OK;
        }
        ESP_OK => {}
        _ => {
            log!(Error, c"http_otp", "error starting otp enrollment");
            return send_err(
                req,
                err::E500_INTERNAL_SERVER_ERROR,
                c"otp enrollment failed",
            );
        }
    }
    // the QR on the display
    crate::display::send_show_qr();
    end_chunks(req);
    ESP_OK
}

fn otp_patch(req: &mut Request) -> EspErr {
    use crate::otp::ffi::{otp_disable_enrollment, otp_save, otp_set_enabled};
    // stops an enrollment (if one runs), hides the QR in any case
    otp_disable_enrollment();
    crate::display::send_hide_qr();
    let body = match read_body(req) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let enabled = match http_settings::otp_body(body) {
        http_settings::OtpBody::Enabled(b) => b,
        http_settings::OtpBody::Invalid => {
            log!(Error, c"http_otp", "JSON parsing failed");
            send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
            return ESP_FAIL;
        }
        http_settings::OtpBody::NoEnabled => {
            log!(Error, c"http_otp", "invalid enabled state");
            send_err(
                req,
                err::E500_INTERNAL_SERVER_ERROR,
                c"invalid enabled state",
            );
            return ESP_FAIL;
        }
    };
    // the flag and the secret: this finishes the enrollment
    otp_set_enabled(enabled);
    otp_save();
    end_chunks(req);
    ESP_OK
}

fn otp_status(req: &mut Request) -> EspErr {
    send_doc(
        req,
        http_json::otp_status(crate::otp::ffi::otp_is_enabled()),
    )
}

fn otp_session(req: &mut Request) -> EspErr {
    if !crate::otp::ffi::otp_is_enabled() {
        return send_err(req, err::E405_METHOD_NOT_ALLOWED, c"OTP disabled");
    }
    const TOTP_CAP: usize = 16;
    let len = req.header_len(c"X-TOTP");
    let totp = if len == 0 || len + 1 > TOTP_CAP {
        None
    } else {
        req.header(c"X-TOTP", TOTP_CAP)
    };
    let Some(totp) = totp else {
        return send_err(req, err::E400_BAD_REQUEST, c"Missing X-TOTP header");
    };
    // behind the rate limit
    let now = crate::clock::now() as i64;
    let synced = crate::clock::is_time_synced();
    match crate::otp::login(totp.to_bytes(), now, synced, crate::clock::now_ms()) {
        crate::otp::Auth::Ok => {}
        crate::otp::Auth::Blocked => {
            return send_err(req, err::E401_UNAUTHORIZED, c"blocked for 5 minutes")
        }
        crate::otp::Auth::Failed => return send_err(req, err::E401_UNAUTHORIZED, c"Invalid TOTP"),
    }
    // (the former 96 byte buffer)
    let Some(token) = crate::otp::mint(SESSION_TTL_S, now).filter(|t| t.len() < 96) else {
        log!(Error, c"http_otp", "createSessionToken failed");
        return send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Session error");
    };
    let expires_ms = crate::clock::now_ms() + SESSION_TTL_S as u64 * 1000;
    let doc = http_json::otp_session(token.as_bytes(), SESSION_TTL_S, expires_ms).serialize();
    set_json(req);
    req.send(&doc);
    ESP_OK
}

/// The settings body of `PATCH /api/system` (`v1`, with `dual_pool`) and
/// `PATCH /api/v2/settings`: the pools whose coinbase verification
/// settings changed, or the error (400 for invalid JSON is sent).
pub(crate) fn apply_pool_settings(
    req: &mut Request,
    v1: bool,
    dual_pool: bool,
    tag: &CStr,
) -> Result<[bool; 2], EspErr> {
    let mut a = Applied::default();
    let r = if v1 {
        apply_settings(
            req,
            tag,
            |d| http_settings::settings_v1(d, dual_pool),
            &mut a,
        )
    } else {
        apply_settings(req, tag, http_settings::settings_v2, &mut a)
    };
    if r == ESP_OK {
        Ok(a.verify_changed)
    } else {
        Err(r)
    }
}

// ---------------------------------------------------------------------------
// WiFi scan (port of handler_wifi_scan.cpp)

/// `GET /api/v2/wifi/scan`: only while the access point is up (the
/// station is disconnected for the scan); the CORS headers after that
/// check.
pub fn wifi_scan(req: &mut Request) -> EspErr {
    if !crate::netmgr::ap_active() {
        return send_err(
            req,
            err::E403_FORBIDDEN,
            c"WiFi scan only available while AP is active",
        );
    }
    set_json(req);
    if !crate::http_server::set_cors_headers(req) {
        // httpd_resp_send_500()
        req.send_err(err::E500_INTERNAL_SERVER_ERROR, None);
        return ESP_FAIL;
    }
    let nets = match crate::network::scan() {
        Ok(n) => n,
        Err(e) => {
            log!(
                Error,
                c"http_wifi_scan",
                "wifi_scan failed: {}",
                crate::idf::nvs::err_name(e)
            );
            return send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"WiFi scan failed");
        }
    };
    let list: Vec<http_json::WifiNetwork> = nets
        .iter()
        .map(|n| http_json::WifiNetwork {
            ssid: n.ssid.as_ptr(),
            rssi: n.rssi,
            authmode: n.authmode,
        })
        .collect();
    // SAFETY: the SSIDs point into `nets`, alive for the call
    send_doc(req, unsafe { http_json::wifi_scan(&list) })
}
