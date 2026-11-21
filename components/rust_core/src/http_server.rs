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
use crate::idf::httpd::{self, err, Req, Request};
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

fn scratch(req: &mut Request) -> Option<&mut [u8]> {
    let ctx = req.user_ctx() as *mut ServerCtx;
    // SAFETY: the context the route was registered with; the server runs
    // one handler at a time, and the slice is bound to the request borrow
    unsafe { ctx.as_mut() }.map(|c| {
        // SAFETY: c_char and u8 have the same layout
        unsafe { core::slice::from_raw_parts_mut(c.scratch.as_mut_ptr() as *mut u8, SCRATCH_SIZE) }
    })
}

/// Runs `f` with the request and the server context's scratch buffer (file
/// and core dump transfers) of a route with the context (`flag::CTX`);
/// None without the context.
pub fn with_scratch<T>(
    req: &mut Request,
    f: impl FnOnce(&mut Request, &mut [u8]) -> T,
) -> Option<T> {
    let buf = scratch(req)?.as_mut_ptr();
    // SAFETY: the buffer is in the server context, not in the request, so
    // the two borrows are disjoint; one handler runs at a time
    let buf = unsafe { core::slice::from_raw_parts_mut(buf, SCRATCH_SIZE) };
    Some(f(req, buf))
}

/// `is_network_allowed()`: the decision is `http_auth.rs`.
fn network_allowed(req: &mut Request) -> bool {
    const T: &CStr = c"http_cors";
    let ap_mode = crate::system::ap_state();
    let mut ip_be = 0u32;
    if !ap_mode {
        let fd = req.sockfd();
        let mut addr = httpd::SockaddrIn6::default();
        let mut len = core::mem::size_of::<httpd::SockaddrIn6>() as u32;
        // SAFETY: addr/len are writable and len is the size of addr
        if unsafe { httpd::lwip_getpeername(fd, &mut addr, &mut len) } < 0 {
            log!(Error, T, "Error getting client IP");
            return false;
        }
        // the IPv4-mapped address, in network byte order
        ip_be = addr.sin6_addr[3];
    }
    // an Origin header that does not fit counts as none, as before
    let origin = req.header(c"Origin", 128);
    let hostname = crate::system::hostname();
    if crate::http_auth::network_allowed(
        ap_mode,
        u32::from_be(ip_be),
        origin.as_deref().map(CStr::to_bytes),
        hostname.to_bytes(),
    ) {
        return true;
    }
    let ip = std::net::Ipv4Addr::from(u32::from_be(ip_be));
    let origin = origin.as_deref().map_or("-".into(), CStr::to_string_lossy);
    log!(Error, T, "denied: {ip} (origin {origin})");
    false
}

/// `set_cors_headers()`
pub(crate) fn set_cors_headers(req: &mut Request) -> bool {
    req.set_hdr(c"Access-Control-Allow-Origin", c"*") == ESP_OK
        && req.set_hdr(
            c"Access-Control-Allow-Methods",
            c"GET, POST, PUT, PATCH, DELETE, OPTIONS",
        ) == ESP_OK
        && req.set_hdr(
            c"Access-Control-Allow-Headers",
            c"Content-Type, Authorization, X-TOTP, X-OTP-Session",
        ) == ESP_OK
        && req.set_hdr(c"Access-Control-Expose-Headers", c"X-ESP-App-ELF-SHA256") == ESP_OK
}

/// A request header of at most `cap - 1` bytes, as before: None if missing
/// or longer.
fn header(req: &mut Request, name: &CStr, cap: usize) -> Option<std::ffi::CString> {
    let len = req.header_len(name);
    if len == 0 || len + 1 > cap {
        return None;
    }
    req.header(name, cap)
}

/// `validateOTP()`: the session token or the TOTP header; sends 401 if
/// neither is valid (`force`: also with OTP disabled, no session token).
pub fn validate_otp(req: &mut Request, force: bool) -> EspErr {
    // a token is 20 + 1 + 52 characters
    let sess = header(req, c"X-OTP-Session", 128);
    let totp = header(req, c"X-TOTP", 16);
    use crate::clock;
    let auth = crate::otp::request(
        force,
        sess.as_deref().map(CStr::to_bytes),
        totp.as_deref().map(CStr::to_bytes),
        clock::now() as i64,
        clock::is_time_synced(),
        clock::now_ms(),
    );
    match auth {
        crate::otp::Auth::Ok => ESP_OK,
        crate::otp::Auth::Blocked => {
            req.send_err(err::E401_UNAUTHORIZED, Some(c"blocked for 5 minutes"));
            ESP_FAIL
        }
        crate::otp::Auth::Failed => {
            req.send_err(err::E401_UNAUTHORIZED, Some(c"OTP/Session required"));
            ESP_FAIL
        }
    }
}

