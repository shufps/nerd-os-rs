//! The HTTP server (port of `main/http_server/http_server.cpp`,
//! `http_cors.cpp` and `http_utils.cpp`): starts `esp_http_server`
//! (wildcard URIs, 10 sockets, LRU purge, no keep-alive), registers the
//! routes of `http_routes.rs` with one trampoline that carries out the
//! checks (network, CORS headers, OTP) before the handler, answers unknown
//! URIs with a redirect to `/` (captive portal), serves the recovery page
//! and the CORS preflights, and starts the captive portal DNS server. The
//! handlers are in the `http_*` modules. C API: `http_server.h`.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use crate::http_routes::{self, flag, Handler, Method, Route, ROUTES};
use crate::idf::httpd::{self, err, Req};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"http_server";

/// `SCRATCH_BUFSIZE`
pub const SCRATCH_SIZE: usize = 16384;
idf_const!(BASE_PATH_SIZE: usize = ESP_VFS_PATH_MAX + 1, 16);

/// `rest_server_context_t`: the request body buffer of the handlers.
#[repr(C)]
pub struct ServerCtx {
    pub base_path: [c_char; BASE_PATH_SIZE],
    pub scratch: [c_char; SCRATCH_SIZE],
}

/// The page served in recovery mode (the web UI could not be mounted).
static RECOVERY_PAGE: &[u8] = include_bytes!("recovery_page.html");

/// `http_server`: the server handle (the web socket code sends with it).
#[no_mangle]
pub static http_server: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());

/// The web UI filesystem failed: only the recovery page
/// and the uploads are served.
pub static ENTER_RECOVERY: AtomicBool = AtomicBool::new(false);

fn recovery() -> bool {
    ENTER_RECOVERY.load(Ordering::Acquire)
}

fn server() -> httpd::Handle {
    http_server.load(Ordering::Acquire)
}

/// core dumps in flash: the download route exists
const COREDUMP: bool = crate::sdkconfig::ESP_COREDUMP_ENABLE_TO_FLASH;

/// A route as registered: its entry, the handler, the handler's
/// `user_ctx` (the server context or NULL).
struct RouteCtx {
    route: &'static Route,
    handler: Handler,
    cpp: Option<httpd::HandlerFn>,
    ctx: *mut c_void,
}

// ---------------------------------------------------------------------------
// helpers for the handlers

unsafe fn set_hdr(req: *mut Req, field: &CStr, value: &CStr) -> EspErr {
    httpd::httpd_resp_set_hdr(req, field.as_ptr(), value.as_ptr())
}

unsafe fn send_err(req: *mut Req, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    httpd::httpd_resp_send_err(req, code, msg.as_ptr())
}

/// A request header (NUL-terminated into `out`); None if missing or too
/// long.
unsafe fn header<'a>(req: *mut Req, name: &CStr, out: &'a mut [c_char]) -> Option<&'a CStr> {
    let len = httpd::httpd_req_get_hdr_value_len(req, name.as_ptr());
    if len == 0 || len + 1 > out.len() {
        return None;
    }
    if httpd::httpd_req_get_hdr_value_str(req, name.as_ptr(), out.as_mut_ptr(), out.len()) != ESP_OK
    {
        return None;
    }
    Some(CStr::from_ptr(out.as_ptr()))
}

/// `is_network_allowed()`: the decision is `http_auth.rs`.
unsafe fn network_allowed(req: *mut Req) -> bool {
    const T: &CStr = c"http_cors";
    let ap_mode = crate::system::ap_state();
    let mut ip = std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED);
    if !ap_mode {
        let fd = httpd::httpd_req_to_sockfd(req);
        let mut addr = httpd::SockaddrIn6::default();
        let mut len = core::mem::size_of::<httpd::SockaddrIn6>() as u32;
        if httpd::lwip_getpeername(fd, &mut addr, &mut len) < 0 {
            log!(Error, T, "Error getting client IP");
            return false;
        }
        // the words are in network byte order; an IPv4 peer is IPv4-mapped
        let mut b = [0u8; 16];
        for (d, w) in b.chunks_exact_mut(4).zip(addr.sin6_addr) {
            d.copy_from_slice(&w.to_ne_bytes());
        }
        let v6 = std::net::Ipv6Addr::from(b);
        ip = v6.to_ipv4_mapped().map_or(v6.into(), Into::into);
    }
    // an Origin header that does not fit counts as none, as before
    let mut buf = [0 as c_char; 128];
    let origin =
        if httpd::httpd_req_get_hdr_value_str(req, c"Origin".as_ptr(), buf.as_mut_ptr(), buf.len())
            == ESP_OK
        {
            Some(CStr::from_ptr(buf.as_ptr()))
        } else {
            None
        };
    let hostname = crate::system::hostname();
    if crate::http_auth::network_allowed(
        ap_mode,
        ip,
        origin.map(CStr::to_bytes),
        hostname.to_bytes(),
    ) {
        return true;
    }
    let origin = origin.map_or("-".into(), CStr::to_string_lossy);
    log!(Error, T, "denied: {ip} (origin {origin})");
    false
}

