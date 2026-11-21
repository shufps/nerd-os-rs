//! The Stratum v1 connection loop (port of `StratumTaskV1::protocolLoop()`
//! and the send/receive loops of `StratumApi`): setup requests, receiving
//! lines, handing them to the manager. The socket is reached through [`Io`];
//! the task's flags and the manager through [`Hooks`]. C API (send, submit,
//! receive; for the reference tests): `stratum_v1.h`.
//!
//! Behaviour notes:
//! * a stop or reconnect request is noticed on every receive timeout, not
//!   only when the pool sends a line,
//! * a send gives up after [`MAX_SEND_STALLS`] attempts without progress.

use super::lines::LineBuffer;
use super::session::Session;
use crate::log::log;

const TAG: &core::ffi::CStr = c"stratum_api";

/// Consecutive sends without progress (each a transport timeout of 30 s on
/// the device) before a send fails.
pub const MAX_SEND_STALLS: u32 = 3;
/// Pause between retries, as in the C++ code.
const RETRY_DELAY_MS: u32 = 10;
/// EAGAIN (= EWOULDBLOCK) on newlib and Linux
const EAGAIN: i32 = 11;

/// Result of one transport call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transfer {
    Bytes(usize),
    /// recv: the peer closed the connection; send: no progress
    Zero,
    /// a negative errno
    Error(i32),
}

impl Transfer {
    pub fn from_raw(r: i32) -> Self {
        match r {
            r if r > 0 => Transfer::Bytes(r as usize),
            0 => Transfer::Zero,
            r => Transfer::Error(-r),
        }
    }
    fn retryable(self) -> bool {
        matches!(self, Transfer::Zero | Transfer::Error(EAGAIN))
    }
}

/// The socket.
pub trait Io {
    fn send(&mut self, data: &[u8]) -> Transfer;
    fn recv(&mut self, buf: &mut [u8]) -> Transfer;
    fn is_connected(&mut self) -> bool;
    fn delay_ms(&mut self, ms: u32);
}

/// The task and the manager.
pub trait Hooks {
    /// stop flag or power shutdown
    fn stop_requested(&mut self) -> bool;
    fn reconnect_requested(&mut self) -> bool;
    /// for the log message when the socket drops
    fn keepalive_enabled(&mut self) -> bool;
    /// the first valid line arrived: the connection works
    fn connected(&mut self);
    fn dispatch(&mut self, line: &[u8]);
    /// the manager found the connection unusable (bad subscribe response)
    fn setup_failed(&mut self) -> bool;
}

/// The request as logged: the password of `mining.authorize` as `***`
/// (the log can be read over the web socket without OTP).
fn loggable(data: &[u8]) -> Vec<u8> {
    let shown = data.strip_suffix(b"\n").unwrap_or(data);
    let Ok(doc) = crate::ajson::deserialize(shown) else {
        return shown.to_vec();
    };
    if doc.get("method").as_cstr() != Some(b"mining.authorize") {
        return shown.to_vec();
    }
    let id = doc.get("id").as_int().unwrap_or(0);
    let user = doc.get("params").at(0).as_cstr().unwrap_or_default();
    let masked = super::request::authorize(id, user, b"***");
    masked.strip_suffix(b"\n").unwrap_or(&masked).to_vec()
}

/// Sends all of `data`; false on an error or after too many stalls.
pub fn send_all(io: &mut impl Io, data: &[u8]) -> bool {
    log!(
        Info,
        TAG,
        "tx: {}",
        String::from_utf8_lossy(&loggable(data))
    );
    if !io.is_connected() {
        log!(Info, TAG, "Socket not connected. Cannot send message.");
        return false;
    }
    let mut rest = data;
    let mut stalls = 0;
    while !rest.is_empty() {
        match io.send(rest) {
            Transfer::Bytes(n) => {
                rest = &rest[n.min(rest.len())..];
                stalls = 0;
            }
            t if t.retryable() => {
                stalls += 1;
                if stalls >= MAX_SEND_STALLS {
                    log!(
                        Error,
                        TAG,
                        "Socket not writable for {stalls} attempts, giving up"
                    );
                    return false;
                }
                io.delay_ms(RETRY_DELAY_MS);
            }
            Transfer::Error(e) => {
                log!(Error, TAG, "Error writing to socket: errno {e}");
                return false;
            }
            _ => unreachable!(),
        }
    }
    true
}

/// Why receiving a line ended without one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecvEnd {
    /// error, peer closed, socket gone or buffer full (buffer cleared)
    Failed,
    /// `interrupted()` returned true during a receive timeout
    Interrupted,
}

