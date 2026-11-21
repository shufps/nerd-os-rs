//! The pool connection (port of `main/stratum/stratum_transport.cpp` and
//! `stratum_transport_noise.cpp`): TCP or TLS (certificate bundle, common
//! name) over `esp_transport`, keep-alive per the settings, TCP_NODELAY, and
//! for Stratum v2 the Noise_NX handshake (`stratum_v2/noise.rs`) on top of
//! TCP.
//!
//! As before: the lock is held across the whole TCP/TLS connect (a share
//! submit never sees a half-built transport); `close()` first shuts the
//! socket down without the lock, so a writer blocked in
//! `esp_transport_write()` fails out and releases it; the Noise keys are
//! published only after the handshake. The reads (`recv()`, the Noise
//! handshake and frames) run without that lock but hold the shared `io`
//! lock, which `close()`/`connect()` take exclusively before they free the
//! handle. The Noise sending half lives under the state lock, the receiving
//! half under its own, so a share submit never waits for a frame.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Mutex, RwLock};

use crate::config;
use crate::log::log;
use crate::sdkconfig;
use crate::stratum_v2::connection::Readiness;
use crate::stratum_v2::noise::{self, EspWire, Receiver, Sender};
use crate::stratum_v2::protocol::{parse_frame_header, FrameHeader};

const TAG: &CStr = c"stratum_transport";
const NOISE_TAG: &CStr = c"noise_transport";

const CONNECT_TIMEOUT_MS: c_int = 5000;
const IO_TIMEOUT_MS: c_int = 30000;

idf_const!(TRANSPORT_TIMEOUT: c_int = esp_tcp_transport_err_t_ERR_TCP_TRANSPORT_CONNECTION_TIMEOUT, 0);
idf_const!(TRANSPORT_CLOSED_BY_FIN: c_int = esp_tcp_transport_err_t_ERR_TCP_TRANSPORT_CONNECTION_CLOSED_BY_FIN, -1);
idf_const!(
    /// errno values (newlib)
    ENOTCONN: c_int = ENOTCONN, 128
);
idf_const!(ECONNRESET: c_int = ECONNRESET, 104);
idf_const!(EAGAIN: c_int = EAGAIN, 11);
idf_const!(EIO: c_int = EIO, 5);
idf_const!(
    /// lwIP
    IPPROTO_TCP: c_int = IPPROTO_TCP, 6
);
idf_const!(TCP_NODELAY: c_int = TCP_NODELAY, 1);
idf_const!(SHUT_RDWR: c_int = SHUT_RDWR, 2);

#[cfg(target_os = "espidf")]
pub type Handle = crate::idf::raw::esp_transport_handle_t;
#[cfg(not(target_os = "espidf"))]
pub type Handle = *mut c_void;

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_transport_keep_alive_t as KeepAlive;
#[cfg(not(target_os = "espidf"))]
/// `esp_transport_keep_alive_t`
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct KeepAlive {
    pub keep_alive_enable: bool,
    pub keep_alive_idle: c_int,
    pub keep_alive_interval: c_int,
    pub keep_alive_count: c_int,
}

#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    pub use crate::idf::raw::{
        esp_transport_close, esp_transport_connect, esp_transport_destroy, esp_transport_get_errno,
        esp_transport_poll_read, esp_transport_poll_write, esp_transport_read,
        esp_transport_ssl_crt_bundle_attach, esp_transport_ssl_init,
        esp_transport_ssl_set_common_name, esp_transport_ssl_set_keep_alive,
        esp_transport_tcp_init, esp_transport_tcp_set_keep_alive, esp_transport_write,
        lwip_setsockopt, lwip_shutdown, strerror,
    };
    extern "C" {
        /// only in the private header in IDF 5.3
        pub fn esp_transport_get_socket(t: Handle) -> c_int;
    }
}

