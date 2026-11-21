//! The pool manager and the stratum tasks (port of
//! `main/stratum/stratum_manager.cpp`, `stratum_task.cpp` and
//! `stratum_task_v2.cpp`): one task per pool runs the connection loop
//! (`stratum_task.rs`) with the Stratum v1 (`stratum_v1::connection`) or
//! v2 (`stratum_v2::connection`) protocol loop; the manager decides through
//! the pool policy (`pool.rs`) which pools are connected, hands the pool
//! messages to `pool_dispatch.rs`, submits shares, keeps the statistics
//! (`pool_stats.rs`) and the coinbase verification (`pool_coinbase.rs`), and
//! starts the ping tasks (`ping.rs`; the ICMP session stays in
//! `ping_task.cpp`). C API: `pool_manager.h`, wrapped by the inline class in
//! `main/stratum/stratum_manager.h`.
//!
//! The manager lock (`m_mutex`, not recursive) is the mutex around the pool
//! configs; it is held where the C++ code held it.
//!
//! Implementation notes:
//! - the 30 s reconnect timers are run by the manager task (checked every
//!   second) instead of FreeRTOS software timers; they do not fire while
//!   the device is shut down (the stratum tasks are suspended then anyway);
//! - the task's copy of the pool config is locked, so a share submitted
//!   while the task reloads its config reads a consistent user name;
//! - the protocol (v1 or v2) can change at runtime: the stratum task builds
//!   the new transport and session between two connections, so switching
//!   needs no restart;
//! - failover and dual pool mode can change at runtime too
//!   (`load_settings()`): the policy and the statistics switch mode and
//!   the pools are connected as the new mode wants.

use core::ffi::{c_char, c_int, c_void, CStr};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering::*};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use crate::ajson::{Json, Num};
use crate::config;
use crate::http_json::{ManagerInfo, ManagerPool};
use crate::log::log;
use crate::pool::block;
use crate::pool::{self as policy, Event as PolicyEvent};
use crate::pool::{Actions, PoolState};
use crate::pool_coinbase as cb;
use crate::pool_coinbase::{Outcome, VerifyConfig};
use crate::pool_dispatch::{self as dispatch, V1Hooks, V2Hooks};
use crate::pool_stats as stats;
use crate::sdkconfig as k;
use crate::stratum_config::PoolConfig;
use crate::stratum_transport::{Kind, Transport};
use crate::stratum_v1::connection::{self as v1, Io, Transfer};
use crate::stratum_v1::lines::LineBuffer;
use crate::stratum_v1::session::Session as V1Session;
use crate::stratum_v1::MiningNotify;
use crate::stratum_v2::connection::{self as v2, FrameIo, Readiness};
use crate::stratum_v2::protocol::FrameHeader;
use crate::stratum_v2::session::{ExtendedJob, Session as V2Session};

const TAG: &CStr = c"stratum-manager";
const RECONNECT_PERIOD_US: u64 = 30_000_000;
/// The watchdog is fed if a share response came within this time.
const WATCHDOG_RESPONSE_US: u64 = 3600 * 1_000_000;
/// `Board::Error::COINBASE_VERIFY_FAULT`
const COINBASE_VERIFY_FAULT: c_int = 6;
/// The version bits the ASICs may roll on Stratum v2.
const SV2_VERSION_MASK: u32 = 0x1fffe000;
/// Frame header (6) and the longest SubmitShares (extended) payload.
const SV2_SUBMIT_BUF: usize = 6 + 24 + 1 + 32;

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::board_api::*;
    pub use crate::idf::raw::{
        esp_task_wdt_add, esp_task_wdt_delete, esp_task_wdt_reset, uxTaskGetStackHighWaterMark,
    };
}

/// On the host: no tasks, no board.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn uxTaskGetStackHighWaterMark(_: *mut c_void) -> u32 {
        0
    }
    pub unsafe fn esp_task_wdt_add(_: *mut c_void) -> c_int {
        0
    }
    pub unsafe fn esp_task_wdt_delete(_: *mut c_void) -> c_int {
        0
    }
    pub unsafe fn esp_task_wdt_reset() -> c_int {
        0
    }
    pub fn bapi_mining_agent() -> &'static CStr {
        c"agent"
    }
    pub fn bapi_asic_model() -> &'static CStr {
        c"BM1370"
    }
    pub fn bapi_device_model() -> &'static CStr {
        c"NerdQAxe++"
    }
    pub fn bapi_asic_min_difficulty(dual: bool) -> u32 {
        if dual {
            1024
        } else {
            512
        }
    }
    pub fn bapi_asic_max_difficulty() -> u32 {
        4096
    }
    pub fn bapi_set_board_error_if_none(_: c_int, _: u32) {}
    pub fn bapi_clear_board_error(_: c_int) {}
}

use crate::idf::task::delay_ms;

use crate::idf::task::suspend_self;

fn now_us() -> u64 {
    crate::cutil::now_us().max(1) as u64
}

