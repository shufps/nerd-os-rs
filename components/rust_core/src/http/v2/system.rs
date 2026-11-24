//! `GET /api/v2/system` and `GET /api/v2/identify` (port of
//! `v2/handler_v2_system.cpp` and `v2/handler_v2_identify.cpp`).

use core::ffi::c_int;
use std::ffi::CString;

use crate::config::sdkconfig as k;
use crate::http::json::Obj;
use crate::http::reply::{cfg_flag, cfg_str, send_doc, set_json};
use crate::http::sources::{CAP_INTERNAL, CAP_SPIRAM};
use crate::idf::httpd::Request;
use crate::idf::EspErr;
use crate::util::ajson::Json;

/// `v2_system_t`: the values of `GET /api/v2/system`.
#[derive(Clone, Debug, Default)]
pub struct V2System {
    pub device_model: Option<CString>,
    pub asic_model: Option<CString>,
    pub version: Option<CString>,
    pub uptime_seconds: i64,
    pub last_reset_reason: Option<CString>,
    pub hostname: Option<CString>,
    pub ssid: Option<CString>,
    pub mac_addr: Option<CString>,
    pub ip_addr: Option<CString>,
    pub wifi_status: Option<CString>,
    pub wifi_rssi: c_int,
    pub free_heap: u64,
    pub free_heap_int: u64,
}

/// `GET /api/v2/system`
pub fn v2_system(s: &V2System) -> Json {
    let mut d = Obj::new();
    d.cstr("deviceModel", s.device_model.as_deref())
        .cstr("asicModel", s.asic_model.as_deref())
        .cstr("version", s.version.as_deref())
        .int("uptimeSeconds", s.uptime_seconds)
        .cstr("lastResetReason", s.last_reset_reason.as_deref());
    let mut net = Obj::new();
    net.cstr("hostname", s.hostname.as_deref())
        .cstr("ssid", s.ssid.as_deref())
        .cstr("macAddr", s.mac_addr.as_deref())
        .cstr("ipAddr", s.ip_addr.as_deref())
        .cstr("wifiStatus", s.wifi_status.as_deref())
        .int("wifiRSSI", s.wifi_rssi as i64);
    d.obj("network", net);
    let mut mem = Obj::new();
    mem.uint("freeHeap", s.free_heap)
        .uint("freeHeapInt", s.free_heap_int);
    d.obj("memory", mem);
    d.into()
}

/// `v2_identify_t`: the values of `GET /api/v2/identify`.
#[derive(Clone, Debug, Default)]
pub struct V2Identify {
    pub device_model: Option<CString>,
    pub default_theme: Option<CString>,
    pub otp: bool,
    pub ap_active: bool,
    pub core_dump_download: bool,
    pub can_enabled: bool,
}

/// `GET /api/v2/identify`
pub fn v2_identify(s: &V2Identify) -> Json {
    let mut d = Obj::new();
    d.cstr("deviceModel", s.device_model.as_deref())
        .cstr("defaultTheme", s.default_theme.as_deref())
        .bool("otp", s.otp)
        .bool("apActive", s.ap_active)
        .bool("coreDumpDownload", s.core_dump_download);
    let mut can = Obj::new();
    can.bool("enabled", s.can_enabled);
    d.obj("can", can);
    d.into()
}

fn board_info() -> crate::board::api::Info {
    crate::board::api::info()
}

pub(crate) fn identify(req: &mut Request) -> EspErr {
    set_json(req);
    let b = board_info();
    let v = V2Identify {
        device_model: Some(b.device_model.into()),
        default_theme: Some(b.default_theme.into()),
        otp: cfg_flag(b"otp_enabled", 0),
        ap_active: crate::net::netmgr::ap_active(),
        core_dump_download: k::ESP_COREDUMP_ENABLE_TO_FLASH,
        can_enabled: cfg_flag(b"can_enabled", 0),
    };
    send_doc(req, v2_identify(&v))
}

pub(crate) fn system(req: &mut Request) -> EspErr {
    set_json(req);
    let b = board_info();
    let hostname = cfg_str(b"hostname", k::LWIP_LOCAL_HOSTNAME);
    let ssid = cfg_str(b"wifissid", k::ESP_WIFI_SSID);
    let version = std::ffi::CString::new(crate::net::mdns::fw_version()).unwrap_or_default();
    let ip = crate::system::ip();
    let wifi_status = crate::system::wifi_status();
    let v = V2System {
        device_model: Some(b.device_model.into()),
        asic_model: Some(b.asic_model.into()),
        version: Some(version),
        uptime_seconds: (crate::util::cutil::now_us() - crate::system::start_us()) / 1_000_000,
        last_reset_reason: Some(crate::system::reset_reason().into()),
        hostname: Some(hostname),
        ssid: Some(ssid),
        mac_addr: Some(crate::net::network::mac_addr().into()),
        ip_addr: Some(ip),
        wifi_status: Some(wifi_status),
        wifi_rssi: crate::net::network::wifi_rssi(),
        free_heap: crate::idf::task::free_heap(CAP_SPIRAM) as u64,
        free_heap_int: crate::idf::task::free_heap(CAP_INTERNAL) as u64,
    };
    send_doc(req, v2_system(&v))
}
