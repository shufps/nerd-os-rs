//! `GET /api/v2/wifi/scan` (port of `handler_wifi_scan.cpp`).

use core::ffi::c_int;
use std::ffi::CString;

use crate::http::json::Obj;
use crate::http::reply::{send_doc, send_err, set_json};
use crate::idf::httpd::{err, Request};
use crate::idf::{EspErr, ESP_FAIL};
use crate::log::log;
use crate::util::ajson::Json;

/// `wifi_network_t`: one scanned access point.
#[derive(Clone, Debug, Default)]
pub struct WifiNetwork {
    pub ssid: CString,
    pub rssi: i8,
    pub authmode: c_int,
}

/// `GET /api/wifi/scan`
pub fn wifi_scan(nets: &[WifiNetwork]) -> Json {
    let list = nets
        .iter()
        .map(|n| {
            let mut o = Obj::new();
            o.cstr("ssid", Some(&n.ssid))
                .int("rssi", n.rssi as i64)
                .int("authmode", n.authmode as i64);
            o.into()
        })
        .collect();
    let mut d = Obj::new();
    d.arr("networks", list);
    d.into()
}

/// `GET /api/v2/wifi/scan`: only while the access point is up (the
/// station is disconnected for the scan); the CORS headers after that
/// check.
pub fn scan(req: &mut Request) -> EspErr {
    if !crate::net::netmgr::ap_active() {
        return send_err(
            req,
            err::E403_FORBIDDEN,
            c"WiFi scan only available while AP is active",
        );
    }
    set_json(req);
    if !crate::http::server::set_cors_headers(req) {
        // httpd_resp_send_500()
        req.send_err(err::E500_INTERNAL_SERVER_ERROR, None);
        return ESP_FAIL;
    }
    let nets = match crate::net::network::scan() {
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
    let list: Vec<WifiNetwork> = nets
        .into_iter()
        .map(|n| WifiNetwork {
            ssid: n.ssid,
            rssi: n.rssi,
            authmode: n.authmode,
        })
        .collect();
    send_doc(req, wifi_scan(&list))
}