/// The next line, reading from the socket as needed (port of
/// `StratumApi::receiveJsonRpcLine`). `interrupted` is asked on every
/// receive timeout.
pub fn receive_line(
    io: &mut impl Io,
    lines: &mut LineBuffer,
    mut interrupted: impl FnMut() -> bool,
) -> Result<Vec<u8>, RecvEnd> {
    loop {
        if let Some(line) = lines.take_line() {
            return Ok(line);
        }
        let space = lines.space();
        if space.is_empty() {
            log!(Error, TAG, "Buffer full without newline. Flushing buffer.");
            lines.clear();
            return Err(RecvEnd::Failed);
        }
        match io.recv(space) {
            Transfer::Bytes(n) => lines.commit(n),
            Transfer::Zero => {
                log!(Info, TAG, "Remote closed the connection.");
                lines.clear();
                return Err(RecvEnd::Failed);
            }
            Transfer::Error(EAGAIN) => {
                if !io.is_connected() {
                    log!(Error, TAG, "Socket is not connected anymore.");
                    lines.clear();
                    return Err(RecvEnd::Failed);
                }
                if interrupted() {
                    return Err(RecvEnd::Interrupted);
                }
                io.delay_ms(RETRY_DELAY_MS);
            }
            Transfer::Error(e) => {
                log!(Error, TAG, "Error in recv: errno {e}");
                lines.clear();
                return Err(RecvEnd::Failed);
            }
        }
    }
}

/// What the setup sends.
pub struct Setup<'a> {
    pub device: &'a [u8],
    pub asic: &'a [u8],
    pub version: &'a [u8],
    pub user: &'a [u8],
    pub password: &'a [u8],
    pub difficulty: u32,
    pub extranonce_subscribe: bool,
}

/// Why the connection loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    SetupSendFailed,
    SocketDisconnected,
    ReceiveFailed,
    ReconnectRequested,
    InvalidJson,
    StopRequested,
    SetupFailed,
}

/// Requests longer than this were rejected by the C++ request buffer.
const REQUEST_LIMIT: usize = 1023;

fn send_request(io: &mut impl Io, request: Vec<u8>) -> bool {
    if request.len() > REQUEST_LIMIT {
        log!(
            Error,
            TAG,
            "Stratum request does not fit into {} bytes, not sent.",
            REQUEST_LIMIT + 1
        );
        return false;
    }
    send_all(io, &request)
}

/// Formats a request in the session and sends it; a request that is not sent
/// for being too long is not left pending.
pub fn send_tracked(
    io: &mut impl Io,
    session: &std::sync::Mutex<Session>,
    request: impl FnOnce(&mut Session) -> Vec<u8>,
) -> bool {
    let bytes = {
        let mut s = session.lock().unwrap_or_else(|e| e.into_inner());
        let bytes = request(&mut s);
        if bytes.len() > REQUEST_LIMIT {
            s.untrack_last();
        }
        bytes
    };
    // not under the lock: the connection task parses while a share is sent
    send_request(io, bytes)
}

