//! `GET`/`PATCH /api/v2/influx` (port of `handler_influx.cpp`).

use std::ffi::CString;

use crate::config;
use crate::config::sdkconfig as k;
use crate::config::settings_apply::Applied;
use crate::http::json::Obj;
use crate::http::reply::{apply_settings, cfg_flag, cfg_str, end_chunks, send_doc, set_json};
use crate::http::settings::{boolean, field, string, uint, Out, Setting as S, Write};
use crate::idf::httpd::Request;
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::util::ajson::Json;

/// `influx_info_t`: `GET /api/influx/info`.
#[derive(Clone, Debug, Default)]
pub struct InfluxInfo {
    pub url: Option<CString>,
    pub port: u16,
    pub bucket: Option<CString>,
    pub org: Option<CString>,
    pub prefix: Option<CString>,
    pub enabled: bool,
}

pub fn influx_info(i: &InfluxInfo) -> Json {
    let mut d = Obj::new();
    d.cstr("url", i.url.as_deref())
        .uint("port", i.port as u64)
        .cstr("bucket", i.bucket.as_deref())
        .cstr("org", i.org.as_deref())
        .cstr("prefix", i.prefix.as_deref())
        .int("enabled", i.enabled as i64);
    d.into()
}

/// `PATCH /api/v2/influx`.
pub fn settings_influx(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.bool(S::InfluxEnabled, 0, boolean(f("enabled")));
    out.text(S::InfluxUrl, 0, string(f("url")));
    out.num(S::InfluxPort, 0, uint(f("port"), 0xffff));
    out.text(S::InfluxToken, 0, string(f("token")));
    out.text(S::InfluxBucket, 0, string(f("bucket")));
    out.text(S::InfluxOrg, 0, string(f("org")));
    out.text(S::InfluxPrefix, 0, string(f("prefix")));
    out.0
}

pub(crate) fn get(req: &mut Request) -> EspErr {
    set_json(req);
    let url = cfg_str(b"influx_url", k::INFLUX_URL);
    let bucket = cfg_str(b"influx_bucket", k::INFLUX_BUCKET);
    let org = cfg_str(b"influx_org", k::INFLUX_ORG);
    let prefix = cfg_str(b"influx_prefix", k::INFLUX_PREFIX);
    let i = InfluxInfo {
        url: Some(url),
        port: config::get_u16(b"influx_port", k::INFLUX_PORT as u16),
        bucket: Some(bucket),
        org: Some(org),
        prefix: Some(prefix),
        enabled: cfg_flag(b"influx_enable", k::INFLUX_ENABLE_VALUE),
    };
    send_doc(req, influx_info(&i))
}

pub(crate) fn patch(req: &mut Request) -> EspErr {
    let mut a = Applied::default();
    if apply_settings(req, c"http_utils", settings_influx, &mut a) != ESP_OK {
        return ESP_FAIL;
    }
    crate::config::nvs::request_flush();
    end_chunks(req);
    ESP_OK
}
