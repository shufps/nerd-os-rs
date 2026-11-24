//! The factory update from GitHub (port of
//! `main/http_server/handler_ota_factory.cpp`): `POST
//! /api/system/OTA/github` starts it, `GET` reports its state
//! (`ota/factory.rs`). A task in internal RAM (the flash writes turn the
//! cache off) follows the redirects to the factory image, writes its
//! firmware part (4 MB at 0x10000) into the next OTA partition with the
//! board shut down, buffers the web UI part (3 MB) in PSRAM, writes it to
//! the `www` partition, erases the NVS unless the config is kept, and
//! restarts. A failure before the new firmware is active restarts into the
//! running one.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use std::ffi::CString;

#[cfg(not(target_os = "espidf"))]
use crate::http::ota::Partition;
use crate::http::ota::{factory_body, factory_url_ok, FactoryBody};
use crate::idf::http_client::{self, Event};
use crate::idf::httpd::{self, err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::ota::factory::{redirect, updater, Redirect, Step, FW_LEN, MAX_REDIRECTS, WWW_LEN};
use crate::power::mgmt;

const TAG: &CStr = c"http_ota";
const FW_START: usize = 0x10000;
const CHUNK: usize = 2048;
/// a redirect URL is cut at this length
const URL_SIZE: usize = 4096;
/// a request body
const MAX_BODY: usize = 4096;
idf_const!(ESP_ERR_NO_MEM: EspErr = ESP_ERR_NO_MEM, 0x101);
idf_const!(ESP_ERR_INVALID_SIZE: EspErr = ESP_ERR_INVALID_SIZE, 0x104);
idf_const!(ESP_ERR_NOT_FOUND: EspErr = ESP_ERR_NOT_FOUND, 0x105);
idf_const!(ESP_ERR_INVALID_RESPONSE: EspErr = ESP_ERR_INVALID_RESPONSE, 0x108);
idf_const!(PARTITION_TYPE_DATA: u32 = esp_partition_type_t_ESP_PARTITION_TYPE_DATA, 1);
idf_const!(PARTITION_SUBTYPE_SPIFFS: u32 = esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_SPIFFS, 0x82);
idf_const!(PARTITION_SUBTYPE_NVS: u32 = esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_NVS, 0x02);
idf_const!(OTA_SIZE_UNKNOWN: usize = OTA_SIZE_UNKNOWN, 0xffff_ffff);
idf_const!(CAP_INTERNAL_8BIT: u32 = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT, (1 << 11) | (1 << 2));
idf_const!(EVENT_ON_HEADER: u32 = esp_http_client_event_id_t_HTTP_EVENT_ON_HEADER, 3);

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_ota_abort, esp_ota_begin, esp_ota_end, esp_ota_get_next_update_partition,
        esp_ota_set_boot_partition, esp_ota_write, esp_partition_erase_range,
        esp_partition_find_first, esp_partition_write, heap_caps_free, heap_caps_malloc,
    };
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_partition_find_first(_: u32, _: u32, _: *const c_char) -> *const Partition {
        core::ptr::null()
    }
    pub unsafe fn esp_partition_erase_range(_: *const Partition, _: usize, _: usize) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_partition_write(
        _: *const Partition,
        _: usize,
        _: *const c_void,
        _: usize,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_get_next_update_partition(_: *const Partition) -> *const Partition {
        core::ptr::null()
    }
    pub unsafe fn esp_ota_begin(_: *const Partition, _: usize, _: *mut u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_write(_: u32, _: *const c_void, _: usize) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_abort(_: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_end(_: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_set_boot_partition(_: *const Partition) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn heap_caps_malloc(_: usize, _: u32) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn heap_caps_free(_: *mut c_void) {}
}

use crate::idf::task::delay_ms;

fn err_name(e: EspErr) -> String {
    crate::idf::nvs::err_name(e)
}

fn set_step(step: Step, msg: &str) {
    updater().set_step(step);
    log!(Info, TAG, "{msg}");
}

/// A chunk buffer in internal RAM (flash writes must not come from PSRAM).
struct Internal(*mut u8);

impl Internal {
    fn new() -> Option<Internal> {
        // SAFETY: heap_caps_malloc has no preconditions; a NULL result is checked below
        let p = unsafe { sys::heap_caps_malloc(CHUNK, CAP_INTERNAL_8BIT) } as *mut u8;
        if p.is_null() {
            log!(Error, TAG, "alloc failed: buf");
            return None;
        }
        // SAFETY: a fresh CHUNK-byte allocation; zeroed so that slice() never exposes
        // uninitialised bytes
        unsafe { core::ptr::write_bytes(p, 0, CHUNK) };
        Some(Internal(p))
    }
    fn slice(&mut self) -> &mut [u8] {
        // SAFETY: self.0 is the non-null CHUNK-byte allocation owned by self, and &mut self keeps
        // the slice unique
        unsafe { core::slice::from_raw_parts_mut(self.0, CHUNK) }
    }
}

impl Drop for Internal {
    fn drop(&mut self) {
        // SAFETY: self.0 came from heap_caps_malloc in Internal::new and is freed only here
        unsafe { sys::heap_caps_free(self.0 as *mut c_void) }
    }
}

/// The `Location` of a redirect, captured by the event handler.
struct RedirectCtx {
    capture: bool,
    location: Vec<u8>,
}

unsafe extern "C" fn capture_location(evt: *mut Event) -> EspErr {
    let Some(evt) = evt.as_ref() else {
        return ESP_OK;
    };
    if evt.user_data.is_null() {
        return ESP_OK;
    }
    let ctx = &mut *(evt.user_data as *mut RedirectCtx);
    if evt.event_id == EVENT_ON_HEADER && ctx.capture {
        let s = |p: *const c_char| {
            if p.is_null() {
                &b""[..]
            } else {
                CStr::from_ptr(p).to_bytes()
            }
        };
        if s(evt.header_key).eq_ignore_ascii_case(b"Location") {
            // the full value (with the query), cut like the former buffer
            let v = s(evt.header_value);
            ctx.location = v[..v.len().min(URL_SIZE - 1)].to_vec();
            log!(
                Info,
                TAG,
                "Location: {}",
                String::from_utf8_lossy(&ctx.location)
            );
        }
    }
    ESP_OK
}

/// Reads exactly `dst.len()` bytes: `ESP_ERR_INVALID_SIZE` on a premature
/// end, `ESP_FAIL` on a read error.
fn read_exact(c: &http_client::Client, dst: &mut [u8]) -> EspErr {
    let mut done = 0;
    while done < dst.len() {
        let got = c.read(&mut dst[done..]);
        if got < 0 {
            return ESP_FAIL;
        }
        if got == 0 {
            return ESP_ERR_INVALID_SIZE;
        }
        done += got as usize;
    }
    ESP_OK
}

fn skip_chunks(c: &http_client::Client, chunks: usize) -> EspErr {
    let mut dummy = vec![0u8; CHUNK];
    for _ in 0..chunks {
        let e = read_exact(c, &mut dummy);
        if e != ESP_OK {
            log!(Error, TAG, "skipping chunks failed");
            return e;
        }
    }
    ESP_OK
}

/// Follows the redirects to the body (kept open).
fn follow_redirects(c: &http_client::Client, url: &mut CString, ctx: &mut RedirectCtx) -> EspErr {
    for _ in 0..MAX_REDIRECTS {
        c.set_url(url);
        ctx.location.clear();
        ctx.capture = true;
        let e = c.open(0);
        if e != ESP_OK {
            ctx.capture = false;
            log!(Error, TAG, "open failed: {}", err_name(e));
            return e;
        }
        let _ = c.fetch_headers();
        let status = c.status();
        ctx.capture = false;
        match redirect(status, !ctx.location.is_empty()) {
            Redirect::Follow => {
                // GitHub sends absolute URLs
                *url = CString::new(ctx.location.clone()).unwrap_or_default();
                c.close();
            }
            Redirect::MissingLocation => {
                log!(Error, TAG, "Location header missing");
                c.close();
                return ESP_ERR_INVALID_RESPONSE;
            }
            // the connection stays open for the body
            Redirect::Body => return ESP_OK,
            Redirect::Unexpected => {
                log!(Error, TAG, "unexpected status: {status}");
                c.close();
                return ESP_FAIL;
            }
        }
    }
    log!(Error, TAG, "too many redirects");
    ESP_ERR_INVALID_RESPONSE
}

fn firmware_update(c: &http_client::Client) -> EspErr {
    let Some(mut buf) = Internal::new() else {
        return ESP_ERR_NO_MEM;
    };
    // SAFETY: NULL asks for the slot after the running partition; the result is checked for NULL
    let p = unsafe { sys::esp_ota_get_next_update_partition(core::ptr::null()) };
    if p.is_null() {
        log!(Error, TAG, "no OTA partition");
        return ESP_ERR_NOT_FOUND;
    }
    let mut h = 0u32;
    // SAFETY: p is a non-null partition entry (static in the partition table) and h is a writable
    // local
    let e = unsafe { sys::esp_ota_begin(p, OTA_SIZE_UNKNOWN, &mut h) };
    if e != ESP_OK {
        log!(Error, TAG, "esp_ota_begin failed: {}", err_name(e));
        return e;
    }
    for offset in (0..FW_LEN as usize).step_by(CHUNK) {
        let e = read_exact(c, buf.slice());
        if e != ESP_OK {
            // SAFETY: h is the open handle from esp_ota_begin, aborted once before returning
            unsafe { sys::esp_ota_abort(h) };
            return e;
        }
        if offset & 0xffff == 0 {
            log!(Info, TAG, "flashing to {offset:08x}");
        }
        // SAFETY: h is the open handle; buf.0 holds CHUNK bytes filled by read_exact
        if unsafe { sys::esp_ota_write(h, buf.0 as *const c_void, CHUNK) } != ESP_OK {
            // SAFETY: h is the open handle, aborted once before returning
            unsafe { sys::esp_ota_abort(h) };
            return ESP_FAIL;
        }
        updater().add(0, CHUNK as u32);
    }
    // validate and activate (the restart comes later)
    // SAFETY: h is the open handle (ended once here), p the partition it was begun on
    if unsafe { sys::esp_ota_end(h) != ESP_OK || sys::esp_ota_set_boot_partition(p) != ESP_OK } {
        return ESP_FAIL;
    }
    ESP_OK
}

fn www_update(data: &[u8]) -> EspErr {
    let Some(mut buf) = Internal::new() else {
        return ESP_ERR_NO_MEM;
    };
    // SAFETY: plain constants and a static NUL-terminated label; the result is checked for NULL
    let p = unsafe {
        sys::esp_partition_find_first(
            PARTITION_TYPE_DATA,
            PARTITION_SUBTYPE_SPIFFS,
            c"www".as_ptr(),
        )
    };
    if p.is_null() {
        return ESP_ERR_NOT_FOUND;
    }
    // SAFETY: p is non-null (checked above) and points to a static partition table entry
    let size = unsafe { (*p).size } as usize;
    if WWW_LEN as usize > size {
        log!(
            Error,
            TAG,
            "WWW image larger than partition ({WWW_LEN} > {size})"
        );
        return ESP_ERR_INVALID_SIZE;
    }
    log!(Info, TAG, "erasing www partition ...");
    // SAFETY: p is a valid partition entry and 0..size is its whole range
    let e = unsafe { sys::esp_partition_erase_range(p, 0, size) };
    if e != ESP_OK {
        log!(Error, TAG, "erasing www failed: {}", err_name(e));
        return e;
    }
    log!(Info, TAG, "erasing done");
    for offset in (0..WWW_LEN as usize).step_by(CHUNK) {
        // from PSRAM into internal RAM
        buf.slice().copy_from_slice(&data[offset..offset + CHUNK]);
        if offset & 0xffff == 0 {
            log!(Info, TAG, "flashing to {offset:08x}");
        }
        // SAFETY: p is a valid partition entry; buf.0 holds CHUNK bytes and offset + CHUNK <=
        // WWW_LEN <= size
        if unsafe { sys::esp_partition_write(p, offset, buf.0 as *const c_void, CHUNK) } != ESP_OK {
            return ESP_FAIL;
        }
        updater().add(1, CHUNK as u32);
    }
    ESP_OK
}

fn erase_nvs() -> EspErr {
    // SAFETY: plain constants and a static NUL-terminated label; the result is checked for NULL
    let p = unsafe {
        sys::esp_partition_find_first(PARTITION_TYPE_DATA, PARTITION_SUBTYPE_NVS, c"nvs".as_ptr())
    };
    if p.is_null() {
        log!(Error, TAG, "NVS partition not found!");
        return ESP_ERR_NOT_FOUND;
    }
    log!(
        Info,
        TAG,
        "erasing NVS partition (reset to factory defaults) ..."
    );
    // SAFETY: p is non-null (checked above), a static partition table entry; the range is the whole
    // partition
    let e = unsafe { sys::esp_partition_erase_range(p, 0, (*p).size as usize) };
    if e != ESP_OK {
        log!(Error, TAG, "NVS erase failed: {}", err_name(e));
    } else {
        log!(Info, TAG, "NVS partition erased successfully");
    }
    e
}

/// The hardware lock for a scope.
struct PmLock;

impl PmLock {
    fn new() -> PmLock {
        mgmt::lock();
        PmLock
    }
}

impl Drop for PmLock {
    fn drop(&mut self) {
        mgmt::unlock();
    }
}

fn update(start_url: &[u8], keep_config: bool) -> EspErr {
    let mut url = CString::new(&start_url[..start_url.len().min(URL_SIZE - 1)]).unwrap_or_default();
    // the web UI part in PSRAM (one partition at a time)
    let mut www = Vec::new();
    if www.try_reserve_exact(WWW_LEN as usize).is_err() {
        log!(Error, TAG, "alloc failed: wwwData");
        return ESP_ERR_NO_MEM;
    }
    www.resize(WWW_LEN as usize, 0u8);
    let mut ctx = Box::new(RedirectCtx {
        capture: false,
        location: Vec::new(),
    });

    let config = http_client::Config {
        url: url.as_ptr(),
        timeout_ms: 60000,
        buffer_size: 8192,
        buffer_size_tx: 8192,
        keep_alive_enable: false,
        // the redirects by hand
        disable_auto_redirect: true,
        user_agent: c"ESP32-DemoDownloader/1.0".as_ptr(),
        event_handler: Some(capture_location),
        user_data: &mut *ctx as *mut RedirectCtx as *mut c_void,
        ..http_client::Config::with_bundle()
    };
    let Some(c) = http_client::Client::from_config(&config) else {
        set_step(Step::Error, "OTA: error");
        return ESP_FAIL;
    };
    c.set_method(http_client::METHOD_GET);
    // no compression (Content-Length = the bytes read), close afterwards
    c.set_header(c"Accept-Encoding", c"identity");
    c.set_header(c"Connection", c"close");

    updater().reset_progress();
    set_step(Step::ResolvingUrl, "OTA: resolving/following redirects");
    let e = follow_redirects(&c, &mut url, &mut ctx);
    if e != ESP_OK {
        log!(Error, TAG, "following redirects failed");
        set_step(Step::Error, "OTA: error");
        return e;
    }
    set_step(Step::ResolvingUrl, "OTA: final URL OK");

    let clen = c.content_length();
    log!(Info, TAG, "Final URL OK. Content-Length={clen}");
    // unknown (-1) is fine, else at least firmware and web UI
    let min_needed = (FW_START as u64 + FW_LEN + WWW_LEN) as i64;
    if clen != -1 && clen < min_needed {
        log!(Error, TAG, "factory image too small: {clen} < {min_needed}");
        set_step(Step::Error, "OTA: error");
        return ESP_ERR_INVALID_SIZE;
    }

    let e = skip_chunks(&c, FW_START / CHUNK);
    if e != ESP_OK {
        log!(Error, TAG, "error reading stream");
        set_step(Step::Error, "OTA: error");
        return e;
    }

    {
        let _g = PmLock::new();
        // the regulator off: the PID cannot run during the flash writes
        // (I2C), and the device restarts afterwards anyway
        mgmt::shutdown();
        log!(Info, TAG, "performing firmware update ...");
        set_step(Step::UpdatingFw, "OTA: updating firmware");
        let e = firmware_update(&c);
        if e != ESP_OK {
            log!(Error, TAG, "firmware update failed");
            set_step(Step::Error, "OTA: error");
            // the boot partition is unchanged: restart into the running
            // firmware
            delay_ms(3000);
            mgmt::restart();
        }
        log!(Info, TAG, "firmware update successful!");

        log!(Info, TAG, "download www binary ...");
        set_step(Step::DownloadingWww, "OTA: downloading www");
        for chunk in www.chunks_mut(CHUNK) {
            if read_exact(&c, chunk) != ESP_OK {
                log!(Error, TAG, "error reading www binary");
                mgmt::restart();
            }
            updater().add(2, CHUNK as u32);
        }

        log!(Info, TAG, "performing www update ...");
        set_step(Step::UpdatingWww, "OTA: updating www");
        if www_update(&www) != ESP_OK {
            log!(Error, TAG, "www update failed");
            // the new firmware is active already: restart
            mgmt::restart();
        }
        log!(Info, TAG, "www update successful!");

        if !keep_config {
            set_step(
                Step::ErasingNvs,
                "OTA: erasing NVS (reset to factory defaults)",
            );
            if erase_nvs() != ESP_OK {
                // best effort, the device restarts anyway
                log!(Warn, TAG, "NVS erase failed, continuing with reboot");
            }
        }
    }

    log!(
        Info,
        TAG,
        "Restarting System because of Firmware update complete"
    );
    set_step(Step::Rebooting, "OTA: done, rebooting");
    // the web UI sees this state
    delay_ms(3000);
    mgmt::restart()
}

unsafe extern "C" fn task(_: *mut c_void) {
    log!(Info, TAG, "OTA task started...");
    loop {
        let (url, keep_config) = updater().wait();
        log!(Info, TAG, "OTA update triggered.");
        if url.is_empty() {
            log!(Error, TAG, "update url is null!");
            updater().done();
            continue;
        }
        if update(&url, keep_config) != ESP_OK {
            log!(Error, TAG, "OTA update failed! Please check the log");
        }
        // a new request is accepted
        updater().done();
    }
}

/// Starts the update task (its stack in internal RAM: the flash writes
/// turn the cache off).
pub fn start() {
    crate::idf::task::create_internal(task, c"ota updater", 8192, 1);
}

pub(crate) mod api {
    pub fn ota_factory_start() {
        super::start();
    }
}

fn send_err(req: &mut Request, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

/// `POST /api/system/OTA/github`: `{"url": "https://github.com/shufps/...",
/// "keep_config": bool}`
pub fn post_handler(req: &mut Request) -> EspErr {
    let need = req.content_len();
    if need == 0 || need > MAX_BODY {
        return send_err(req, err::E400_BAD_REQUEST, c"Body too large or empty");
    }
    let mut body = vec![0u8; need];
    let mut off = 0;
    while off < need {
        let n = req.recv(&mut body[off..]);
        if n <= 0 {
            return send_err(req, err::E400_BAD_REQUEST, c"recv failed");
        }
        off += n as usize;
    }
    // as the C string it was
    let body = crate::util::cutil::until_nul(&body);
    let (url, keep_config) = match factory_body(body) {
        FactoryBody::Invalid => {
            log!(Error, TAG, "JSON parsing failed");
            return send_err(req, err::E400_BAD_REQUEST, c"Invalid JSON");
        }
        FactoryBody::NoUrl => return send_err(req, err::E400_BAD_REQUEST, c"`url` missing"),
        FactoryBody::Ok { url, keep_config } => (url, keep_config),
    };
    let url = crate::util::cutil::until_nul(&url).to_vec();
    let text = String::from_utf8_lossy(&url).into_owned();
    if !factory_url_ok(&url) {
        log!(Error, TAG, "Rejected URL: {text}");
        return send_err(req, err::E400_BAD_REQUEST, c"Invalid or unsafe URL");
    }
    log!(
        Info,
        TAG,
        "starting OTA update for URL: {text} (keep_config={})",
        keep_config as c_int
    );
    if !updater().trigger(&url, keep_config) {
        req.set_status(c"409 Conflict");
        return req.send(b"{\"status\":\"busy\"}");
    }
    req.set_type(c"application/json");
    req.set_status(c"202 Accepted");
    req.send(b"{\"status\":\"started\"}")
}

/// `GET /api/system/OTA/github`: `{"pending", "running", "step",
/// "progress"}`
pub fn status_handler(req: &mut Request) -> EspErr {
    req.set_hdr(c"Cache-Control", c"no-store, no-cache, must-revalidate");
    let json = updater().status_json();
    req.set_type(c"application/json");
    req.send(json.as_bytes())
}