/// The connection loop: sends the setup requests, then hands every line to
/// the manager until the connection ends.
pub fn run(
    io: &mut impl Io,
    hooks: &mut impl Hooks,
    setup: &Setup,
    session: &std::sync::Mutex<Session>,
    lines: &mut LineBuffer,
) -> Exit {
    let lock = || session.lock().unwrap_or_else(|e| e.into_inner());
    lock().reset();
    lines.clear();

    // subscribe (ID 1), configure (2), authorize (3), suggest_difficulty (4),
    // optionally extranonce.subscribe (5); each formatted right before sending
    let send =
        |io: &mut _, request: &dyn Fn(&mut Session) -> Vec<u8>| send_tracked(io, session, request);
    let sent = send(io, &|s| {
        s.subscribe(setup.device, setup.asic, setup.version)
    }) && send(io, &|s| s.configure_version_rolling())
        && send(io, &|s| s.authorize(setup.user, setup.password))
        && send(io, &|s| s.suggest_difficulty(setup.difficulty))
        && (!setup.extranonce_subscribe || send(io, &|s| s.extranonce_subscribe()));
    if !sent {
        log!(Error, TAG, "Error sending Stratum setup commands!");
        return Exit::SetupSendFailed;
    }

    let mut connected = false;
    loop {
        if !io.is_connected() {
            if hooks.keepalive_enabled() {
                log!(
                    Warn,
                    TAG,
                    "Socket disconnected — possible TCP KeepAlive timeout (enabled)"
                );
            } else {
                log!(Warn, TAG, "Socket disconnected — no KeepAlive active");
            }
            return Exit::SocketDisconnected;
        }

        let line = receive_line(io, lines, || {
            hooks.reconnect_requested() || hooks.stop_requested()
        });
        if hooks.reconnect_requested() {
            log!(Info, TAG, "reconnect requested ...");
            return Exit::ReconnectRequested;
        }
        let line = match line {
            Ok(line) => line,
            Err(RecvEnd::Interrupted) => return Exit::StopRequested,
            Err(RecvEnd::Failed) => {
                log!(
                    Error,
                    TAG,
                    "Failed to receive JSON-RPC line, reconnecting ..."
                );
                return Exit::ReceiveFailed;
            }
        };
        log!(Info, TAG, "rx: {}", String::from_utf8_lossy(&line));

        // valid JSON before the connected callback runs
        if !super::parse::is_json(&line) {
            log!(Error, TAG, "Unable to parse JSON");
            return Exit::InvalidJson;
        }
        if !connected {
            hooks.connected();
            connected = true;
        }
        if hooks.stop_requested() {
            return Exit::StopRequested;
        }
        hooks.dispatch(&line);
        // without a valid subscribe result there is no extranonce to build
        // jobs with: treat it as a pool error instead of mining on
        if hooks.setup_failed() {
            return Exit::SetupFailed;
        }
    }
}

mod ffi {
    use super::*;
    use core::ffi::{c_char, c_int, c_void, CStr};

    /// `stratum_io_t`: the C++ transport.
    #[repr(C)]
    pub struct CIo {
        pub ctx: *mut c_void,
        /// bytes sent, 0 for no progress, or a negative errno
        pub send: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> c_int,
        /// bytes received, 0 when the peer closed, or a negative errno
        pub recv: unsafe extern "C" fn(*mut c_void, *mut u8, usize) -> c_int,
        pub is_connected: unsafe extern "C" fn(*mut c_void) -> bool,
        pub delay_ms: unsafe extern "C" fn(*mut c_void, u32),
    }

    impl Io for &CIo {
        fn send(&mut self, data: &[u8]) -> Transfer {
            // SAFETY: the C++ side provides valid callbacks for its context.
            Transfer::from_raw(unsafe { (self.send)(self.ctx, data.as_ptr(), data.len()) })
        }
        fn recv(&mut self, buf: &mut [u8]) -> Transfer {
            // SAFETY: as above; writes at most buf.len() bytes.
            Transfer::from_raw(unsafe { (self.recv)(self.ctx, buf.as_mut_ptr(), buf.len()) })
        }
        fn is_connected(&mut self) -> bool {
            // SAFETY: as above.
            unsafe { (self.is_connected)(self.ctx) }
        }
        fn delay_ms(&mut self, ms: u32) {
            // SAFETY: as above.
            unsafe { (self.delay_ms)(self.ctx, ms) }
        }
    }

    unsafe fn bytes<'a>(p: *const c_char) -> &'a [u8] {
        if p.is_null() {
            &[]
        } else {
            CStr::from_ptr(p).to_bytes()
        }
    }

    /// # Safety
    /// `io` valid, `data` holds `len` bytes.
    #[no_mangle]
    pub unsafe extern "C" fn stratum_v1_send_all(
        io: *const CIo,
        data: *const u8,
        len: usize,
    ) -> bool {
        let data = if len == 0 {
            &[][..]
        } else {
            core::slice::from_raw_parts(data, len)
        };
        send_all(&mut &*io, data)
    }

    /// Formats and sends a `mining.submit` (port of
    /// `StratumApi::submitShare()`); `version` is the rolled version delta.
    ///
    /// # Safety
    /// `io` valid, `session` from `stratum_v1_session_new()`, strings
    /// NUL-terminated or NULL.
    #[no_mangle]
    #[allow(clippy::too_many_arguments)]
    pub unsafe extern "C" fn stratum_v1_submit(
        io: *const CIo,
        session: *const super::super::session::ffi::Handle,
        user: *const c_char,
        job_id: *const c_char,
        extranonce2: *const c_char,
        ntime: u32,
        nonce: u32,
        version: u32,
    ) -> bool {
        send_tracked(&mut &*io, &(*session).0, |s| {
            s.submit(
                bytes(user),
                bytes(job_id),
                bytes(extranonce2),
                ntime,
                nonce,
                version,
            )
        })
    }