fn set_u64(key: &[u8], v: u64) {
    config::set(key, Json::Num(Num::U(v)));
    crate::config_nvs::request_flush();
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------------------
// the stratum task of one pool

enum Proto {
    V1 {
        tcp: Transport,
        tls: Transport,
        /// boxed like the v2 session
        session: Box<Mutex<V1Session>>,
        lines: Mutex<LineBuffer>,
    },
    V2 {
        noise: Transport,
        /// boxed: its pending jobs would crowd the task stack
        session: Box<Mutex<V2Session>>,
        /// (no 64-bit atomics on the ESP32-S3)
        last_submit_us: Mutex<u64>,
    },
}

impl Proto {
    /// A new transport and session for v2 (`sv2`) or v1.
    fn new(sv2: bool) -> Self {
        if sv2 {
            Proto::V2 {
                noise: Transport::new(Kind::Noise),
                session: Box::new(Mutex::new(V2Session::new(true))),
                last_submit_us: Mutex::new(0),
            }
        } else {
            Proto::V1 {
                tcp: Transport::new(Kind::Tcp),
                tls: Transport::new(Kind::Tls),
                session: Box::default(),
                lines: Mutex::new(LineBuffer::default()),
            }
        }
    }

    fn is_v2(&self) -> bool {
        matches!(self, Proto::V2 { .. })
    }

    /// The transport of the connection (`tls`: v1 over TLS).
    fn transport(&self, tls: bool) -> &Transport {
        match self {
            Proto::V1 { tcp, tls: t, .. } => {
                if tls {
                    t
                } else {
                    tcp
                }
            }
            Proto::V2 { noise, .. } => noise,
        }
    }
}

pub struct Task {
    pool: usize,
    tag: &'static CStr,
    /// replaced by the stratum task between two connections when the
    /// protocol changes; a submit holds its own reference meanwhile
    proto: Mutex<Arc<Proto>>,
    /// the task's copy of the pool config (`m_config`)
    config: Mutex<PoolConfig>,
    /// the pool is not to be connected (`m_stopFlag`)
    stop: AtomicBool,
    reconnect: AtomicBool,
    connected: AtomicBool,
    first_job: AtomicBool,
    valid_notify: AtomicBool,
    setup_failed: AtomicBool,
    /// V1: the current connection uses TLS
    tls: AtomicBool,
    pool_errors: AtomicI32,
    stack_low: AtomicU32,
    /// the last resolved address (for the ping task), empty: none
    resolved_ip: Mutex<String>,
    /// when the reconnect timer fires next, 0: stopped
    timer_due_us: Mutex<u64>,
}

impl Task {
    fn new(pool: usize, config: PoolConfig) -> Self {
        let proto = Mutex::new(Arc::new(Proto::new(config.is_sv2())));
        Task {
            pool,
            tag: if pool == 0 {
                c"stratum task (Pri)"
            } else {
                c"stratum task (Sec)"
            },
            proto,
            config: Mutex::new(config),
            stop: AtomicBool::new(true),
            reconnect: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            first_job: AtomicBool::new(true),
            valid_notify: AtomicBool::new(false),
            setup_failed: AtomicBool::new(false),
            tls: AtomicBool::new(false),
            pool_errors: AtomicI32::new(0),
            stack_low: AtomicU32::new(0),
            resolved_ip: Mutex::new(String::new()),
            timer_due_us: Mutex::new(0),
        }
    }

    fn proto(&self) -> Arc<Proto> {
        lock(&self.proto).clone()
    }

    /// The transport of the current connection.
    fn transport<'a>(&self, proto: &'a Proto) -> &'a Transport {
        proto.transport(self.tls.load(Acquire))
    }

    /// Replaces transport and session when the configured protocol is not
    /// the current one (between connections, from the stratum task).
    fn switch_protocol(&self, sv2: bool) -> Arc<Proto> {
        let mut cur = lock(&self.proto);
        if cur.is_v2() != sv2 {
            log!(
                Info,
                self.tag,
                "protocol changed to Stratum {}",
                if sv2 { "v2" } else { "v1" }
            );
            // the jobs of the old protocol's session are void
            crate::jobs::invalidate(self.pool as c_int);
            self.valid_notify.store(false, Release);
            *cur = Arc::new(Proto::new(sv2));
        }
        cur.clone()
    }

    fn trigger_reconnect(&self) {
        self.reconnect.store(true, Release);
    }

    /// Starts the reconnect timer unless it runs.
    fn start_timer(&self) {
        let mut due = lock(&self.timer_due_us);
        if *due == 0 {
            *due = now_us() + RECONNECT_PERIOD_US;
        }
    }

    fn stop_timer(&self) {
        *lock(&self.timer_due_us) = 0;
    }

    /// Whether the timer fired at `now`; reloads it.
    fn timer_fired(&self, now: u64) -> bool {
        let mut due = lock(&self.timer_due_us);
        if *due == 0 || now < *due {
            return false;
        }
        *due += RECONNECT_PERIOD_US;
        if *due <= now {
            *due = now + RECONNECT_PERIOD_US;
        }
        true
    }

    /// Logs the free stack when it reaches a new low.
    fn log_stack_low(&self) {
        let free = unsafe { sys::uxTaskGetStackHighWaterMark(core::ptr::null_mut()) };
        let low = self.stack_low.load(Relaxed);
        if low == 0 || free < low {
            self.stack_low.store(free, Relaxed);
            log!(Info, self.tag, "free stack low: {free} bytes");
        }
    }

    fn resolved_ip(&self) -> Option<String> {
        let ip = lock(&self.resolved_ip);
        (!ip.is_empty()).then(|| ip.clone())
    }

    /// The authority key and whether it is required (Stratum v2).
    fn configure_noise(&self, noise: &Transport) {
        let primary = lock(&self.config).primary;
        let key = config::get_str(if primary {
            b"sv2_auth_pk"
        } else {
            b"fbsv2_authpk"
        })
        .map(|k| String::from_utf8_lossy(crate::cutil::until_nul(&k)).into_owned())
        .unwrap_or_default();
        let key = if key.is_empty() {
            None
        } else {
            use crate::stratum_v2::authority::{authority_key, KeyError};
            const T: &CStr = c"stratum_v2";
            match authority_key(&key) {
                Ok(k) => {
                    log!(Info, T, "Successfully decoded base58 authority pubkey");
                    Some(k)
                }
                Err(KeyError::Length(n)) => {
                    log!(Error, T, "Invalid decoded length: {n} (expected 38)");
                    None
                }
                Err(KeyError::Checksum) => {
                    log!(
                        Error,
                        T,
                        "Invalid authority pubkey checksum (typo in the key?)"
                    );
                    None
                }
                Err(KeyError::Version(v)) => {
                    log!(Error, T, "Invalid key version: 0x{v:04x} (expected 0x0001)");
                    None
                }
                Err(KeyError::Encoding) => {
                    log!(Error, T, "Failed to decode base58 authority pubkey");
                    None
                }
            }
        };
        if key.is_some() {
            log!(
                Info,
                self.tag,
                "Authority pubkey configured, will verify server certificate"
            );
        } else {
            log!(
                Warn,
                self.tag,
                "No authority pubkey configured, server identity will not be verified"
            );
        }
        noise.set_authority(key);
        // defaults to false: connect unauthenticated as before
        let require = config::get_u16(
            if primary {
                b"sv2_reqauth"
            } else {
                b"fbsv2_reqauth"
            },
            0,
        ) != 0;
        noise.set_require_auth(require);
    }

    fn submit(
        &self,
        job_id: &CStr,
        extranonce2: &CStr,
        ntime: u32,
        nonce: u32,
        version_rolled: u32,
        version_base: u32,
    ) {
        let proto = self.proto();
        match &*proto {
            Proto::V1 { session, .. } => {
                // mining.submit takes the rolled bits (delta), not the version
                let delta = version_rolled ^ version_base;
                let user = lock(&self.config).user.clone();
                v1::send_tracked(&mut TIo(self.transport(&proto)), session, |s| {
                    s.submit(
                        user.to_bytes(),
                        job_id.to_bytes(),
                        extranonce2.to_bytes(),
                        ntime,
                        nonce,
                        delta,
                    )
                });
            }
            Proto::V2 {
                noise,
                session,
                last_submit_us,
            } => {
                if !noise.is_secured() {
                    log!(Error, self.tag, "Cannot submit share: no connection");
                    return;
                }
                // the session picks extended or standard and counts the
                // sequence number
                let frame = lock(session).submit(
                    job_id.to_bytes(),
                    extranonce2.to_bytes(),
                    ntime,
                    nonce,
                    version_rolled,
                );
                let Some(frame) = frame.filter(|f| f.len() <= SV2_SUBMIT_BUF) else {
                    log!(Error, self.tag, "Failed to build SubmitShares frame");
                    return;
                };
                *lock(last_submit_us) = now_us();
                // through the transport: its lock protects the Noise keys
                // against a concurrent close()
                if noise.send(&frame) != frame.len() as c_int {
                    log!(Error, self.tag, "Failed to send share");
                }
            }
        }
    }
}

/// The transport as the socket of the Stratum v1 loop.
struct TIo<'a>(&'a Transport);

impl Io for TIo<'_> {
    fn send(&mut self, data: &[u8]) -> Transfer {
        Transfer::from_raw(self.0.send(data))
    }
    fn recv(&mut self, buf: &mut [u8]) -> Transfer {
        Transfer::from_raw(self.0.recv(buf))
    }
    fn is_connected(&mut self) -> bool {
        self.0.is_connected()
    }
    fn delay_ms(&mut self, ms: u32) {
        delay_ms(ms)
    }
}

/// A task and its manager, for the loops' hooks.
struct Ctx {
    m: &'static Manager,
    t: &'static Task,
    /// the protocol of the current round
    proto: Arc<Proto>,
}

