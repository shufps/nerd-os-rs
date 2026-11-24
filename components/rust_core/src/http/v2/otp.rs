//! The OTP endpoints under `/api/v2/otp` (port of `handler_otp.cpp`):
//! enrollment, enable/disable, status and the session login.

use crate::idf::idf_const;
use core::ffi::c_int;

use crate::ajson::Json;
use crate::http::json::Obj;
use crate::http::reply::{end_chunks, send_doc, send_err, set_json};
use crate::http::server::read_body;
use crate::idf::httpd::{err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;

/// `GET /api/otp/status`
pub fn otp_status(enabled: bool) -> Json {
    let mut d = Obj::new();
    d.bool("enabled", enabled);
    d.into()
}

/// The OTP session response: the token, its lifetime and expiry.
pub fn otp_session(token: &[u8], ttl_seconds: u32, expires_ms: u64) -> Json {
    let mut d = Obj::new();
    d.set("token", Json::Str(token.to_vec()))
        .uint("ttlMs", ttl_seconds.wrapping_mul(1000) as u64)
        .double("expiresAt", expires_ms as f64);
    d.into()
}

/// `PATCH /api/v2/otp`: the `enabled` flag.
pub enum OtpBody {
    Invalid,
    NoEnabled,
    Enabled(bool),
}

pub fn otp_body(body: &[u8]) -> OtpBody {
    match crate::ajson::deserialize(body) {
        Err(_) => OtpBody::Invalid,
        Ok(doc) => doc
            .get("enabled")
            .as_bool()
            .map_or(OtpBody::NoEnabled, OtpBody::Enabled),
    }
}

idf_const!(
    /// `ESP_ERR_NOT_FINISHED`: an enrollment is running already
    ESP_ERR_NOT_FINISHED: c_int = ESP_ERR_NOT_FINISHED, 0x10c
);

/// the OTP session lifetime
const SESSION_TTL_S: u32 = 24 * 3600;

pub(crate) fn create(req: &mut Request) -> EspErr {
    use crate::otp::api::otp_is_enabled;
    if otp_is_enabled() {
        return send_err(req, err::E405_METHOD_NOT_ALLOWED, c"OTP is enabled");
    }
    // an enrollment already started: no new QR, no error
    match crate::otp_device::api::otp_dev_start_enrollment() {
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

pub(crate) fn patch(req: &mut Request) -> EspErr {
    use crate::otp::api::{otp_save, otp_set_enabled};
    use crate::otp::disable_enrollment as otp_disable_enrollment;
    // stops an enrollment (if one runs), hides the QR in any case
    otp_disable_enrollment();
    crate::display::send_hide_qr();
    let body = match read_body(req) {
        Ok(b) => b,
        Err(e) => return e,
    };
    let enabled = match otp_body(&body) {
        OtpBody::Enabled(b) => b,
        OtpBody::Invalid => {
            log!(Error, c"http_otp", "JSON parsing failed");
            send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
            return ESP_FAIL;
        }
        OtpBody::NoEnabled => {
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

pub(crate) fn status(req: &mut Request) -> EspErr {
    send_doc(req, otp_status(crate::otp::api::otp_is_enabled()))
}

pub(crate) fn session(req: &mut Request) -> EspErr {
    if !crate::otp::api::otp_is_enabled() {
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
    let doc = otp_session(token.as_bytes(), SESSION_TTL_S, expires_ms).serialize();
    set_json(req);
    req.send(&doc);
    ESP_OK
}
