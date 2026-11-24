//! The log over the web socket (port of `main/http_server/http_websocket.cpp`):
//! after the handshake on `/api/ws` every log line also goes to that socket
//! (one client at a time, the newest), sent by a task from a queue of 128
//! lines (full: dropped). The `vprintf` hook that formats the lines is C
//! (`ws_log.c`: Rust cannot take a `va_list`).

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Mutex, OnceLock};

use crate::idf::httpd::{self, Request};
use crate::idf::{EspErr, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"http_websocket";
/// `MESSAGE_QUEUE_SIZE`
const QUEUE_SIZE: usize = 128;
idf_const!(WS_TEXT: u32 = httpd_ws_type_t_HTTPD_WS_TYPE_TEXT, 1);
idf_const!(WS_CLOSE: u32 = httpd_ws_type_t_HTTPD_WS_TYPE_CLOSE, 8);
idf_const!(WS_CLIENT_WEBSOCKET: u32 = httpd_ws_client_info_t_HTTPD_WS_CLIENT_WEBSOCKET, 2);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::httpd_ws_frame_t as WsFrame;
#[cfg(not(target_os = "espidf"))]
/// `httpd_ws_frame_t` (host model)
#[repr(C)]
pub struct WsFrame {
    pub final_: bool,
    pub fragmented: bool,
    pub type_: u32,
    pub payload: *mut u8,
    pub len: usize,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        httpd_ws_get_fd_info, httpd_ws_recv_frame, httpd_ws_send_frame_async,
    };
    extern "C" {
        pub fn ws_log_install();
        pub fn ws_log_uninstall();
    }
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn httpd_ws_recv_frame(_: *mut httpd::Req, _: *mut WsFrame, _: usize) -> EspErr {
        crate::idf::ESP_FAIL
    }
    pub unsafe fn httpd_ws_send_frame_async(_: httpd::Handle, _: c_int, _: *mut WsFrame) -> EspErr {
        crate::idf::ESP_FAIL
    }
    pub unsafe fn httpd_ws_get_fd_info(_: httpd::Handle, _: c_int) -> u32 {
        0
    }
    pub unsafe fn ws_log_install() {}
    pub unsafe fn ws_log_uninstall() {}
}

/// The log socket, -1: none (written by the httpd task, read by the log
/// task).
static FD: AtomicI32 = AtomicI32::new(-1);

/// A log line (with its newline).
struct Line(Vec<u8>);

static TX: OnceLock<SyncSender<Line>> = OnceLock::new();

/// Stops the forwarding if `fd` is (still) the log socket: a stale reset
/// must not stop the forwarding to a newer one.
fn reset_fd(fd: c_int) {
    if fd >= 0
        && FD
            .compare_exchange(fd, -1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        // the normal log output again
        // SAFETY: restores the previous C vprintf hook; no arguments
        unsafe { sys::ws_log_uninstall() };
    }
}

/// The httpd close callback.
pub fn on_close(fd: c_int) {
    if fd == FD.load(Ordering::Acquire) {
        log!(Info, TAG, "resetting websocket {fd}");
    }
    reset_fd(fd);
}

/// The `/api/ws` handler: the handshake starts the forwarding, a CLOSE
/// frame stops it.
pub fn handler(req: &mut Request) -> EspErr {
    if req.method() as httpd::Method == httpd::method::GET {
        log!(Info, TAG, "Handshake done, the new connection was opened");
        // SAFETY: installs the C vprintf hook
        unsafe { sys::ws_log_install() };
        FD.store(req.sockfd(), Ordering::Release);
        return ESP_OK;
    }
    // the header only (opcode, length)
    // SAFETY: an all-zero frame is valid (no payload)
    let mut pkt: WsFrame = unsafe { core::mem::zeroed() };
    // SAFETY: the live request; max_len 0: no payload is written
    let ret = unsafe { sys::httpd_ws_recv_frame(req.raw(), &mut pkt, 0) };
    if ret != ESP_OK {
        return ret;
    }
    if pkt.type_ == WS_CLOSE {
        log!(Info, TAG, "WebSocket closed, disabling log forwarding");
        reset_fd(req.sockfd());
    }
    ESP_OK
}

fn send(line: &mut [u8]) {
    // SAFETY: an all-zero frame is valid (no payload)
    let mut pkt: WsFrame = unsafe { core::mem::zeroed() };
    pkt.payload = line.as_mut_ptr();
    pkt.len = line.len();
    pkt.type_ = WS_TEXT;
    // the socket number is reused after a close: send only while it is a
    // web socket (the close callback resets FD before)
    let fd = FD.load(Ordering::Acquire);
    let server = crate::http::server::HTTP_SERVER.load(Ordering::Acquire);
    if server.is_null() || fd < 0 {
        return;
    }
    // SAFETY: the running server's handle; `pkt` points to `line`, which
    // outlives the (synchronous) send
    let failed = unsafe {
        sys::httpd_ws_get_fd_info(server, fd) != WS_CLIENT_WEBSOCKET
            || sys::httpd_ws_send_frame_async(server, fd, &mut pkt) != ESP_OK
    };
    if failed {
        // dead, or the socket was reused
        reset_fd(fd);
    }
}

unsafe extern "C" fn task(_: *mut c_void) {
    let Some(rx) = task_rx().lock().unwrap_or_else(|e| e.into_inner()).take() else {
        return;
    };
    for Line(mut line) in rx.iter() {
        if FD.load(Ordering::Acquire) < 0 {
            // no socket: dropped
            std::thread::sleep(std::time::Duration::from_millis(100));
            continue;
        }
        send(&mut line);
    }
}

/// The queue and the sending task.
pub fn start() {
    let (tx, rx) = sync_channel(QUEUE_SIZE);
    let _ = TX.set(tx);
    // the task takes the receiver
    *task_rx().lock().unwrap_or_else(|e| e.into_inner()) = Some(rx);
    crate::idf::task::spawn_psram(task, c"websocket_log_handler", 4096, 0, 2);
}

/// The receiver until the task takes it.
fn task_rx() -> &'static Mutex<Option<Receiver<Line>>> {
    static RX: Mutex<Option<Receiver<Line>>> = Mutex::new(None);
    &RX
}

pub(crate) mod api {
    use super::*;

    /// A formatted log line for the socket (copied; the caller keeps its
    /// buffer); false if the queue is full or not there. Must not log.
    ///
    /// # Safety
    /// `line` holds `len` bytes.
    #[no_mangle]
    pub unsafe extern "C" fn ws_log_enqueue(line: *const c_char, len: usize) -> bool {
        let Some(tx) = TX.get() else {
            return false;
        };
        // SAFETY: `len` bytes (caller contract), copied at once
        let bytes = unsafe { core::slice::from_raw_parts(line as *const u8, len) };
        tx.try_send(Line(bytes.to_vec())).is_ok()
    }
}