/// `getPostData()`: the body (up to a NUL byte, as before) through the
/// server context's scratch buffer; sends 500 and returns the error if it
/// does not fit or fails.
pub fn read_body(req: &mut Request) -> Result<&[u8], EspErr> {
    const T: &CStr = c"http_utils";
    let total = req.content_len();
    if total >= SCRATCH_SIZE {
        log!(Error, T, "content too long");
        req.send_err(err::E500_INTERNAL_SERVER_ERROR, Some(c"content too long"));
        return Err(ESP_FAIL);
    }
    // read into a copy of the pointer: recv() needs the request as well
    let Some(buf) = scratch(req).map(|b| b.as_mut_ptr()) else {
        req.send_err(err::E500_INTERNAL_SERVER_ERROR, None);
        return Err(ESP_FAIL);
    };
    let mut cur = 0;
    while cur < total {
        // SAFETY: the scratch buffer holds SCRATCH_SIZE > total bytes
        let dst = unsafe { core::slice::from_raw_parts_mut(buf.add(cur), total - cur) };
        let n = req.recv(dst);
        if n <= 0 {
            log!(Error, T, "error receiving data");
            req.send_err(
                err::E500_INTERNAL_SERVER_ERROR,
                Some(c"error receiving data"),
            );
            return Err(ESP_FAIL);
        }
        cur += n as usize;
    }
    let body = &scratch(req).unwrap_or_default()[..total];
    Ok(crate::cutil::until_nul(body))
}

/// `sendJsonBuffer()`: sends `json` in 2048 byte chunks.
pub fn send_json(req: &mut Request, json: &[u8]) -> EspErr {
    const CHUNK: usize = 2048;
    for chunk in json.chunks(CHUNK) {
        if req.send_chunk(chunk) != ESP_OK {
            return ESP_FAIL;
        }
    }
    req.end_chunks()
}

// ---------------------------------------------------------------------------
// the server

fn recovery_handler(req: &mut Request) -> EspErr {
    req.set_type(c"text/html; charset=UTF-8");
    req.send(RECOVERY_PAGE);
    ESP_OK
}

/// CORS preflight: the headers are set by the route's checks.
fn options_handler(req: &mut Request) -> EspErr {
    req.send(&[]);
    ESP_OK
}

/// Closes the connection after the response (`ConGuard`).
fn close_after(req: &mut Request) {
    let fd = req.sockfd();
    let s = server();
    if fd >= 0 && !s.is_null() {
        // SAFETY: the running server and one of its sockets
        unsafe { httpd::httpd_sess_trigger_close(s, fd) };
    }
}

/// Unknown URIs: a redirect to `/` (captive portal).
unsafe extern "C" fn not_found_handler(req: *mut Req, _: httpd::ErrCode) -> EspErr {
    let mut req = Request::from_raw(req);
    req.set_hdr(c"Connection", c"close");
    req.set_status(c"302 Temporary Redirect");
    req.set_hdr(c"Location", c"/");
    // iOS needs content to detect a captive portal
    req.send(b"Redirect to the captive portal");
    log!(Info, TAG, "Redirecting to root");
    close_after(&mut req);
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

fn checks_and_handler(req: &mut Request, rc: &RouteCtx) -> EspErr {
    let flags = rc.route.flags;
    if flags & flag::NET != 0 && !network_allowed(req) {
        return req.send_err(err::E401_UNAUTHORIZED, Some(c"Unauthorized"));
    }
    if flags & flag::CORS != 0 && !set_cors_headers(req) {
        req.send_err(err::E500_INTERNAL_SERVER_ERROR, None);
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
    let mut req = Request::from_raw(req);
    if rc.route.flags & flag::CLOSE != 0 {
        req.set_hdr(c"Connection", c"close");
        let r = checks_and_handler(&mut req, rc);
        close_after(&mut req);
        return r;
    }
    checks_and_handler(&mut req, rc)
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
    // SAFETY: `ctx` is a non-null allocation of size_of::<ServerCtx>() bytes;
    // all-zero is a valid ServerCtx (char arrays)
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
    // SAFETY: `handle` and `config` are live locals; httpd copies the config
    if unsafe { httpd::httpd_start(&mut handle, &config) } != ESP_OK {
        log!(Error, TAG, "Start server failed");
        // SAFETY: `ctx` came from c_malloc() above and was not handed out yet
        unsafe { crate::cutil::c_free(ctx as *mut c_void) };
        return ESP_FAIL;
    }
    http_server.store(handle, Ordering::Release);

    // SAFETY: the server is running; `ctx` is initialized and never freed (it
    // stays the routes' user context); the handler matches the err callback type
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