/// `set_cors_headers()`
pub(crate) unsafe fn set_cors_headers(req: *mut Req) -> bool {
    set_hdr(req, c"Access-Control-Allow-Origin", c"*") == ESP_OK
        && set_hdr(
            req,
            c"Access-Control-Allow-Methods",
            c"GET, POST, PUT, PATCH, DELETE, OPTIONS",
        ) == ESP_OK
        && set_hdr(
            req,
            c"Access-Control-Allow-Headers",
            c"Content-Type, Authorization, X-TOTP, X-OTP-Session",
        ) == ESP_OK
        && set_hdr(
            req,
            c"Access-Control-Expose-Headers",
            c"X-ESP-App-ELF-SHA256",
        ) == ESP_OK
}

/// `validateOTP()`: the session token or the TOTP header; sends 401 if
/// neither is valid (`force`: also with OTP disabled, no session token).
///
/// # Safety
/// `req` valid.
pub unsafe fn validate_otp(req: *mut Req, force: bool) -> EspErr {
    // a token is 20 + 1 + 52 characters
    let mut sess = [0 as c_char; 128];
    let mut totp = [0 as c_char; 16];
    let sess = header(req, c"X-OTP-Session", &mut sess).map_or(core::ptr::null(), CStr::as_ptr);
    let totp = header(req, c"X-TOTP", &mut totp).map_or(core::ptr::null(), CStr::as_ptr);
    match crate::otp_device::ffi::otp_dev_request_check(sess, totp, force) {
        0 => ESP_OK,
        1 => {
            send_err(req, err::E401_UNAUTHORIZED, c"blocked for 5 minutes");
            ESP_FAIL
        }
        _ => {
            send_err(req, err::E401_UNAUTHORIZED, c"OTP/Session required");
            ESP_FAIL
        }
    }
}

/// `getPostData()`: the body into the server context's scratch buffer
/// (NUL-terminated); 500 if it does not fit or fails.
///
/// # Safety
/// `req` a request of a route with the server context.
pub unsafe fn post_data(req: *mut Req) -> EspErr {
    const T: &CStr = c"http_utils";
    let total = (*req).content_len;
    let ctx = (*req).user_ctx as *mut ServerCtx;
    if total >= SCRATCH_SIZE {
        log!(Error, T, "content too long");
        send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"content too long");
        return ESP_FAIL;
    }
    let buf = (*ctx).scratch.as_mut_ptr();
    let mut cur = 0;
    while cur < total {
        let n = httpd::httpd_req_recv(req, buf.add(cur), total - cur);
        if n <= 0 {
            log!(Error, T, "error receiving data");
            send_err(
                req,
                err::E500_INTERNAL_SERVER_ERROR,
                c"error receiving data",
            );
            return ESP_FAIL;
        }
        cur += n as usize;
    }
    *buf.add(total) = 0;
    ESP_OK
}

/// `sendJsonBuffer()`: sends `json` (`len` bytes, `malloc()`ed) in 2048
/// byte chunks and frees it; 500 if it is NULL.
///
/// # Safety
/// `req` valid; `json` `malloc()`ed (or NULL), `len` bytes.
pub unsafe fn send_json(req: *mut Req, json: *mut c_char, len: usize) -> EspErr {
    if json.is_null() {
        send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Out of memory");
        return ESP_FAIL;
    }
    const CHUNK: usize = 2048;
    let data = core::slice::from_raw_parts(json as *const u8, len);
    let mut ok = true;
    for chunk in data.chunks(CHUNK) {
        if httpd::httpd_resp_send_chunk(req, chunk.as_ptr() as *const c_char, chunk.len() as isize)
            != ESP_OK
        {
            ok = false;
            break;
        }
    }
    crate::cutil::c_free(json as *mut c_void);
    if !ok {
        return ESP_FAIL;
    }
    httpd::httpd_resp_send_chunk(req, core::ptr::null(), 0)
}

// ---------------------------------------------------------------------------
// the server