impl crate::stratum_task::Hooks for Ctx {
    fn suspend_if_shutdown(&mut self) {
        if crate::power_mgmt::is_shutdown() {
            log!(Warn, self.t.tag, "suspended");
            suspend_self();
        }
    }
    fn clear_reconnect(&mut self) {
        self.t.reconnect.store(false, Release);
    }
    fn reconnect_requested(&mut self) -> bool {
        self.t.reconnect.load(Acquire)
    }
    fn load_config(&mut self) -> (String, u16) {
        // a consistent copy of the config (it may be changed in the UI)
        let c = self.m.lock()[self.t.pool].clone();
        let r = (c.host.to_string_lossy().into_owned(), c.port as u16);
        self.proto = self.t.switch_protocol(c.is_sv2());
        *lock(&self.t.config) = c;
        r
    }
    fn stopped(&mut self) -> bool {
        self.t.stop.load(Acquire)
    }
    fn network_up(&mut self) -> bool {
        crate::netmgr::has_ip()
    }
    fn resolve(&mut self, host: &str) -> Option<String> {
        use std::net::{IpAddr, ToSocketAddrs};
        const T: &CStr = c"DNS";
        let ip = match (host, 0u16).to_socket_addrs() {
            Ok(addrs) => addrs
                .map(|a| a.ip())
                .find(IpAddr::is_ipv4)
                .map(|ip| ip.to_string()),
            Err(e) => {
                log!(Error, T, "DNS lookup failed: {e}");
                return None;
            }
        };
        let Some(ip) = ip else {
            log!(Error, T, "DNS lookup failed: no IPv4 address");
            return None;
        };
        lock(&self.t.resolved_ip).clone_from(&ip);
        log!(Info, T, "Resolved IP: {ip}");
        Some(ip)
    }
    fn connect(&mut self, host: &str, ip: &str, port: u16) -> bool {
        let t = self.t;
        // the transport of this connection
        match &*self.proto {
            Proto::V1 { .. } => t.tls.store(lock(&t.config).tls, Release),
            Proto::V2 { noise, .. } => t.configure_noise(noise),
        }
        let (Ok(h), Ok(i)) = (CString::new(host), CString::new(ip)) else {
            return false;
        };
        if !t.transport(&self.proto).connect(&h, Some(&i), port) {
            let errno = std::io::Error::last_os_error().raw_os_error().unwrap_or(0);
            log!(
                Error,
                t.tag,
                "Socket unable to connect to {host}:{port} (errno {errno})"
            );
            return false;
        }
        true
    }
    fn protocol_loop(&mut self) {
        let proto = self.proto.clone();
        match &*proto {
            Proto::V1 { session, lines, .. } => self.v1_loop(session, lines),
            Proto::V2 {
                session,
                last_submit_us,
                ..
            } => {
                *lock(last_submit_us) = 0;
                self.v2_loop(session)
            }
        }
    }
    fn count_pool_error(&mut self) {
        self.t.pool_errors.fetch_add(1, Relaxed);
    }
    fn disconnect(&mut self) {
        // gate new submits before tearing the transport down; the
        // transport's lock covers a submit already inside send()
        self.t.connected.store(false, Release);
        self.t.transport(&self.proto).close();
        self.m.on_event(self.t.pool, PolicyEvent::Disconnected);
    }
    fn delay_ms(&mut self, ms: u32) {
        delay_ms(ms)
    }
}

impl Ctx {
    fn v1_loop(&self, session: &Mutex<V1Session>, lines: &Mutex<LineBuffer>) {
        let t = self.t;
        t.setup_failed.store(false, Release);
        // the first job clears the queue, whatever its clean flag says
        t.first_job.store(true, Release);

        let (user, password, enonce_sub) = {
            let c = lock(&t.config);
            (c.user.clone(), c.password.clone(), c.enonce_sub)
        };
        let (device, asic) = (sys::bapi_mining_agent(), sys::bapi_asic_model());
        let version = crate::mdns::fw_version();
        let setup = v1::Setup {
            device: device.to_bytes(),
            asic: asic.to_bytes(),
            version: version.as_bytes(),
            user: user.to_bytes(),
            password: password.to_bytes(),
            difficulty: config::get_u64(b"stratumdiff", k::STRATUM_DIFFICULTY as u64) as u32,
            extranonce_subscribe: enonce_sub,
        };
        let mut hooks = V1Loop {
            m: self.m,
            t,
            session,
        };
        v1::run(
            &mut TIo(t.transport(&self.proto)),
            &mut hooks,
            &setup,
            session,
            &mut lock(lines),
        );
    }

    fn v2_loop(&self, session: &Mutex<V2Session>) {
        let t = self.t;
        t.setup_failed.store(false, Release);
        let (host, port, user) = {
            let c = lock(&t.config);
            (c.host.clone(), c.port as u16, c.user.clone())
        };
        let (vendor, hw) = (sys::bapi_device_model(), sys::bapi_asic_model());
        let setup = v2::Setup {
            host: host.to_bytes(),
            port,
            vendor: vendor.to_bytes(),
            hw_version: hw.to_bytes(),
            user: user.to_bytes(),
            // extended channels only (standard channel support disabled)
            extended: true,
        };
        let mut hooks = V2Loop { c: self, session };
        v2::run(
            &mut NoiseIo(t.transport(&self.proto)),
            &mut hooks,
            &setup,
            session,
        );
    }
}

struct V1Loop<'a> {
    m: &'static Manager,
    t: &'static Task,
    session: &'a Mutex<V1Session>,
}

impl v1::Hooks for V1Loop<'_> {
    fn stop_requested(&mut self) -> bool {
        self.t.stop.load(Acquire) || crate::power_mgmt::is_shutdown()
    }
    fn reconnect_requested(&mut self) -> bool {
        self.t.reconnect.load(Acquire)
    }
    fn keepalive_enabled(&mut self) -> bool {
        config::get_u16(b"stratum_keep", k::STRATUM_KEEPALIVE_ENABLE_VALUE as u16) != 0
    }
    fn connected(&mut self) {
        self.m.on_event(self.t.pool, PolicyEvent::Connected);
        self.t.connected.store(true, Release);
    }
    fn dispatch(&mut self, line: &[u8]) {
        // cut at a NUL byte, as the C string it once was
        let line = crate::cutil::until_nul(line);
        self.m.dispatch(self.t.pool, line, self.session);
        self.t.log_stack_low();
    }
    fn setup_failed(&mut self) -> bool {
        self.t.setup_failed.load(Acquire)
    }
}

/// SV2 frames through the transport's Noise channel.
struct NoiseIo<'a>(&'a Transport);

impl FrameIo for NoiseIo<'_> {
    fn wait_readable(&mut self, ms: u32) -> Readiness {
        self.0.poll_read(ms)
    }
    fn send(&mut self, frame: &[u8]) -> bool {
        self.0.send(frame) == frame.len() as c_int
    }
    fn recv(&mut self, buf: &mut [u8]) -> Option<FrameHeader> {
        self.0.recv_frame(buf)
    }
}

/// The task's side of the SV2 connection loop and its session events.
struct V2Loop<'a> {
    c: &'a Ctx,
    session: &'a Mutex<V2Session>,
}