    /// The next line as a `malloc()`ed string the caller frees, or NULL.
    ///
    /// # Safety
    /// `io` valid, `lines` from `stratum_v1_lines_new()`.
    #[no_mangle]
    pub unsafe extern "C" fn stratum_v1_receive_line(
        io: *const CIo,
        lines: *mut LineBuffer,
    ) -> *mut c_char {
        match receive_line(&mut &*io, &mut *lines, || false) {
            Ok(line) => {
                let p = crate::cutil::c_strdup(&line);
                if p.is_null() {
                    (*lines).clear();
                }
                p
            }
            Err(_) => core::ptr::null_mut(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted socket: recv returns the queued results in order, then
    /// timeouts; send records what it accepts.
    #[derive(Default)]
    struct FakeIo {
        incoming: VecDeque<Transfer>,
        data: VecDeque<u8>,
        sent: Vec<u8>,
        send_script: VecDeque<Transfer>,
        disconnected_after_timeouts: Option<usize>,
        timeouts: usize,
    }

    impl FakeIo {
        fn feed(&mut self, bytes: &[u8]) {
            self.data.extend(bytes);
            self.incoming.push_back(Transfer::Bytes(bytes.len()));
        }
    }

    impl Io for FakeIo {
        fn send(&mut self, data: &[u8]) -> Transfer {
            let t = self
                .send_script
                .pop_front()
                .unwrap_or(Transfer::Bytes(data.len()));
            if let Transfer::Bytes(n) = t {
                self.sent.extend(&data[..n.min(data.len())]);
            }
            t
        }
        fn recv(&mut self, buf: &mut [u8]) -> Transfer {
            match self.incoming.pop_front() {
                Some(Transfer::Bytes(n)) => {
                    let n = n.min(buf.len());
                    for b in &mut buf[..n] {
                        *b = self.data.pop_front().unwrap();
                    }
                    Transfer::Bytes(n)
                }
                Some(t) => t,
                None => {
                    self.timeouts += 1;
                    Transfer::Error(EAGAIN)
                }
            }
        }
        fn is_connected(&mut self) -> bool {
            self.disconnected_after_timeouts
                .map_or(true, |n| self.timeouts < n)
        }
        fn delay_ms(&mut self, _ms: u32) {}
    }

    #[derive(Default)]
    struct FakeHooks {
        dispatched: Vec<Vec<u8>>,
        connected: usize,
        reconnect_after: Option<usize>,
        stop: bool,
        fail_setup_after: Option<usize>,
        polls: usize,
    }

    impl Hooks for FakeHooks {
        fn stop_requested(&mut self) -> bool {
            self.stop
        }
        fn reconnect_requested(&mut self) -> bool {
            self.polls += 1;
            self.reconnect_after.is_some_and(|n| self.polls > n)
        }
        fn keepalive_enabled(&mut self) -> bool {
            false
        }
        fn connected(&mut self) {
            self.connected += 1;
        }
        fn dispatch(&mut self, line: &[u8]) {
            self.dispatched.push(line.to_vec());
        }
        fn setup_failed(&mut self) -> bool {
            self.fail_setup_after
                .is_some_and(|n| self.dispatched.len() >= n)
        }
    }

    fn setup(extranonce_subscribe: bool) -> Setup<'static> {
        Setup {
            device: b"NerdQAxe+",
            asic: b"BM1370",
            version: b"rs-test",
            user: b"bc1q.worker",
            password: b"x",
            difficulty: 1000,
            extranonce_subscribe,
        }
    }

    fn run_with(io: &mut FakeIo, hooks: &mut FakeHooks, extranonce_subscribe: bool) -> Exit {
        let session = std::sync::Mutex::new(Session::default());
        let mut lines = LineBuffer::default();
        run(
            io,
            hooks,
            &setup(extranonce_subscribe),
            &session,
            &mut lines,
        )
    }

    #[test]
    fn setup_requests_then_lines() {
        let mut io = FakeIo {
            disconnected_after_timeouts: Some(1),
            ..Default::default()
        };
        io.feed(b"{\"id\":1,\"result\":[[],\"0800\",4],\"error\":null}\n{\"id\":null,\"method\":\"mining.set_difficulty\",\"params\":[512]}\r\n");
        let mut hooks = FakeHooks::default();
        assert_eq!(run_with(&mut io, &mut hooks, true), Exit::ReceiveFailed);

        let sent = String::from_utf8(io.sent).unwrap();
        let methods: Vec<&str> = sent.lines().map(|l| l.split('"').nth(5).unwrap()).collect();
        assert_eq!(
            methods,
            [
                "mining.subscribe",
                "mining.configure",
                "mining.authorize",
                "mining.suggest_difficulty",
                "mining.extranonce.subscribe"
            ]
        );
        assert!(sent.starts_with("{\"id\": 1, \"method\": \"mining.subscribe\", \"params\": [\"NerdQAxe+/BM1370/rs-test\"]}\n"));
        assert_eq!((hooks.connected, hooks.dispatched.len()), (1, 2));
    }

    #[test]
    fn invalid_json_ends_before_connected() {
        let mut io = FakeIo::default();
        io.feed(b"not json\n");
        let mut hooks = FakeHooks::default();
        assert_eq!(run_with(&mut io, &mut hooks, false), Exit::InvalidJson);
        assert_eq!((hooks.connected, hooks.dispatched.len()), (0, 0));
    }

    /// A reconnect request is noticed while the pool is quiet.
    #[test]
    fn reconnect_while_idle() {
        let mut io = FakeIo::default();
        let mut hooks = FakeHooks {
            reconnect_after: Some(5),
            ..Default::default()
        };
        assert_eq!(
            run_with(&mut io, &mut hooks, false),
            Exit::ReconnectRequested
        );
        assert!(io.timeouts >= 1);
    }

    #[test]
    fn stop_while_idle_and_setup_failure() {
        let mut io = FakeIo::default();
        let mut hooks = FakeHooks {
            stop: true,
            ..Default::default()
        };
        assert_eq!(run_with(&mut io, &mut hooks, false), Exit::StopRequested);

        let mut io = FakeIo::default();
        io.feed(b"{\"id\":1,\"result\":[[],\"0800\",99],\"error\":null}\n");
        let mut hooks = FakeHooks {
            fail_setup_after: Some(1),
            ..Default::default()
        };
        assert_eq!(run_with(&mut io, &mut hooks, false), Exit::SetupFailed);
    }

    #[test]
    fn peer_close_and_full_buffer() {
        let mut io = FakeIo::default();
        io.incoming.push_back(Transfer::Zero);
        assert_eq!(
            run_with(&mut io, &mut FakeHooks::default(), false),
            Exit::ReceiveFailed
        );

        let mut io = FakeIo::default();
        io.feed(&vec![b'x'; super::super::lines::CAPACITY]);
        assert_eq!(
            run_with(&mut io, &mut FakeHooks::default(), false),
            Exit::ReceiveFailed
        );
    }

    #[test]
    fn send_retries_partial_writes_and_gives_up_on_stalls() {
        let mut io = FakeIo::default();
        io.send_script.extend([
            Transfer::Bytes(3),
            Transfer::Error(EAGAIN),
            Transfer::Bytes(2),
            Transfer::Zero,
        ]);
        assert!(send_all(&mut io, b"0123456789"));
        assert_eq!(io.sent, b"0123456789");

        // No endless retry.
        let mut io = FakeIo::default();
        io.send_script
            .extend([Transfer::Error(EAGAIN); MAX_SEND_STALLS as usize]);
        assert!(!send_all(&mut io, b"abc"));
        let mut io = FakeIo::default();
        io.send_script.push_back(Transfer::Error(104));
        assert!(!send_all(&mut io, b"abc"));
    }

    #[test]
    fn oversized_request_is_neither_sent_nor_pending() {
        let session = std::sync::Mutex::new(Session::default());
        let mut io = FakeIo::default();
        let long = vec![b'x'; 2000];
        assert!(!send_tracked(&mut io, &session, |s| {
            s.submit(&long, b"j", b"00", 1, 2, 3)
        }));
        assert!(io.sent.is_empty());
        // the ID was given back
        assert!(send_tracked(&mut io, &session, |s| {
            s.submit(b"u", b"j", b"00", 1, 2, 3)
        }));
        assert!(io.sent.starts_with(b"{\"id\": 1,"));
    }

    #[test]
    fn authorize_password_is_not_logged() {
        let req = super::super::request::authorize(3, b"bc1qworker.1", b"s3cr\"et");
        let shown = String::from_utf8(loggable(&req)).unwrap();
        assert!(!shown.contains("s3cr") && shown.contains("bc1qworker.1") && shown.contains("***"));
        assert!(shown.contains("\"id\": 3"));
        let other = super::super::request::suggest_difficulty(4, 1000);
        assert_eq!(loggable(&other), other.strip_suffix(b"\n").unwrap());
    }
}