unsafe extern "C" fn recovery_handler(req: *mut Req) -> EspErr {
    httpd::httpd_resp_set_type(req, c"text/html; charset=UTF-8".as_ptr());
    httpd::httpd_resp_send(
        req,
        RECOVERY_PAGE.as_ptr() as *const c_char,
        RECOVERY_PAGE.len() as isize,
    );
    ESP_OK
}

/// CORS preflight: the headers are set by the route's checks.
unsafe extern "C" fn options_handler(req: *mut Req) -> EspErr {
    httpd::httpd_resp_send(req, core::ptr::null(), 0);
    ESP_OK
}

/// Closes the connection after the response (`ConGuard`).
unsafe fn close_after(req: *mut Req) {
    let fd = httpd::httpd_req_to_sockfd(req);
    let s = server();
    if fd >= 0 && !s.is_null() {
        httpd::httpd_sess_trigger_close(s, fd);
    }
}

/// Unknown URIs: a redirect to `/` (captive portal).
unsafe extern "C" fn not_found_handler(req: *mut Req, _: httpd::ErrCode) -> EspErr {
    set_hdr(req, c"Connection", c"close");
    httpd::httpd_resp_set_status(req, c"302 Temporary Redirect".as_ptr());
    set_hdr(req, c"Location", c"/");
    // iOS needs content to detect a captive portal
    httpd::httpd_resp_send(
        req,
        c"Redirect to the captive portal".as_ptr(),
        httpd::RESP_USE_STRLEN,
    );
    log!(Info, TAG, "Redirecting to root");
    close_after(req);
    ESP_OK
}

unsafe extern "C" fn close_cb(_: httpd::Handle, fd: c_int) {
    // our web socket being closed resets the logging
    crate::http_ws::on_close(fd);
    if fd >= 0 {
        httpd::lwip_close(fd);
    }
}

unsafe extern "C" fn open_cb(_: httpd::Handle, _: c_int) -> EspErr {
    ESP_OK
}

unsafe fn checks_and_handler(req: *mut Req, rc: &RouteCtx) -> EspErr {
    let flags = rc.route.flags;
    if flags & flag::NET != 0 && !network_allowed(req) {
        return send_err(req, err::E401_UNAUTHORIZED, c"Unauthorized");
    }
    if flags & flag::CORS != 0 && !set_cors_headers(req) {
        httpd::httpd_resp_send_err(req, err::E500_INTERNAL_SERVER_ERROR, core::ptr::null());
        return ESP_FAIL;
    }
    if http_routes::needs_otp(rc.route, recovery()) {
        // forced: also with OTP disabled, and no session token
        let force = flags & flag::OTP_FORCE != 0;
        if validate_otp(req, force) != ESP_OK {
            if force {
                log!(Error, TAG, "totp validation failed");
            }
            return ESP_FAIL;
        }
    }
    match (rc.handler, rc.cpp) {
        (Handler::Recovery, _) => recovery_handler(req),
        (Handler::Options, _) => options_handler(req),
        (_, Some(f)) => f(req),
        _ => ESP_FAIL,
    }
}

unsafe extern "C" fn trampoline(req: *mut Req) -> EspErr {
    let rc = &*((*req).user_ctx as *const RouteCtx);
    (*req).user_ctx = rc.ctx;
    if rc.route.flags & flag::CLOSE != 0 {
        set_hdr(req, c"Connection", c"close");
        let r = checks_and_handler(req, rc);
        close_after(req);
        return r;
    }
    checks_and_handler(req, rc)
}

fn method(m: Method) -> httpd::Method {
    match m {
        Method::Get => httpd::method::GET,
        Method::Post => httpd::method::POST,
        Method::Patch => httpd::method::PATCH,
        Method::Delete => httpd::method::DELETE,
        Method::Options => httpd::method::OPTIONS,
    }
}