/// On the host there is no network.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_transport_tcp_init() -> Handle {
        core::ptr::null_mut()
    }
    pub unsafe fn esp_transport_ssl_init() -> Handle {
        core::ptr::null_mut()
    }
    pub unsafe fn esp_transport_ssl_crt_bundle_attach(
        _: Handle,
        _: Option<unsafe extern "C" fn(*mut c_void) -> c_int>,
    ) {
    }
    pub unsafe fn esp_transport_ssl_set_common_name(_: Handle, _: *const c_char) {}
    pub unsafe fn esp_transport_ssl_set_keep_alive(_: Handle, _: *mut KeepAlive) {}
    pub unsafe fn esp_transport_tcp_set_keep_alive(_: Handle, _: *mut KeepAlive) {}
    pub unsafe fn esp_transport_connect(_: Handle, _: *const c_char, _: c_int, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_read(_: Handle, _: *mut c_char, _: c_int, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_write(_: Handle, _: *const c_char, _: c_int, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_poll_write(_: Handle, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_poll_read(_: Handle, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_get_errno(_: Handle) -> c_int {
        0
    }
    pub unsafe fn esp_transport_get_socket(_: Handle) -> c_int {
        -1
    }
    pub unsafe fn esp_transport_close(_: Handle) -> c_int {
        0
    }
    pub unsafe fn esp_transport_destroy(_: Handle) -> c_int {
        0
    }
    pub unsafe fn lwip_setsockopt(_: c_int, _: c_int, _: c_int, _: *const c_void, _: u32) -> c_int {
        -1
    }
    pub unsafe fn lwip_shutdown(_: c_int, _: c_int) -> c_int {
        -1
    }
    pub unsafe fn strerror(_: c_int) -> *const c_char {
        c"error".as_ptr()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Tcp,
    Tls,
    /// Stratum v2: Noise over TCP
    Noise,
}

struct State {
    t: Handle,
    /// Stratum v2: the Noise sending half once the handshake is done
    noise_tx: Option<Sender>,
    /// `esp_transport_*_set_keep_alive()` keeps this pointer and reads it
    /// inside `esp_transport_connect()`: it lives as long as the transport
    keep_alive: KeepAlive,
}

pub struct Transport {
    kind: Kind,
    state: Mutex<State>,
    /// held (shared) by the reads that use the handle without the state
    /// lock, (exclusive) by close/connect before they free it
    io: RwLock<()>,
    /// Stratum v2: the Noise receiving half (used by the owner task)
    noise_rx: Mutex<Option<Receiver>>,
    /// the socket for the unlocked shutdown in `close()`
    sock: AtomicI32,
    authority: Mutex<Option<[u8; 32]>>,
    require_auth: AtomicBool,
}

// SAFETY: the raw handle is used under the state lock, or (the blocking
// reads) under the shared `io` lock, which close/connect take exclusively
// before they free it; the keep-alive pointer stays inside the locked state
unsafe impl Send for Transport {}
// SAFETY: see Send
unsafe impl Sync for Transport {}

fn err_text(e: c_int) -> String {
    // SAFETY: strerror() returns a NUL-terminated static string for any errno
    unsafe { CStr::from_ptr(sys::strerror(e)) }
        .to_string_lossy()
        .into_owned()
}

impl Transport {
    pub fn new(kind: Kind) -> Self {
        Transport {
            kind,
            state: Mutex::new(State {
                t: core::ptr::null_mut(),
                noise_tx: None,
                keep_alive: KeepAlive::default(),
            }),
            io: RwLock::new(()),
            noise_rx: Mutex::new(None),
            sock: AtomicI32::new(-1),
            authority: Mutex::new(None),
            require_auth: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn rx(&self) -> std::sync::MutexGuard<'_, Option<Receiver>> {
        self.noise_rx.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The handle for a read outside the state lock, with the guard that
    /// keeps close/connect from freeing it meanwhile.
    fn reading(&self) -> (std::sync::RwLockReadGuard<'_, ()>, Handle) {
        let g = self.io.read().unwrap_or_else(|e| e.into_inner());
        (g, self.lock().t)
    }

    fn freeing(&self) -> std::sync::RwLockWriteGuard<'_, ()> {
        self.io.write().unwrap_or_else(|e| e.into_inner())
    }

    fn close_locked(&self, s: &mut State) {
        s.noise_tx = None; // wipes the key
        if !s.t.is_null() {
            // SAFETY: a non-null handle from a successful *_init(), destroyed
            // once here under the lock and nulled right after
            unsafe {
                sys::esp_transport_close(s.t);
                sys::esp_transport_destroy(s.t);
            }
            s.t = core::ptr::null_mut();
        }
        self.sock.store(-1, Ordering::Release);
    }

    /// Makes a writer blocked inside `esp_transport_write()` fail out.
    fn shutdown_socket(&self) {
        let sock = self.sock.load(Ordering::Acquire);
        if sock >= 0 {
            // SAFETY: plain syscall on an fd number; a stale fd only makes it fail
            unsafe { sys::lwip_shutdown(sock, SHUT_RDWR) };
        }
    }

    pub fn close(&self) {
        // a read blocked on the socket fails out, so the guard comes quickly
        self.shutdown_socket();
        // never together with the state lock (recv_frame() takes them in turn)
        *self.rx() = None;
        let _io = self.freeing();
        let mut s = self.lock();
        self.close_locked(&mut s);
    }

    /// The TCP/TLS connection (under the lock throughout).
    fn connect_plain(&self, host: &CStr, ip: Option<&CStr>, port: u16) -> bool {
        self.shutdown_socket();
        let _io = self.freeing();
        let mut s = self.lock();
        self.close_locked(&mut s);
        let tls = self.kind == Kind::Tls;
        // SAFETY: the handle is null-checked before use; `host` is a valid C
        // string (the transport copies the common name)
        let t = unsafe {
            if tls {
                let t = sys::esp_transport_ssl_init();
                if t.is_null() {
                    log!(Error, TAG, "esp_transport_ssl_init failed");
                    return false;
                }
                sys::esp_transport_ssl_crt_bundle_attach(t, Some(crate::crt_bundle::attach));
                sys::esp_transport_ssl_set_common_name(t, host.as_ptr());
                t
            } else {
                let t = sys::esp_transport_tcp_init();
                if t.is_null() {
                    log!(Error, TAG, "esp_transport_tcp_init failed");
                    return false;
                }
                t
            }
        };
        s.t = t;
        let enable = config::get_u16(
            b"stratum_keep",
            sdkconfig::STRATUM_KEEPALIVE_ENABLE_VALUE as u16,
        ) != 0;
        s.keep_alive = KeepAlive {
            keep_alive_enable: enable,
            keep_alive_idle: 10,
            keep_alive_interval: 5,
            keep_alive_count: 3,
        };
        if enable {
            let ka: *mut KeepAlive = &mut s.keep_alive;
            // SAFETY: `t` is the fresh non-null handle; `ka` points into the
            // locked state, which lives as long as the transport
            unsafe {
                if tls {
                    sys::esp_transport_ssl_set_keep_alive(t, ka);
                } else {
                    sys::esp_transport_tcp_set_keep_alive(t, ka);
                }
            }
        }
        // Connect to the resolved address. TLS still checks the certificate
        // against host (set as the common name above).
        let target = ip.unwrap_or(host);
        log!(
            Info,
            TAG,
            "Connecting ({}) to {}:{port}",
            if tls { "TLS" } else { "TCP" },
            target.to_string_lossy()
        );
        // SAFETY: `t` is the non-null handle created above; `target` is a valid C string
        if unsafe {
            sys::esp_transport_connect(t, target.as_ptr(), port as c_int, CONNECT_TIMEOUT_MS)
        } != 0
        {
            // SAFETY: `t` is still the valid handle (closed only below)
            let e = unsafe { sys::esp_transport_get_errno(t) };
            log!(
                Error,
                TAG,
                "esp_transport_connect failed, errno={e} ({})",
                err_text(e)
            );
            self.close_locked(&mut s);
            return false;
        }
        // Nagle off: share submits are latency-sensitive, and the Noise
        // transport writes header and payload as separate segments
        // SAFETY: `t` is the connected handle, owned under the held lock
        let sock = unsafe { sys::esp_transport_get_socket(t) };
        if sock < 0 {
            log!(Warn, TAG, "Could not get socket fd to set TCP_NODELAY");
        } else {
            self.sock.store(sock, Ordering::Release);
            let one: c_int = 1;
            // SAFETY: `one` is a live c_int and the length passed is its size
            let r = unsafe {
                sys::lwip_setsockopt(
                    sock,
                    IPPROTO_TCP,
                    TCP_NODELAY,
                    &one as *const c_int as *const c_void,
                    core::mem::size_of::<c_int>() as u32,
                )
            };
            if r < 0 {
                let e = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
                log!(
                    Warn,
                    TAG,
                    "Failed to set TCP_NODELAY (errno={e}: {})",
                    err_text(e)
                );
            } else {
                log!(Info, TAG, "TCP_NODELAY enabled");
            }
        }
        log!(Info, TAG, "Connected");
        true
    }

    /// Connects (and for Stratum v2 runs the Noise handshake).
    pub fn connect(&self, host: &CStr, ip: Option<&CStr>, port: u16) -> bool {
        if self.kind != Kind::Noise {
            return self.connect_plain(host, ip, port);
        }
        let authority = *self.authority.lock().unwrap_or_else(|e| e.into_inner());
        // refuse rather than fall back to an encrypted but unverified channel
        if self.require_auth.load(Ordering::Acquire) && authority.is_none() {
            log!(
                Error,
                NOISE_TAG,
                "SV2 authentication required but no authority pubkey configured; refusing to connect"
            );
            return false;
        }
        *self.rx() = None;
        if !self.connect_plain(host, ip, port) {
            return false;
        }
        log!(Info, NOISE_TAG, "Starting Noise handshake");
        // with a synced clock, the certificate's validity period is checked too
        let now = crate::clock::is_time_synced().then(crate::clock::now);
        let cipher = {
            // no state lock during the handshake: the read guard keeps the
            // handle alive
            let (_io, t) = self.reading();
            if t.is_null() {
                return false;
            }
            // SAFETY: the handle just connected; close/connect wait for the guard
            let mut wire = unsafe { EspWire::new(t.cast()) };
            noise::handshake(&mut wire, authority.as_ref(), now)
        };
        let Some(cipher) = cipher else {
            log!(Error, NOISE_TAG, "Noise handshake failed");
            self.close();
            return false;
        };
        *self.rx() = Some(cipher.recv);
        self.lock().noise_tx = Some(cipher.send);
        log!(
            Info,
            NOISE_TAG,
            "Encrypted channel established (ChaCha20-Poly1305)"
        );
        true
    }

    /// Bytes sent, or a negative errno.
    pub fn send(&self, data: &[u8]) -> c_int {
        let mut s = self.lock();
        if self.kind == Kind::Noise {
            let s = &mut *s;
            let (Some(tx), false) = (s.noise_tx.as_mut(), s.t.is_null()) else {
                return -EIO;
            };
            // SAFETY: a connected transport; close() needs the lock held here
            let mut wire = unsafe { EspWire::new(s.t.cast()) };
            return if tx.send(&mut wire, data) {
                data.len() as c_int
            } else {
                -EIO
            };
        }
        if s.t.is_null() {
            return -ENOTCONN;
        }
        // SAFETY: non-null handle used under the lock; pointer and length come
        // from the same slice
        let r = unsafe {
            sys::esp_transport_write(
                s.t,
                data.as_ptr() as *const c_char,
                data.len() as c_int,
                IO_TIMEOUT_MS,
            )
        };
        if r < 0 {
            // SAFETY: the same non-null handle, lock still held
            let e = unsafe { sys::esp_transport_get_errno(s.t) };
            let e = if e > 0 { e } else { ECONNRESET };
            log!(
                Warn,
                TAG,
                "write failed ret={r} errno={e} ({})",
                err_text(e)
            );
            return -e;
        }
        if r == 0 && !data.is_empty() {
            return -EAGAIN;
        }
        r
    }

    /// Bytes received, 0 when the peer closed, or a negative errno.
    pub fn recv(&self, buf: &mut [u8]) -> c_int {
        if self.kind == Kind::Noise {
            // SV2 reads whole frames (recv_frame())
            log!(
                Warn,
                NOISE_TAG,
                "Generic recv() not used for SV2 — use recv_frame()"
            );
            return -EIO;
        }
        let (_io, t) = self.reading();
        if t.is_null() {
            return -ENOTCONN;
        }
        // SAFETY: a connected handle that close/connect free only after the
        // read guard is gone; pointer and length come from the same slice
        let r = unsafe {
            sys::esp_transport_read(
                t,
                buf.as_mut_ptr() as *mut c_char,
                buf.len() as c_int,
                IO_TIMEOUT_MS,
            )
        };
        if r > 0 {
            return r;
        }
        if r == TRANSPORT_TIMEOUT {
            return -EAGAIN;
        }
        if r == TRANSPORT_CLOSED_BY_FIN {
            return 0;
        }
        // SAFETY: as above, the handle is still open
        let e = unsafe { sys::esp_transport_get_errno(t) };
        let e = if e > 0 { e } else { ECONNRESET };
        log!(Warn, TAG, "read failed ret={r} errno={e} ({})", err_text(e));
        -e
    }

    pub fn is_connected(&self) -> bool {
        let s = self.lock();
        // SAFETY: only called on a non-null handle, under the lock
        !s.t.is_null() && unsafe { sys::esp_transport_poll_write(s.t, 0) } >= 0
    }

    pub fn set_authority(&self, key: Option<[u8; 32]>) {
        *self.authority.lock().unwrap_or_else(|e| e.into_inner()) = key;
    }

    pub fn set_require_auth(&self, require: bool) {
        self.require_auth.store(require, Ordering::Release);
    }

    /// Stratum v2: whether the Noise channel is up.
    pub fn is_secured(&self) -> bool {
        let s = self.lock();
        s.noise_tx.is_some() && !s.t.is_null()
    }

    /// Stratum v2: waits up to `ms` for data of the next frame.
    pub fn poll_read(&self, ms: u32) -> Readiness {
        let (_io, t) = self.reading();
        if t.is_null() || self.rx().is_none() {
            log!(Error, NOISE_TAG, "Noise context or transport lost");
            return Readiness::Error;
        }
        // SAFETY: a connected handle, kept alive by the read guard
        match unsafe { sys::esp_transport_poll_read(t, ms as c_int) } {
            r if r > 0 => Readiness::Readable,
            0 => Readiness::Timeout,
            _ => Readiness::Error,
        }
    }

    /// Stratum v2: receives and decrypts the next frame, its payload into
    /// `buf`. Owner task only, like `recv()`.
    pub fn recv_frame(&self, buf: &mut [u8]) -> Option<FrameHeader> {
        let (_io, t) = self.reading();
        let mut rx = self.rx();
        let (Some(rx), false) = (rx.as_mut(), t.is_null()) else {
            log!(Error, NOISE_TAG, "Noise context or transport lost");
            return None;
        };
        // SAFETY: a connected handle, kept alive by the read guard
        let mut wire = unsafe { EspWire::new(t.cast()) };
        rx.recv(&mut wire, buf).map(|h| parse_frame_header(&h))
    }
}

impl Drop for Transport {
    fn drop(&mut self) {
        self.close();
    }
}
