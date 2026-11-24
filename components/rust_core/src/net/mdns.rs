//! mDNS announcement (port of `main/network/mdns_service.cpp`): the host
//! name (default "nerdminer") as `<host>.local`, an `_http._tcp` service on
//! port 80 with the TXT records board, family, asic, asic_count and
//! fw_version, and the `_axeos` subtype; once.

use core::ffi::CStr;
#[cfg(not(target_os = "espidf"))]
use core::ffi::{c_char, c_int};
use std::ffi::CString;

use crate::idf::{nvs::err_name, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"mdns_svc";

#[cfg(target_os = "espidf")]
use crate::idf::raw::mdns_txt_item_t as TxtItem;

/// `mdns_txt_item_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[allow(dead_code)]
struct TxtItem {
    key: *const c_char,
    value: *const c_char,
}

/// `esp_app_desc_t` up to the version (host model)
#[cfg(not(target_os = "espidf"))]
#[allow(dead_code)]
struct AppDesc {
    magic_word: u32,
    secure_version: u32,
    reserv1: [u32; 2],
    version: [c_char; 32],
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_app_get_description, mdns_hostname_set, mdns_init, mdns_instance_name_set,
        mdns_service_add, mdns_service_subtype_add_for_host,
    };
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    use crate::idf::ESP_FAIL;
    pub unsafe fn mdns_init() -> c_int {
        ESP_FAIL
    }
    pub unsafe fn mdns_hostname_set(_: *const c_char) -> c_int {
        ESP_FAIL
    }
    pub unsafe fn mdns_instance_name_set(_: *const c_char) -> c_int {
        ESP_FAIL
    }
    pub unsafe fn mdns_service_add(
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
        _: u16,
        _: *mut TxtItem,
        _: usize,
    ) -> c_int {
        ESP_FAIL
    }
    pub unsafe fn mdns_service_subtype_add_for_host(
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
        _: *const c_char,
    ) -> c_int {
        ESP_FAIL
    }
    pub unsafe fn esp_app_get_description() -> *const AppDesc {
        core::ptr::null()
    }
}

static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The firmware version (`esp_app_get_description()->version`).
pub(crate) fn fw_version() -> String {
    // SAFETY: no arguments; returns a pointer to the static app descriptor (null on the host)
    let app = unsafe { sys::esp_app_get_description() };
    if app.is_null() {
        return String::new();
    }
    // SAFETY: `app` is non-null and points to the static descriptor whose `version` is
    // a NUL-terminated char array
    unsafe { CStr::from_ptr((*app).version.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

/// Starts mDNS (once): `hostname` (empty: "nerdminer") and the board data.
pub fn start(hostname: &str, board_version: i32, asic_count: i32, family: &str, asic: &str) {
    use std::sync::atomic::Ordering;
    if STARTED.load(Ordering::Acquire) {
        return;
    }
    // SAFETY: no arguments; init happens once (STARTED)
    let err = unsafe { sys::mdns_init() };
    if err != ESP_OK {
        log!(Error, TAG, "mdns_init failed: {}", err_name(err));
        return;
    }
    let host = if hostname.is_empty() {
        "nerdminer"
    } else {
        hostname
    };
    let c = |s: &str| CString::new(s).unwrap_or_default();
    let (host_c, ver_c, count_c, family_c, asic_c, fw) = (
        c(host),
        c(&board_version.to_string()),
        c(&asic_count.to_string()),
        c(family),
        c(asic),
        fw_version(),
    );
    let fw_c = c(&fw);
    // SAFETY: `host_c` is a live NUL-terminated CString; mdns copies the strings
    unsafe {
        sys::mdns_hostname_set(host_c.as_ptr());
        sys::mdns_instance_name_set(host_c.as_ptr());
    }
    let mut txt = [
        TxtItem {
            key: c"board".as_ptr(),
            value: ver_c.as_ptr(),
        },
        TxtItem {
            key: c"family".as_ptr(),
            value: family_c.as_ptr(),
        },
        TxtItem {
            key: c"asic".as_ptr(),
            value: asic_c.as_ptr(),
        },
        TxtItem {
            key: c"asic_count".as_ptr(),
            value: count_c.as_ptr(),
        },
        TxtItem {
            key: c"fw_version".as_ptr(),
            value: fw_c.as_ptr(),
        },
    ];
    // SAFETY: all strings are live NUL-terminated CStrings or static literals and `txt`
    // has `txt.len()` items; mdns copies the strings and items
    let err = unsafe {
        sys::mdns_service_add(
            host_c.as_ptr(),
            c"_http".as_ptr(),
            c"_tcp".as_ptr(),
            80,
            txt.as_mut_ptr(),
            txt.len(),
        )
    };
    if err != ESP_OK {
        log!(Warn, TAG, "mdns_service_add failed: {}", err_name(err));
        return;
    }
    // SAFETY: all strings are live NUL-terminated CStrings or static literals; the
    // instance name may be null
    let err = unsafe {
        sys::mdns_service_subtype_add_for_host(
            host_c.as_ptr(),
            c"_http".as_ptr(),
            c"_tcp".as_ptr(),
            core::ptr::null(),
            c"_axeos".as_ptr(),
        )
    };
    if err != ESP_OK {
        log!(Warn, TAG, "subtype add failed: {}", err_name(err));
    }
    log!(
        Info,
        TAG,
        "mDNS up: {host}.local (family={family}, asic={asic}, count={asic_count}, fw={fw})"
    );
    STARTED.store(true, Ordering::Release);
}