unsafe fn register_routes(ctx: *mut ServerCtx) {
    let s = server();
    for rt in ROUTES.iter() {
        let builtin = matches!(rt.handler, Handler::Recovery | Handler::Options);
        let cpp = if builtin {
            None
        } else {
            crate::http_handlers::handler(rt.handler)
                .or(match rt.handler {
                    Handler::Websocket => Some(crate::http_ws::handler as httpd::HandlerFn),
                    Handler::File => Some(crate::http_files::file_handler as httpd::HandlerFn),
                    Handler::Ota => Some(crate::ota_upload::firmware_update as httpd::HandlerFn),
                    Handler::OtaWww => Some(crate::ota_upload::www_update as httpd::HandlerFn),
                    Handler::Coredump => Some(crate::http_coredump::handler as httpd::HandlerFn),
                    Handler::CanNodes => Some(crate::http_can::nodes as httpd::HandlerFn),
                    Handler::CanSlaveAction => Some(crate::http_can::action as httpd::HandlerFn),
                    Handler::CanSlavePatch => Some(crate::http_can::patch as httpd::HandlerFn),
                    Handler::CanSlaveDelete => Some(crate::http_can::delete as httpd::HandlerFn),
                    Handler::WifiScan => Some(crate::http_handlers::wifi_scan as httpd::HandlerFn),
                    Handler::ProfilesGet => Some(crate::profiles::http::get as httpd::HandlerFn),
                    Handler::ProfilesPatch => {
                        Some(crate::profiles::http::patch as httpd::HandlerFn)
                    }
                    Handler::ProfileApply => Some(crate::profiles::http::apply as httpd::HandlerFn),
                    Handler::OtaGithubPost => {
                        Some(crate::ota_factory_task::post_handler as httpd::HandlerFn)
                    }
                    Handler::OtaGithubGet => {
                        Some(crate::ota_factory_task::status_handler as httpd::HandlerFn)
                    }
                    _ => None,
                })
                .or_else(|| crate::http_data::handler(rt.handler))
        };
        if !http_routes::active(rt, recovery(), COREDUMP) || (!builtin && cpp.is_none()) {
            continue;
        }
        let rc: &'static RouteCtx = Box::leak(Box::new(RouteCtx {
            route: rt,
            handler: rt.handler,
            cpp,
            ctx: if rt.flags & flag::CTX != 0 {
                ctx as *mut c_void
            } else {
                core::ptr::null_mut()
            },
        }));
        let u = httpd::Uri {
            uri: rt.uri,
            method: method(rt.method),
            handler: Some(trampoline),
            user_ctx: rc as *const RouteCtx as *mut c_void,
            is_websocket: rt.flags & flag::WEBSOCKET != 0,
            handle_ws_control_frames: false,
            supported_subprotocol: core::ptr::null(),
        };
        let e = httpd::httpd_register_uri_handler(s, &u);
        if e != ESP_OK {
            log!(
                Error,
                TAG,
                "registering {} failed: {}",
                rt.uri(),
                crate::idf::nvs::err_name(e)
            );
        }
    }
}

/// `start_rest_server()`
pub fn start() -> EspErr {
    // the web UI filesystem missing: recovery mode
    if crate::http_files::init_fs() != ESP_OK {
        ENTER_RECOVERY.store(true, Ordering::Release);
    }

    let ctx = crate::cutil::c_malloc(core::mem::size_of::<ServerCtx>()) as *mut ServerCtx;
    if ctx.is_null() {
        log!(Error, TAG, "No memory for rest context");
        return ESP_FAIL;
    }
    // the base path is ""
    unsafe { core::ptr::write_bytes(ctx, 0, 1) };

    let mut config = httpd::default_config();
    config.uri_match_fn = Some(httpd::httpd_uri_match_wildcard);
    config.max_uri_handlers = 56;
    config.lru_purge_enable = true;
    config.max_open_sockets = 10;
    config.stack_size = 12288;
    config.keep_alive_enable = false;
    config.recv_wait_timeout = 5;
    config.send_wait_timeout = 5;
    config.close_fn = Some(close_cb);
    config.open_fn = Some(open_cb);

    log!(Info, TAG, "Starting HTTP Server");
    let mut handle: httpd::Handle = core::ptr::null_mut();
    if unsafe { httpd::httpd_start(&mut handle, &config) } != ESP_OK {
        log!(Error, TAG, "Start server failed");
        unsafe { crate::cutil::c_free(ctx as *mut c_void) };
        return ESP_FAIL;
    }
    http_server.store(handle, Ordering::Release);

    unsafe {
        register_routes(ctx);
        httpd::httpd_register_err_handler(handle, err::E404_NOT_FOUND, Some(not_found_handler));
    }
    // the queue and the task of the log over the web socket
    crate::http_ws::start();

    // the captive portal: every A query answered with the access point's
    // address
    crate::dns::start_server();
    ESP_OK
}

pub(crate) mod ffi {
    use super::*;

    #[no_mangle]
    pub extern "C" fn start_rest_server(_: *mut c_void) -> EspErr {
        start()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_page_is_the_former_one() {
        assert_eq!(RECOVERY_PAGE.len(), 2967);
        assert!(RECOVERY_PAGE.starts_with(b"<html><meta charset=\"UTF-8\">"));
        assert!(RECOVERY_PAGE.ends_with(b"</html>"));
    }

    #[test]
    fn context_layout() {
        assert_eq!(core::mem::size_of::<ServerCtx>(), 16 + 16384);
    }
}