impl v2::Hooks for V2Loop<'_> {
    fn exit_requested(&mut self) -> bool {
        let t = self.c.t;
        t.stop.load(Acquire) || t.reconnect.load(Acquire) || crate::power_mgmt::is_shutdown()
    }
    fn connected(&mut self) {
        let c = self.c;
        c.m.on_event(c.t.pool, PolicyEvent::Connected);
        c.t.connected.store(true, Release);
    }
    fn drain_events(&mut self) {
        let Proto::V2 { last_submit_us, .. } = &*self.c.proto else {
            return;
        };
        let last_submit_us = *lock(last_submit_us);
        let (c, session) = (self.c, self.session);
        // statistics in Rust, everything else through the hooks
        dispatch::drain_v2(
            self,
            session,
            &c.m.stats,
            c.t.pool as c_int,
            c.t.tag,
            last_submit_us,
            now_us(),
        );
        c.t.log_stack_low();
    }
    fn setup_failed(&mut self) -> bool {
        self.c.t.setup_failed.load(Acquire)
    }
}

impl V2Hooks for V2Loop<'_> {
    fn setup_failed(&mut self, _: c_int) {
        // drop the connection like a pool error instead of mining nothing
        self.c.t.setup_failed.store(true, Release);
    }

    fn valid_notify(&mut self, _: c_int) {
        self.c.t.valid_notify.store(true, Release);
    }

    /// Coinbase verification, then the job source.
    fn extended_job(
        &mut self,
        _: c_int,
        job: ExtendedJob,
        clean: bool,
        extranonce_size: u8,
        pool_difficulty: u32,
    ) {
        let c = self.c;
        // SV2: coinbase_prefix | extranonce_prefix | extranonce2 | suffix; the
        // verification gets coinbase_1 = prefix + extranonce_prefix, an empty
        // extranonce1 and the full extranonce size, so that coinbase_2 (the
        // suffix) starts with the nSequence
        if !job.coinbase_prefix.is_empty() && !job.coinbase_suffix.is_empty() {
            let en_len = job.extranonce_prefix.len().min(32);
            let mut c1 = crate::cutil::to_hex(&job.coinbase_prefix);
            c1 += &crate::cutil::to_hex(&job.extranonce_prefix[..en_len]);
            let c2 = crate::cutil::to_hex(&job.coinbase_suffix);
            if let (Ok(c1), Ok(c2)) = (CString::new(c1), CString::new(c2)) {
                c.m.process_coinbase_v2(
                    c.t.pool,
                    &c1,
                    &c2,
                    job.version,
                    job.nbits,
                    extranonce_size as i32,
                );
            }
        }
        crate::jobs::sv2_extended(
            c.t.pool as c_int,
            job,
            clean,
            extranonce_size,
            SV2_VERSION_MASK,
            pool_difficulty,
        );
    }
}

// ---------------------------------------------------------------------------
// the manager

pub struct Manager {
    /// dual pool mode (changed by `load_settings()`)
    dual: AtomicBool,
    policy: policy::Handle,
    /// boxed like the coinbase state: the manager is built on the small
    /// main task stack
    stats: Box<stats::Handle>,
    /// boxed: two decoded coinbases (with their scriptSig) would not fit on
    /// the main task stack the manager is built on
    coinbase: Box<cb::Handle>,
    /// the manager lock (`m_mutex`) with the pool configs
    configs: Mutex<[PoolConfig; 2]>,
    tasks: [OnceLock<&'static Task>; 2],
    pings: [AtomicBool; 2],
    initialized: AtomicBool,
}

static MANAGER: OnceLock<Manager> = OnceLock::new();

// The manager is built on the main task stack (3.5 KB, CONFIG_ESP_MAIN_TASK_
// STACK_SIZE) before it moves to the heap: its large state stays boxed.
const _: () = assert!(core::mem::size_of::<Manager>() <= 512);

/// The manager once created.
pub fn get() -> Option<&'static Manager> {
    MANAGER.get()
}

/// Lets the dispatch hooks reach the manager and the configs it holds the
/// lock for.
struct DispatchCtx<'a> {
    m: &'a Manager,
    cfg: &'a [PoolConfig; 2],
}

impl V1Hooks for DispatchCtx<'_> {
    fn setup_failed(&mut self, pool: c_int) {
        if let Some(t) = self.m.task(pool as usize) {
            t.setup_failed.store(true, Release);
        }
    }

    fn take_first_job(&mut self, pool: c_int) -> bool {
        self.m
            .task(pool as usize)
            .is_some_and(|t| t.first_job.swap(false, AcqRel))
    }

    fn valid_notify(&mut self, pool: c_int) {
        if let Some(t) = self.m.task(pool as usize) {
            t.valid_notify.store(true, Release);
        }
    }

    fn process_coinbase(&mut self, pool: c_int, n: &MiningNotify) {
        if n.coinbase_1.is_null() || n.coinbase_2.is_null() || !(0..2).contains(&pool) {
            return;
        }
        let c = &self.cfg[pool as usize];
        // Stratum v2 pools are verified in V2Loop::extended_job()
        if c.is_sv2() {
            return;
        }
        let vc = verify_config(pool);
        // SAFETY: the parser's NUL-terminated strings, alive for this call
        let (c1, c2) = unsafe { (CStr::from_ptr(n.coinbase_1), CStr::from_ptr(n.coinbase_2)) };
        let o = self.m.coinbase.process_v1(
            pool,
            c1.to_bytes(),
            c2.to_bytes(),
            n.version,
            n.target,
            Some(c.user.to_bytes()),
            vc,
        );
        self.m.apply_verify_outcome(self.cfg, pool, &o);
    }
}

/// The verification settings of a pool.
fn verify_config(pool: c_int) -> VerifyConfig {
    let primary = pool == 0;
    VerifyConfig {
        mode: config::get_u16(
            if primary {
                b"cb_verify_mode"
            } else {
                b"fb_cb_vfy_mode"
            },
            0,
        ),
        max_fee: config::get_u16(
            if primary {
                b"cb_max_fee"
            } else {
                b"fb_cb_max_fee"
            },
            30,
        ),
        force: config::get_u16(
            if primary {
                b"cb_vfy_force"
            } else {
                b"fb_cb_vfy_frc"
            },
            0,
        ) != 0,
    }
}

fn block_reason(b: u8) -> Option<&'static CStr> {
    match b {
        block::ADDRESS_NOT_FOUND => Some(c"address_not_found"),
        block::FEE_EXCEEDED => Some(c"fee_exceeded"),
        _ => None,
    }
}

