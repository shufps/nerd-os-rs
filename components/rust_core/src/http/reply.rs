//! Response helpers shared by the handlers of `v1/` and `v2/`: JSON
//! documents, plain text, errors, config reads with their defaults, and the
//! settings bodies (`applySettingsBody()`).

use core::ffi::CStr;

use crate::ajson::Json;
use crate::config;
use crate::http::server::read_body;
use crate::http::settings::Write;
use crate::idf::httpd::{err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::settings_apply::{self, Applied};

pub(crate) fn send_err(req: &mut Request, code: crate::idf::httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

pub(crate) fn send_str(req: &mut Request, s: &CStr) -> EspErr {
    req.send(s.to_bytes())
}

/// The end of a chunked (empty) response.
pub(crate) fn end_chunks(req: &mut Request) -> EspErr {
    req.end_chunks()
}

pub(crate) fn set_json(req: &mut Request) {
    req.set_type(c"application/json");
}

/// A document in 2048 byte chunks (`sendJsonBuffer()`).
pub(crate) fn send_doc(req: &mut Request, doc: Json) -> EspErr {
    crate::http::server::send_json(req, &doc.serialize())
}

/// A config string with its default (`cfgGetStrAlloc()`), NUL-terminated.
pub(crate) fn cfg_str(key: &[u8], def: &str) -> std::ffi::CString {
    let v = config::get_str(key)
        .map(|v| crate::cutil::until_nul(&v).to_vec())
        .unwrap_or_else(|| def.as_bytes().to_vec());
    std::ffi::CString::new(v).unwrap_or_default()
}

pub(crate) fn cfg_flag(key: &[u8], def: i64) -> bool {
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
    if !settings_apply::apply_body(&body, parse, applied, &mut settings_apply::Config) {
        log!(Error, tag, "JSON parsing failed");
        send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
        return ESP_FAIL;
    }
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
            |d| crate::http::v1::settings::settings_v1(d, dual_pool),
            &mut a,
        )
    } else {
        apply_settings(req, tag, crate::http::v2::settings::settings_v2, &mut a)
    };
    if r == ESP_OK {
        Ok(a.verify_changed)
    } else {
        Err(r)
    }
}