impl Manager {
    fn new(dual: bool) -> Self {
        // SAFETY: new handles, kept for the lifetime of the firmware
        Manager {
            dual: AtomicBool::new(dual),
            policy: policy::Handle::new(dual),
            stats: Box::new(stats::Handle::new(
                dual,
                config::get_u64(b"bestdiff", 0),
                config::get_u64(b"totalblocks", 0) as u32,
            )),
            coinbase: Box::default(),
            configs: Mutex::new([PoolConfig::load(0), PoolConfig::load(1)]),
            tasks: [OnceLock::new(), OnceLock::new()],
            pings: [AtomicBool::new(false), AtomicBool::new(false)],
            initialized: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, [PoolConfig; 2]> {
        lock(&self.configs)
    }

    pub fn is_dual(&self) -> bool {
        self.dual.load(Acquire)
    }

    fn task(&self, pool: usize) -> Option<&'static Task> {
        self.tasks.get(pool)?.get().copied()
    }

    pub fn is_connected(&self, pool: usize) -> bool {
        self.task(pool).is_some_and(|t| t.connected.load(Acquire))
    }

    pub fn is_verify_blocked(&self, pool: usize) -> bool {
        self.coinbase.blocked((pool & 1) as i32) != block::NONE
    }

    fn state(&self) -> PoolState {
        let mut s = PoolState::default();
        for i in 0..2 {
            s.has_task[i] = self.task(i).is_some();
            s.connected[i] = self.is_connected(i);
            s.verify_blocked[i] = self.is_verify_blocked(i);
        }
        s
    }

    /// Carries out the policy's decisions (with the lock held).
    fn apply(&self, a: &Actions) {
        let each = |f: &dyn Fn(&Task), on: [bool; 2]| {
            for (i, &on) in on.iter().enumerate() {
                if let Some(t) = self.task(i).filter(|_| on) {
                    f(t)
                }
            }
        };
        for i in 0..2 {
            if a.invalidate[i] {
                crate::jobs::invalidate(i as c_int);
                if let Some(t) = self.task(i) {
                    t.valid_notify.store(false, Release);
                }
            }
        }
        each(&|t| t.stop.store(true, Release), a.disconnect);
        each(&|t| t.stop_timer(), a.stop_timer);
        each(&|t| t.start_timer(), a.start_timer);
        each(&|t| t.stop.store(false, Release), a.connect);
        for i in 0..2 {
            if a.reset_verify_counts[i] {
                self.coinbase.reset_counts(i as i32);
            }
        }
    }

    /// A connection event: the policy decides what happens.
    fn on_event(&self, pool: usize, event: PolicyEvent) {
        let _g = self.lock();
        let s = self.state();
        let a = self.policy.event(event, pool as i32, &s);
        self.apply(&a);
    }

    /// Hands a Stratum v1 line to `pool_dispatch.rs`.
    fn dispatch(&self, pool: usize, line: &[u8], session: &Mutex<V1Session>) {
        let cfg = self.lock();
        if !self.policy.accepts_notify_from(pool as i32) {
            return;
        }
        let Some(t) = self.task(pool) else {
            log!(Error, TAG, "stratum task is null");
            return;
        };
        let mut d = DispatchCtx { m: self, cfg: &cfg };
        dispatch::dispatch_v1(
            &mut d,
            session,
            &self.stats,
            &self.coinbase,
            pool as c_int,
            t.tag,
            line,
            now_us(),
        );
    }

    fn process_coinbase_v2(
        &self,
        pool: usize,
        c1: &CStr,
        c2: &CStr,
        version: u32,
        nbits: u32,
        extranonce2_len: i32,
    ) {
        // the stratum task calls without the lock; applying the outcome
        // needs it
        let cfg = self.lock();
        let vc = verify_config(pool as c_int);
        let o = self.coinbase.process_v2(
            pool as c_int,
            c1.to_bytes(),
            c2.to_bytes(),
            version,
            nbits,
            (b"", extranonce2_len),
            Some(cfg[pool].user.to_bytes()),
            vc,
        );
        self.apply_verify_outcome(&cfg, pool as c_int, &o);
    }

    /// Sends the alert, reconnects, sets the board's fault flag (with the
    /// lock held).
    fn apply_verify_outcome(&self, cfg: &[PoolConfig; 2], pool: c_int, o: &Outcome) {
        const T: &CStr = c"stratum_manager";
        if o.parse_failed {
            log!(Error, T, "SV2 coinbase parse failed");
            return;
        }
        if o.alert {
            let info = self.coinbase_info(pool);
            crate::discord::ffi::discord_send_coinbase_verify_failed(
                pool,
                info.result.user_value_satoshis,
                info.result.total_value_satoshis,
                verify_config(pool).mode as c_int,
            );
        }
        if o.reconnect {
            let reason = block_reason(self.coinbase_info(pool).blocked)
                .map_or("(null)".into(), CStr::to_string_lossy);
            log!(
                Warn,
                T,
                "Coinbase verification failed for pool {pool} ({reason}) - blocking"
            );
            if let Some(t) = self.task(pool as usize) {
                t.trigger_reconnect();
            }
        }
        // all *configured* pools verify-blocked: the display shows an error
        // (the ASICs keep running on stale jobs, nothing is submitted to a
        // blocked pool); a pool without host does not count as usable
        if o.check_fault {
            let usable = policy::any_usable(
                [!cfg[0].host.is_empty(), !cfg[1].host.is_empty()],
                [self.is_verify_blocked(0), self.is_verify_blocked(1)],
            );
            if !usable {
                log!(
                    Warn,
                    T,
                    "All pools blocked by verification - raising fault flag"
                );
                sys::bapi_set_board_error_if_none(COINBASE_VERIFY_FAULT, 0);
            } else {
                sys::bapi_clear_board_error(COINBASE_VERIFY_FAULT);
            }
        }
    }

    pub(crate) fn coinbase_info(&self, pool: c_int) -> cb::Info {
        self.coinbase.info(pool & 1)
    }

    /// The coinbase verification state.
    pub(crate) fn coinbase(&self) -> &cb::Handle {
        &self.coinbase
    }

    pub fn rerun_verification(&self, pool: c_int) {
        let cfg = self.lock();
        let pool = pool & 1;
        let vc = verify_config(pool);
        let o = self.coinbase.verify(pool, vc);
        self.apply_verify_outcome(&cfg, pool, &o);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn submit_share(
        &self,
        pool: usize,
        job_id: &CStr,
        extranonce2: &CStr,
        ntime: u32,
        nonce: u32,
        version_rolled: u32,
        version_base: u32,
    ) {
        let Some(t) = self.task(pool) else {
            log!(Error, TAG, "stratum task is null");
            return;
        };
        if !t.connected.load(Acquire) {
            log!(Error, TAG, "selected pool not connected");
            return;
        }
        t.submit(
            job_id,
            extranonce2,
            ntime,
            nonce,
            version_rolled,
            version_base,
        );
    }

    /// The pool for the next job (dual pool dithering; failover: the
    /// selected pool).
    pub fn next_active_pool(&self) -> c_int {
        let _g = self.lock();
        let valid = |i| {
            self.task(i)
                .is_some_and(|t| t.valid_notify.load(Acquire) && !self.is_verify_blocked(i))
        };
        self.policy.next_pool([valid(0), valid(1)])
    }

    fn selected(&self) -> c_int {
        self.policy.next_pool([false, false])
    }

    pub fn select_asic_diff(&self, pool: c_int, pool_diff: u32) -> u32 {
        let (min, max) = (
            sys::bapi_asic_min_difficulty(self.is_dual()),
            sys::bapi_asic_max_difficulty(),
        );
        self.policy.select_asic_diff(pool, pool_diff, min, max)
    }

    pub fn active_balance(&self, pool: c_int) -> c_int {
        let _g = self.lock();
        let s = self.state();
        self.policy.active_balance(pool, &s)
    }

    /// The statistics (one consistent snapshot).
    pub fn summary(&self) -> stats::Summary {
        self.stats.summary()
    }

    pub fn best_session(&self) -> u64 {
        self.summary().best_session
    }

    /// Blocks found since the start (or the last session reset).
    pub fn found_blocks(&self) -> u32 {
        self.stats.summary().found_blocks
    }

    pub fn pool_errors(&self) -> c_int {
        (0..2)
            .filter_map(|i| self.task(i))
            .map(|t| t.pool_errors.load(Relaxed))
            .sum()
    }

    pub fn reconnect_all(&self) {
        for i in 0..2 {
            if let Some(t) = self.task(i) {
                t.trigger_reconnect();
            }
            if self.pings[i].load(Acquire) {
                crate::ping::reset(i as c_int);
            }
        }
    }

    /// Reloads the pool configs (and the dual pool balance) and reconnects
    /// the pools whose settings changed.
    pub fn load_settings(&self) {
        let mut cfg = self.lock();
        let mut reconnect = false;
        let (mode, _) = crate::boot::pool_mode(crate::cfg::pool_mode() as c_int);
        self.set_mode(mode == crate::boot::PoolMode::Dual);
        if self.is_dual() {
            // a changed balance resets the dithering and reconnects
            let balance = config::get_u16(b"pool_balance", 50);
            reconnect = self.policy.set_balance(balance);
            self.stats.set_balance(balance);
        }
        self.stats.set_totals(
            config::get_u64(b"bestdiff", 0),
            config::get_u64(b"totalblocks", 0) as u32,
        );
        for (i, c) in cfg.iter_mut().enumerate() {
            if !(c.reload() || reconnect) {
                continue;
            }
            if let Some(t) = self.task(i) {
                t.trigger_reconnect();
            }
            // session stats, verification, ping
            self.stats.reset_pool(i as i32);
            if let Some(t) = self.task(i) {
                t.pool_errors.store(0, Relaxed);
            }
            self.coinbase.reset_counts(i as i32);
            self.coinbase.clear_blocked(i as i32);
            if self.pings[i].load(Acquire) {
                crate::ping::reset(i as c_int);
            }
        }
    }

    /// Switches between failover and dual pool mode (with the lock held):
    /// policy and statistics in the new mode, then the pools connected as
    /// the new mode wants.
    fn set_mode(&self, dual: bool) {
        let balance = config::get_u16(b"pool_balance", 50);
        if !self.policy.set_mode(dual, balance) {
            return;
        }
        log!(
            Info,
            TAG,
            "pool mode changed to {}",
            if dual { "Dual Pool" } else { "Failover" }
        );
        self.stats.set_mode(dual);
        self.stats.set_balance(balance);
        self.dual.store(dual, Release);
        if !self.initialized.load(Acquire) {
            // the manager task connects the pools of the mode it starts in
            return;
        }
        // the jobs of a pool that stays connected stay valid; a dropped
        // pool's jobs go when its connection ends (policy: disconnected)
        let s = self.state();
        if dual {
            // both pools: the connected one keeps its connection
            for i in 0..2 {
                if let Some(t) = self.task(i) {
                    t.stop.store(false, Release);
                    t.start_timer();
                }
            }
        } else {
            // the policy picks the pool to keep from the connected ones
            // (the primary first) and drops the other
            for i in 0..2 {
                if s.connected[i] {
                    let a = self.policy.event(PolicyEvent::Connected, i as i32, &s);
                    self.apply(&a);
                    if s.connected[0] {
                        break;
                    }
                }
            }
            // a primary that is not connected: try it, and the secondary
            // as its fallback
            if !s.connected[0] {
                if let Some(t) = self.task(0) {
                    t.stop.store(false, Release);
                    t.start_timer();
                }
                if let Some(t) = self.task(1) {
                    t.start_timer();
                }
            }
        }
        // a stopped pool leaves its protocol loop
        for i in 0..2 {
            if let Some(t) = self.task(i).filter(|t| t.stop.load(Acquire)) {
                t.trigger_reconnect();
            }
        }
    }

    pub fn reset_session_stats(&self) {
        let _g = self.lock();
        for i in 0..2 {
            if let Some(t) = self.task(i) {
                t.pool_errors.store(0, Relaxed);
            }
        }
        self.stats.reset_session();
    }

    pub fn reset_all_time_stats(&self) {
        let _g = self.lock();
        self.stats.reset_all_time();
        set_u64(b"bestdiff", 0);
        set_u64(b"totalblocks", 0);
    }

    pub fn check_best_diff(&self, pool: c_int, diff: f64, nbits: u32) {
        let ev = self.stats.best_diff(pool, diff, nbits);
        if !ev.new_total_best {
            return;
        }
        set_u64(b"bestdiff", ev.total_best);
        crate::discord::ffi::discord_send_best_diff(diff, ev.network_difficulty);
    }

    pub fn check_found_block(&self, diff: f64, nbits: u32) {
        let ev = self.stats.found_block(diff, nbits);
        if !ev.found_block {
            return;
        }
        log!(
            Info,
            TAG,
            "FOUND BLOCK!!! {} > {}",
            crate::cutil::fixed(diff, 6),
            crate::cutil::fixed(ev.network_difficulty, 6)
        );
        set_u64(b"totalblocks", ev.total_found_blocks as u64);
        crate::discord::ffi::discord_send_block_found(diff, ev.network_difficulty);
    }

    pub fn pool_host(&self, pool: c_int) -> CString {
        // under the lock: a reload replaces the host
        let cfg = self.lock();
        let h = &cfg[(pool & 1) as usize].host;
        if h.is_empty() {
            c"-".into()
        } else {
            h.clone()
        }
    }

    pub fn pool_port(&self, pool: c_int) -> c_int {
        self.lock()[(pool & 1) as usize].port
    }

    fn ping_value(&self, pool: usize, f: extern "C" fn(c_int) -> f64) -> f64 {
        if self.pings[pool].load(Acquire) {
            f(pool as c_int)
        } else {
            0.0
        }
    }

    /// The ping values of the pool the v1 API shows.
    pub fn compat_ping(&self, f: extern "C" fn(c_int) -> f64) -> f64 {
        let i = self.policy.compat_ping_pool();
        self.ping_value((i & 1) as usize, f)
    }

    pub fn info(&self) -> ManagerInfo {
        let cfg = self.lock();
        let sum = self.stats.summary();
        let pool = |i: usize| {
            let st = self.stats.pool(i as i32);
            ManagerPool {
                connected: self.is_connected(i),
                accepted: st.accepted,
                rejected: st.rejected,
                best_session: st.best_session,
                network_difficulty: st.network_difficulty,
                pool_difficulty: st.pool_difficulty,
                pool_diff_err: self.policy.pool_diff_err(i as i32),
                ping_rtt: self.ping_value(i, crate::ping::ffi::ping_last_rtt),
                ping_loss: self.ping_value(i, crate::ping::ffi::ping_recent_loss),
                verify_blocked: block_reason(self.coinbase_info(i as c_int).blocked)
                    .map_or(core::ptr::null(), CStr::as_ptr),
                protocol: cfg[i].protocol as c_int,
                encrypted: cfg[i].is_sv2() || cfg[i].tls,
            }
        };
        let fallback = !self.is_dual();
        ManagerInfo {
            config_pool_mode: config::get_u16(b"pool_mode", 0),
            pool_mode: self.is_dual() as c_int,
            pool_balance: config::get_u16(b"pool_balance", 50),
            total_best_diff: sum.total_best,
            fallback,
            using_fallback: fallback && self.selected() == 1,
            dual: self.is_dual(),
            selected: self.selected(),
            pools: [pool(0), pool(1)],
            accepted: sum.accepted,
            rejected: sum.rejected,
            best_session: sum.best_session,
            network_difficulty: sum.network_difficulty,
            pool_difficulty: sum.pool_difficulty,
        }
    }

    /// The manager task: starts the stratum and ping tasks, then runs the
    /// reconnect timers and feeds the watchdog.
    fn run(&'static self) -> ! {
        const T: &CStr = c"StratumManager";
        log!(Info, T, "Subscribing to task watchdog.");
        if unsafe { sys::esp_task_wdt_add(core::ptr::null_mut()) } != 0 {
            log!(Error, T, "Failed to add task to watchdog!");
        }
        log!(
            Info,
            T,
            "{} mode enabled",
            if self.is_dual() {
                "Dual Pool"
            } else {
                "Failover"
            }
        );

        for i in 0..2 {
            let c = self.lock()[i].clone();
            let t: &'static Task = Box::leak(Box::new(Task::new(i, c)));
            let _ = self.tasks[i].set(t);
            // 12 KB: with the Rust loops in the call chain, 8 KB overflowed
            // once on the first mining.notify (see log_stack_low())
            let name = [c"stratum task (pri)", c"stratum task (sec)"][i];
            crate::idf::task::spawn_psram(stratum_task_entry, name, 12288, i, 5);
            self.pings[i].store(true, Release);
            let name = [c"ping task (pri)", c"ping task (sec)"][i];
            crate::idf::task::spawn_psram(ping_task_entry, name, 4096, i, 1);
        }

        let first = if self.is_dual() { 2 } else { 1 };
        for i in 0..first {
            if let Some(t) = self.task(i) {
                t.stop.store(false, Release);
            }
        }
        self.initialized.store(true, Release);

        let mut seconds = 0u32;
        loop {
            if crate::power_mgmt::is_shutdown() {
                if unsafe { sys::esp_task_wdt_delete(core::ptr::null_mut()) } != 0 {
                    log!(Error, TAG, "Couldn't remove watchdog");
                }
                log!(Warn, TAG, "suspended");
                suspend_self();
            }
            delay_ms(1000);
            let now = now_us();
            for i in 0..2 {
                if self.task(i).is_some_and(|t| t.timer_fired(now)) {
                    self.on_event(i, PolicyEvent::ReconnectTimer);
                }
            }
            seconds += 1;
            if seconds % 30 == 0 {
                // fed only while the pool answers submits (within an hour)
                let last = self.stats.summary().last_response_us;
                if last != 0 && now_us().saturating_sub(last) < WATCHDOG_RESPONSE_US {
                    unsafe { sys::esp_task_wdt_reset() };
                }
            }
        }
    }
}

unsafe extern "C" fn stratum_task_entry(arg: *mut c_void) {
    let Some(m) = get() else { return };
    let Some(t) = m.task(arg as usize) else {
        return;
    };
    t.start_timer();
    let mut c = Ctx {
        m,
        t,
        proto: t.proto(),
    };
    loop {
        crate::stratum_task::round(&mut c, t.tag);
    }
}

/// The ping task of a pool: the loop in `ping.rs`, the pings in C++.
struct PingEnv {
    m: &'static Manager,
    pool: usize,
    tag: &'static CStr,
}

impl crate::ping::Env for PingEnv {
    fn shutdown(&mut self) -> bool {
        crate::power_mgmt::is_shutdown()
    }
    fn suspend(&mut self) {
        log!(Warn, self.tag, "suspended");
        suspend_self();
    }
    fn connected(&mut self) -> bool {
        self.m.is_connected(self.pool)
    }
    fn target(&mut self) -> Option<(String, String)> {
        let host = self.m.lock()[self.pool].host.to_string_lossy().into_owned();
        let ip = self.m.task(self.pool)?.resolved_ip()?;
        Some((host, ip))
    }
    fn ping(&mut self, ip: &str, host: &str) -> crate::ping::Round {
        crate::ping_session::perform(self.pool, ip, host)
    }
    fn log(&mut self, line: &str) {
        let level = if line.starts_with("No resolved") {
            crate::log::Level::Error
        } else {
            crate::log::Level::Info
        };
        crate::log::write(level, self.tag, line);
    }
}

unsafe extern "C" fn ping_task_entry(arg: *mut c_void) {
    let Some(m) = get() else { return };
    let pool = arg as usize;
    let mut env = PingEnv {
        m,
        pool,
        tag: [c"ping task (pri)", c"ping task (sec)"][pool & 1],
    };
    crate::ping::run(pool as c_int, &mut env)
}

pub(crate) mod ffi {
    use super::*;

    fn m() -> Option<&'static Manager> {
        get()
    }

    unsafe fn cstr<'a>(p: *const c_char) -> &'a CStr {
        if p.is_null() {
            c""
        } else {
            CStr::from_ptr(p)
        }
    }

    /// Creates the manager (once); `dual`: dual pool mode, else failover.
    #[no_mangle]
    pub extern "C" fn pool_mgr_create(dual: bool) {
        let _ = MANAGER.set(Manager::new(dual));
    }

    pub extern "C" fn pool_mgr_exists() -> bool {
        m().is_some()
    }

    /// The manager task; never returns.
    #[no_mangle]
    pub unsafe extern "C" fn pool_mgr_task(_: *mut c_void) {
        if let Some(m) = m() {
            m.run();
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_is_dual() -> bool {
        m().is_some_and(|m| m.is_dual())
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_is_initialized() -> bool {
        m().is_some_and(|m| m.initialized.load(Acquire))
    }

    /// # Safety
    /// Strings NUL-terminated or NULL.
    #[no_mangle]
    #[allow(clippy::too_many_arguments)]
    pub unsafe extern "C" fn pool_mgr_submit_share(
        pool: c_int,
        job_id: *const c_char,
        extranonce2: *const c_char,
        ntime: u32,
        nonce: u32,
        version_rolled: u32,
        version_base: u32,
    ) {
        if let Some(m) = m() {
            m.submit_share(
                (pool & 1) as usize,
                cstr(job_id),
                cstr(extranonce2),
                ntime,
                nonce,
                version_rolled,
                version_base,
            );
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_check_best_diff(pool: c_int, diff: f64, nbits: u32) {
        if let Some(m) = m() {
            m.check_best_diff(pool, diff, nbits);
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_check_found_block(_pool: c_int, diff: f64, nbits: u32) {
        if let Some(m) = m() {
            m.check_found_block(diff, nbits);
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_next_active_pool() -> c_int {
        m().map_or(-1, Manager::next_active_pool)
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_select_asic_diff(pool: c_int, pool_diff: u32) -> u32 {
        m().map_or(pool_diff, |m| m.select_asic_diff(pool, pool_diff))
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_is_connected(pool: c_int) -> bool {
        m().is_some_and(|m| m.is_connected((pool & 1) as usize))
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_reconnect_all() {
        if let Some(m) = m() {
            m.reconnect_all();
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_load_settings() {
        if let Some(m) = m() {
            m.load_settings();
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_reset_session_stats() {
        if let Some(m) = m() {
            m.reset_session_stats();
        }
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_reset_all_time_stats() {
        if let Some(m) = m() {
            m.reset_all_time_stats();
        }
    }

    /// # Safety
    /// `info` writable.
    #[no_mangle]
    pub unsafe extern "C" fn pool_mgr_info(info: *mut ManagerInfo) {
        if let Some(m) = m() {
            *info = m.info();
        }
    }

    /// The host of `pool` ("-" if none) into `buf`, copied under the lock.
    ///
    /// # Safety
    /// `buf` writable for `len` bytes (or NULL).
    #[no_mangle]
    pub unsafe extern "C" fn pool_mgr_pool_host(pool: c_int, buf: *mut c_char, len: usize) {
        if buf.is_null() || len == 0 {
            return;
        }
        let h = m().map_or_else(|| c"-".into(), |m| m.pool_host(pool));
        let b = h.as_bytes();
        let n = b.len().min(len - 1);
        core::ptr::copy_nonoverlapping(b.as_ptr(), buf as *mut u8, n);
        *buf.add(n) = 0;
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_pool_port(pool: c_int) -> c_int {
        m().map_or(0, |m| m.pool_port(pool))
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_active_balance(pool: c_int) -> c_int {
        m().map_or(0, |m| m.active_balance(pool))
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_display_pool(pool: c_int) -> c_int {
        m().map_or(0, |m| m.policy.display_pool(pool))
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_using_fallback() -> bool {
        m().is_some_and(|m| !m.is_dual() && m.selected() == 1)
    }

    #[no_mangle]
    pub extern "C" fn pool_mgr_pool_errors() -> c_int {
        m().map_or(0, Manager::pool_errors)
    }

    /// The aggregated statistics (zeros before the manager exists).
    ///
    /// # Safety
    /// `out` writable.
    #[no_mangle]
    pub unsafe extern "C" fn pool_mgr_summary(out: *mut stats::Summary) {
        *out = m().map_or_else(Default::default, |m| m.stats.summary());
    }

    /// The counters of one pool (zeros before the manager exists).
    ///
    /// # Safety
    /// `out` writable.
    #[no_mangle]
    pub unsafe extern "C" fn pool_mgr_pool_stats(pool: c_int, out: *mut stats::PoolSummary) {
        *out = m().map_or_else(Default::default, |m| m.stats.pool(pool));
    }

    /// Round trip time (ms) of the pool the v1 API shows; 0 without ping.
    #[no_mangle]
    pub extern "C" fn pool_mgr_ping_rtt() -> f64 {
        m().map_or(0.0, |m| m.compat_ping(crate::ping::ffi::ping_last_rtt))
    }

    /// Recent ping loss (0..1) of the pool the v1 API shows.
    #[no_mangle]
    pub extern "C" fn pool_mgr_ping_loss() -> f64 {
        m().map_or(0.0, |m| m.compat_ping(crate::ping::ffi::ping_recent_loss))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(dual: bool) -> &'static Manager {
        let m: &'static Manager = Box::leak(Box::new(Manager::new(dual)));
        for i in 0..2 {
            let t: &'static Task = Box::leak(Box::new(Task::new(i, PoolConfig::load(i as c_int))));
            let _ = m.tasks[i].set(t);
        }
        m
    }

    #[test]
    fn mode_switch_connects_the_pools_of_the_new_mode() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _j = crate::jobs::test_support::fresh();
        let m = manager(false);
        m.initialized.store(true, Release);
        let (t0, t1) = (m.task(0).unwrap(), m.task(1).unwrap());
        // failover, the primary mining
        t0.stop.store(false, Release);
        t0.connected.store(true, Release);
        {
            let _l = m.lock();
            m.set_mode(true);
        }
        assert!(m.is_dual());
        assert!(!t0.stop.load(Acquire) && !t1.stop.load(Acquire));
        assert!(!t0.reconnect.load(Acquire));
        // dual, both connected: back to failover keeps the primary
        t1.connected.store(true, Release);
        {
            let _l = m.lock();
            m.set_mode(false);
        }
        assert!(!m.is_dual());
        assert!(!t0.stop.load(Acquire) && !t0.reconnect.load(Acquire));
        assert!(t1.stop.load(Acquire) && t1.reconnect.load(Acquire));
        // only the secondary connected: failover keeps it, retries the primary
        t0.connected.store(false, Release);
        t1.stop.store(false, Release);
        t1.reconnect.store(false, Release);
        {
            let _l = m.lock();
            m.set_mode(true);
            m.set_mode(false);
        }
        assert!(!t1.stop.load(Acquire) && !t0.stop.load(Acquire));
        assert_eq!(m.selected(), 1);
    }

    #[test]
    fn protocol_switches_between_connections() {
        // the switch drops the pool's jobs: the job tables are shared
        let _j = crate::jobs::test_support::fresh();
        let t = Task::new(1, PoolConfig::load(1));
        let v1 = t.proto();
        assert!(!v1.is_v2());
        // unchanged: the same transport and session
        assert!(Arc::ptr_eq(&t.switch_protocol(false), &v1));
        // v2: a new one; a submit holding the old one keeps it alive
        let v2 = t.switch_protocol(true);
        assert!(v2.is_v2() && Arc::ptr_eq(&t.proto(), &v2));
        assert!(!v1.is_v2());
        drop(v1);
        // and back
        assert!(!t.switch_protocol(false).is_v2());
        assert!(!t.proto().is_v2());
    }

    #[test]
    fn timer_starts_once_fires_and_reloads() {
        let m = manager(false);
        let t = m.task(0).unwrap();
        assert!(!t.timer_fired(u64::MAX / 2));
        t.start_timer();
        let due = *lock(&t.timer_due_us);
        // a running timer is not restarted
        t.start_timer();
        assert_eq!(*lock(&t.timer_due_us), due);
        assert!(!t.timer_fired(due - 1));
        assert!(t.timer_fired(due));
        assert_eq!(*lock(&t.timer_due_us), due + RECONNECT_PERIOD_US);
        // long overdue: the next period starts now
        assert!(t.timer_fired(due + 10 * RECONNECT_PERIOD_US));
        assert_eq!(*lock(&t.timer_due_us), due + 11 * RECONNECT_PERIOD_US);
        t.stop_timer();
        assert!(!t.timer_fired(u64::MAX / 2));
    }

    #[test]
    fn actions_drive_the_tasks() {
        let m = manager(false);
        let (p, s) = (m.task(0).unwrap(), m.task(1).unwrap());
        p.valid_notify.store(true, Release);
        let a = Actions {
            invalidate: [true, false],
            disconnect: [true, false],
            start_timer: [false, true],
            connect: [false, true],
            ..Default::default()
        };
        m.apply(&a);
        assert!(!p.valid_notify.load(Acquire));
        assert!(p.stop.load(Acquire));
        assert!(!s.stop.load(Acquire));
        assert_ne!(*lock(&s.timer_due_us), 0);
        m.apply(&Actions {
            stop_timer: [false, true],
            ..Default::default()
        });
        assert_eq!(*lock(&s.timer_due_us), 0);
    }

    #[test]
    fn failover_events() {
        let m = manager(false);
        let (p, s) = (m.task(0).unwrap(), m.task(1).unwrap());
        p.stop.store(false, Release);
        // the primary drops: the secondary's timer runs, then connects it
        m.on_event(0, PolicyEvent::Disconnected);
        assert_ne!(*lock(&s.timer_due_us), 0);
        assert!(s.stop.load(Acquire));
        m.on_event(1, PolicyEvent::ReconnectTimer);
        assert!(!s.stop.load(Acquire));
        // the primary is back: the secondary is dropped, its timer stopped
        p.connected.store(true, Release);
        m.on_event(0, PolicyEvent::Connected);
        assert!(s.stop.load(Acquire));
        assert_eq!(*lock(&s.timer_due_us), 0);
    }

    #[test]
    fn submit_needs_a_connection_and_errors_add_up() {
        let m = manager(true);
        m.submit_share(0, c"1", c"00", 0, 0, 0, 0);
        m.task(0).unwrap().pool_errors.store(2, Relaxed);
        m.task(1).unwrap().pool_errors.store(3, Relaxed);
        assert_eq!(m.pool_errors(), 5);
        m.reset_session_stats();
        assert_eq!(m.pool_errors(), 0);
    }

    #[test]
    fn info_reports_mode_and_pools() {
        let m = manager(true);
        m.task(1).unwrap().connected.store(true, Release);
        let i = m.info();
        assert!(i.dual && !i.fallback && !i.using_fallback);
        assert_eq!(i.pool_mode, 1);
        assert!(!i.pools[0].connected && i.pools[1].connected);
        assert!(i.pools[0].verify_blocked.is_null());
        assert_eq!(i.pools[0].ping_rtt, 0.0);
    }
}
