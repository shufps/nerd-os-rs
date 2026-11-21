//! What the pool manager does with pool messages (port of
//! `StratumManager::dispatch()` for Stratum v1 and
//! `StratumTaskV2::drainEvents()` for Stratum v2): statistics and stored
//! extranonce and the job sources (`jobs.rs`) here in Rust, connection
//! flags and coinbase verification through hooks.
//!
//! `SubmitShares.Success` counts its shares at once instead of once per
//! share: the count comes from the pool and can be as large as `0xFFFFFFFF`.

use core::ffi::{c_char, c_int, CStr};

use crate::jobs;
use crate::log::log;
use crate::pool_coinbase::Handle as Coinbase;
use crate::pool_stats::Handle as Stats;
use crate::stratum_v1::method::*;
use crate::stratum_v1::{Message, MiningNotify};
use crate::stratum_v2::session::{event_kind, CEvent};
use crate::stratum_v2::ExtJob;

/// `pool_v1_hooks_t`: the C++ side of a Stratum v1 message. `pool` is the
/// pool the message came from.
#[repr(C)]
pub struct V1Hooks {
    pub ctx: *mut core::ffi::c_void,
    /// The subscribe response was unusable: reconnect.
    pub setup_failed: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int),
    /// Whether no notify was handled since connecting; clears the flag.
    pub take_first_job: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int) -> bool,
    /// The pool sends valid work (pool selection).
    pub valid_notify: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int),
    /// Coinbase verification of a notify (needs the pool config).
    pub process_coinbase:
        unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int, notify: *const MiningNotify),
}

/// `pool_v2_hooks_t`: the C++ side of the Stratum v2 session events.
#[repr(C)]
pub struct V2Hooks {
    pub ctx: *mut core::ffi::c_void,
    /// Coinbase verification, then `job_source_sv2_extended()`. Takes
    /// ownership of `job` (free with `sv2_ext_job_free()`).
    pub extended_job: unsafe extern "C" fn(
        ctx: *mut core::ffi::c_void,
        pool: c_int,
        job: *mut ExtJob,
        extranonce_size: u8,
        pool_difficulty: u32,
    ),
    /// The connection is unusable: reconnect.
    pub setup_failed: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int),
    /// The pool sends valid work (pool selection).
    pub valid_notify: unsafe extern "C" fn(ctx: *mut core::ffi::c_void, pool: c_int),
}

unsafe fn text<'a>(p: *const c_char) -> std::borrow::Cow<'a, str> {
    if p.is_null() {
        "".into()
    } else {
        CStr::from_ptr(p).to_string_lossy()
    }
}

/// Acts on one parsed v1 message (`ok`: the parser's result).
///
/// # Safety
/// `m` comes from the v1 parser; the hooks are valid.
#[allow(clippy::too_many_arguments)]
unsafe fn handle_v1(
    h: &V1Hooks,
    stats: &Stats,
    coinbase: &Coinbase,
    pool: c_int,
    tag: &CStr,
    m: &Message,
    ok: bool,
    now_us: u64,
) {
    if !ok {
        log!(Error, tag, "error in stratum");
        if m.method == STRATUM_RESULT_SUBSCRIBE {
            log!(
                Error,
                tag,
                "invalid mining.subscribe response, reconnecting ..."
            );
            (h.setup_failed)(h.ctx, pool);
        }
        return;
    }

    let enonce =
        || (!m.extranonce_str.is_null()).then(|| CStr::from_ptr(m.extranonce_str).to_bytes());
    let store_extranonce =
        || coinbase.with(|c| c.set_extranonce(pool, enonce(), m.extranonce_2_len));

    match m.method {
        MINING_NOTIFY => {
            let Some(n) = m.mining_notification.as_ref() else {
                return;
            };
            stats.with(|s| s.set_network_difficulty(pool, n.target));
            (h.process_coinbase)(h.ctx, pool, n);
            let first = (h.take_first_job)(h.ctx, pool);
            jobs::v1_notify(pool, n, m.should_abandon_work != 0 || first);
            if n.ntime != 0 {
                (h.valid_notify)(h.ctx, pool);
            }
        }
        MINING_SET_DIFFICULTY => {
            stats.with(|s| s.set_pool_difficulty(pool, m.new_difficulty));
            if jobs::v1_set_difficulty(pool, m.new_difficulty) {
                log!(Info, tag, "Set stratum difficulty: {}", m.new_difficulty);
            }
        }
        MINING_SET_VERSION_MASK | STRATUM_RESULT_VERSION_MASK => {
            log!(Info, tag, "Set version mask: {:08x}", m.version_mask);
            jobs::v1_set_version_mask(pool, m.version_mask);
        }
        MINING_SET_EXTRANONCE => {
            log!(
                Info,
                tag,
                "Set next enonce {} enonce2-len: {}",
                text(m.extranonce_str),
                m.extranonce_2_len
            );
            store_extranonce();
            jobs::v1_set_next_extranonce(pool, enonce(), m.extranonce_2_len);
        }
        STRATUM_RESULT_SUBSCRIBE => {
            log!(
                Info,
                tag,
                "Set enonce {} enonce2-len: {}",
                text(m.extranonce_str),
                m.extranonce_2_len
            );
            store_extranonce();
            jobs::v1_set_extranonce(pool, enonce(), m.extranonce_2_len);
        }
        CLIENT_RECONNECT => log!(Error, tag, "Pool requested client reconnect ..."),
        STRATUM_RESULT => {
            if m.response_success {
                log!(Info, tag, "message result accepted");
            } else {
                log!(Warn, tag, "message result rejected");
            }
            stats.with(|s| s.share_result(pool, m.response_success, now_us));
        }
        STRATUM_RESULT_SETUP => {
            if m.response_success {
                log!(Info, tag, "setup message accepted");
            } else {
                log!(Error, tag, "setup message rejected");
            }
        }
        _ => {}
    }
}

/// Acts on one SV2 session event.
///
/// # Safety
/// The hooks are valid; `ev` comes from the session.
unsafe fn handle_v2(
    h: &V2Hooks,
    stats: &Stats,
    pool: c_int,
    tag: &CStr,
    ev: &CEvent,
    last_submit_us: u64,
    now_us: u64,
) {
    use event_kind::*;
    match ev.kind {
        NETWORK_DIFFICULTY => stats.with(|s| s.set_network_difficulty(pool, ev.value)),
        POOL_DIFFICULTY => stats.with(|s| s.set_pool_difficulty(pool, ev.value)),
        SET_TARGET => {
            stats.with(|s| s.set_pool_difficulty(pool, ev.value));
            jobs::sv2_set_difficulty(pool, ev.value);
        }
        // the standard channel rolls version bits 13-28
        STANDARD_JOB => jobs::sv2_standard(pool, &ev.job, 0x1fffe000, ev.value),
        EXTENDED_JOB => (h.extended_job)(h.ctx, pool, ev.ext_job, ev.extranonce_size, ev.value),
        SHARES_ACCEPTED => {
            if last_submit_us > 0 {
                // tenths of a millisecond, without float formatting
                let tenths = now_us.saturating_sub(last_submit_us) / 100;
                log!(
                    Info,
                    tag,
                    "Shares accepted: {} ({}.{} ms)",
                    ev.value,
                    tenths / 10,
                    tenths % 10
                );
            } else {
                log!(Info, tag, "Shares accepted: {}", ev.value);
            }
            stats.with(|s| s.shares_accepted(pool, ev.value, now_us));
        }
        SHARE_REJECTED => stats.with(|s| s.share_result(pool, false, now_us)),
        // drop the connection like a pool error instead of silently mining nothing
        SETUP_FAILED => (h.setup_failed)(h.ctx, pool),
        // used by the manager for pool selection
        VALID_NOTIFY => (h.valid_notify)(h.ctx, pool),
        _ => {}
    }
}

fn tag_or(tag: *const c_char, default: &CStr) -> &CStr {
    if tag.is_null() {
        default
    } else {
        // SAFETY: a NUL-terminated string that outlives the call (caller contract)
        unsafe { CStr::from_ptr(tag) }
    }
}

pub(crate) mod ffi {
    use super::*;
    use crate::stratum_v1::session::ffi::{log_parsed, Handle as V1Session};
    use crate::stratum_v2::session::ffi::{take_event, Handle as V2Session};

    /// Parses `line` and acts on it (the former `StratumManager::dispatch()`
    /// after its pool check, which stays with the caller and its lock).
    ///
    /// # Safety
    /// Valid hooks and handles, `line` and `tag` NUL-terminated (`tag` may be
    /// NULL).
    #[allow(clippy::too_many_arguments)]
    pub unsafe extern "C" fn pool_dispatch_v1(
        hooks: *const V1Hooks,
        session: *mut V1Session,
        stats: &Stats,
        coinbase: &Coinbase,
        pool: c_int,
        tag: *const c_char,
        line: *const c_char,
        now_us: u64,
    ) {
        if line.is_null() {
            return;
        }
        let mut m = Message::zeroed();
        let ok = (*session).with(|s| s.parse(CStr::from_ptr(line).to_bytes(), &mut m));
        log_parsed(ok, &m);
        let tag = tag_or(tag, c"stratum");
        handle_v1(&*hooks, stats, coinbase, pool, tag, &m, ok, now_us);
        crate::stratum_v1::ffi::stratum_v1_message_free(&mut m);
    }

    /// Takes every pending session event and acts on it (the former
    /// `StratumTaskV2::drainEvents()`).
    ///
    /// # Safety
    /// Valid hooks and handles, `tag` NUL-terminated or NULL.
    pub unsafe extern "C" fn pool_drain_v2(
        hooks: *const V2Hooks,
        session: *mut V2Session,
        stats: &Stats,
        pool: c_int,
        tag: *const c_char,
        last_submit_us: u64,
        now_us: u64,
    ) {
        let tag = tag_or(tag, c"stratum_v2");
        while let Some(ev) = take_event(&*session) {
            handle_v2(&*hooks, stats, pool, tag, &ev, last_submit_us, now_us);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ffi::*;
    use super::*;
    use crate::jobs::test_support::{fresh, take};
    use crate::pool_stats::PoolSummary;
    use crate::stratum_v1::session::ffi::{
        stratum_v1_session_free, stratum_v1_session_new, Handle as V1Session,
    };
    use crate::stratum_v2::session::ffi::{sv2_session_new, Handle as V2Session};
    use crate::stratum_v2::session::Event;
    use core::ffi::c_void;

    /// What the hooks saw, in order.
    #[derive(Default)]
    struct Rec {
        calls: Vec<String>,
        first_job: bool,
    }

    unsafe fn rec<'a>(ctx: *mut c_void) -> &'a mut Rec {
        &mut *(ctx as *mut Rec)
    }

    unsafe extern "C" fn setup_failed(ctx: *mut c_void, pool: c_int) {
        rec(ctx).calls.push(format!("setup_failed {pool}"));
    }
    unsafe extern "C" fn take_first_job(ctx: *mut c_void, _pool: c_int) -> bool {
        core::mem::replace(&mut rec(ctx).first_job, false)
    }
    unsafe extern "C" fn valid_notify(ctx: *mut c_void, pool: c_int) {
        rec(ctx).calls.push(format!("valid_notify {pool}"));
    }
    unsafe extern "C" fn process_coinbase(ctx: *mut c_void, pool: c_int, n: *const MiningNotify) {
        rec(ctx)
            .calls
            .push(format!("coinbase {pool} {}", text((*n).coinbase_1)));
    }
    unsafe extern "C" fn extended_job(
        ctx: *mut c_void,
        pool: c_int,
        job: *mut ExtJob,
        size: u8,
        d: u32,
    ) {
        rec(ctx)
            .calls
            .push(format!("extended {pool} {} {size} {d}", (*job).job_id));
        crate::stratum_v2::ffi::sv2_ext_job_free(job);
    }

    fn v1_hooks(r: &mut Rec) -> V1Hooks {
        V1Hooks {
            ctx: r as *mut Rec as *mut c_void,
            setup_failed,
            take_first_job,
            valid_notify,
            process_coinbase,
        }
    }

    fn v2_hooks(r: &mut Rec) -> V2Hooks {
        V2Hooks {
            ctx: r as *mut Rec as *mut c_void,
            extended_job,
            setup_failed,
            valid_notify,
        }
    }

    fn pool_summary(stats: &Stats, pool: c_int) -> PoolSummary {
        stats.pool(pool)
    }

    struct V1 {
        session: *mut V1Session,
        stats: Stats,
        coinbase: Coinbase,
        rec: Rec,
        _jobs: std::sync::MutexGuard<'static, ()>,
    }

    impl V1 {
        fn new() -> Self {
            Self {
                session: stratum_v1_session_new(),
                stats: Stats::new(true, 0, 0),
                coinbase: Coinbase::default(),
                rec: Rec::default(),
                _jobs: fresh(),
            }
        }

        fn line(&mut self, pool: c_int, line: &str, now_us: u64) {
            let line = std::ffi::CString::new(line).unwrap();
            let hooks = v1_hooks(&mut self.rec);
            unsafe {
                pool_dispatch_v1(
                    &hooks,
                    self.session,
                    &self.stats,
                    &self.coinbase,
                    pool,
                    core::ptr::null(),
                    line.as_ptr(),
                    now_us,
                )
            };
        }
    }

    impl Drop for V1 {
        fn drop(&mut self) {
            unsafe { stratum_v1_session_free(self.session) };
        }
    }

    const NOTIFY: &str = r#"{"id":null,"method":"mining.notify","params":["1d2e",
        "0000000000000000000000000000000000000000000000000000000000000000",
        "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff2003",
        "ffffffff0100f2052a01000000160014000000000000000000000000000000000000000000000000",
        [],"20000000","1703a30c","66f00000",false]}"#;

    const SUBSCRIBE: &str = r#"{"id":1,"result":[[["mining.notify","ae6812eb4cd7735a302a8a9dd95cf71f"]],"08000002",4],"error":null}"#;

    /// The job the pool's work builds now (extranonce2 counter 1).
    fn job(pool: c_int) -> Option<(u32, u32, String, String, c_int, u32)> {
        let j = jobs::build(pool, 1, false, |d| d);
        (!j.is_null()).then(|| unsafe { take(j) })
    }

    #[test]
    fn notify_sets_network_difficulty_and_creates_the_job() {
        let mut t = V1::new();
        t.line(1, SUBSCRIBE, 0);
        // the first notify abandons the pool's old jobs
        jobs::sv2_standard(1, &old_job(), 0, 1);
        unsafe { jobs::JOB_TABLES[0].store(jobs::build(1, 0, false, |d| d), 3) };
        t.rec.first_job = true;
        t.line(1, &NOTIFY.replace('\n', ""), 0);
        assert_eq!(
            t.rec.calls,
            [
                "coinbase 1 01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff2003",
                "valid_notify 1",
            ]
        );
        assert!(jobs::JOB_TABLES[0].clone_job(3).is_null());
        assert!(pool_summary(&t.stats, 1).network_difficulty > 1.0);
        // the work was replaced by v1, which needs the extranonce again
        assert!(job(1).is_none());
        t.line(1, SUBSCRIBE, 0);
        t.line(1, &NOTIFY.replace('\n', ""), 0);
        let (pool_diff, _, job_id, en2, pool, _) = job(1).unwrap();
        assert_eq!(
            (pool_diff, job_id.as_str(), en2.as_str(), pool),
            (8192, "1d2e", "00000001", 1)
        );
    }

    fn old_job() -> crate::stratum_v2::session::CJob {
        crate::stratum_v2::session::CJob {
            job_id: 1,
            version: 0,
            merkle_root: [0; 32],
            prev_hash: [0; 32],
            ntime: 1,
            nbits: 0,
            clean_jobs: false,
        }
    }

    #[test]
    fn set_difficulty_and_version_mask_reach_the_work() {
        let mut t = V1::new();
        t.line(0, SUBSCRIBE, 0);
        t.line(
            0,
            r#"{"id":null,"method":"mining.set_difficulty","params":[4096]}"#,
            0,
        );
        t.line(
            0,
            r#"{"id":null,"method":"mining.set_version_mask","params":["1fffe000"]}"#,
            0,
        );
        assert!(t.rec.calls.is_empty());
        assert_eq!(pool_summary(&t.stats, 0).pool_difficulty, 4096);
        // active with the next notify
        t.line(0, &NOTIFY.replace('\n', ""), 0);
        let (pool_diff, .., mask) = job(0).unwrap();
        assert_eq!((pool_diff, mask), (4096, 0x1fffe000));
    }

    #[test]
    fn subscribe_result_sets_and_stores_the_extranonce() {
        let mut t = V1::new();
        // ID 1 without a tracked request: a setup response (subscribe shape)
        t.line(0, SUBSCRIBE, 0);
        t.line(0, &NOTIFY.replace('\n', ""), 0);
        assert_eq!(job(0).unwrap().3, "00000001");
        t.line(
            0,
            r#"{"id":null,"method":"mining.set_extranonce","params":["0a0b0c0d",8]}"#,
            0,
        );
        let stored = t.coinbase.with(|c| c.extranonce(0));
        assert_eq!(stored, (Some(b"0a0b0c0d".to_vec()), 8));
        // the job source switches with the next notify
        assert_eq!(job(0).unwrap().3, "00000001");
        t.line(0, &NOTIFY.replace('\n', ""), 0);
        assert_eq!(job(0).unwrap().3, "0000000000000001");
        assert_eq!(t.rec.calls.len(), 4); // coinbase + valid notify, twice
    }

    #[test]
    fn invalid_subscribe_result_drops_the_connection() {
        let mut t = V1::new();
        t.line(
            0,
            r#"{"id":1,"result":[[["mining.notify","ae68"]],"08000002",99],"error":null}"#,
            0,
        );
        assert_eq!(t.rec.calls, ["setup_failed 0"]);
    }

    #[test]
    fn share_results_are_counted() {
        let mut t = V1::new();
        t.line(1, r#"{"id":10,"result":true,"error":null}"#, 5);
        t.line(1, r#"{"id":11,"result":false,"error":[23,"low",null]}"#, 7);
        let s = pool_summary(&t.stats, 1);
        assert_eq!((s.accepted, s.rejected), (1, 1));
        assert!(t.rec.calls.is_empty());
    }

    struct V2 {
        session: *mut V2Session,
        stats: Stats,
        rec: Rec,
        _jobs: std::sync::MutexGuard<'static, ()>,
    }

    impl V2 {
        fn new() -> Self {
            Self {
                session: sv2_session_new(true),
                stats: Stats::new(false, 0, 0),
                rec: Rec::default(),
                _jobs: fresh(),
            }
        }

        fn drain(&mut self, events: Vec<Event>) {
            unsafe { (*self.session).with(|s| s.events.extend(events)) };
            let hooks = v2_hooks(&mut self.rec);
            unsafe {
                pool_drain_v2(
                    &hooks,
                    self.session,
                    &self.stats,
                    0,
                    core::ptr::null(),
                    0,
                    1,
                )
            };
        }
    }

    #[test]
    fn sv2_events() {
        let mut t = V2::new();
        t.drain(vec![
            Event::PoolDifficulty(512),
            Event::StandardJob {
                job: crate::stratum_v2::session::StandardJob {
                    job_id: 5,
                    version: 0x2000_0000,
                    merkle_root: [1; 32],
                    prev_hash: [2; 32],
                    ntime: 1,
                    nbits: 0x1703_a30c,
                },
                pool_difficulty: 512,
                clean: false,
            },
            Event::SetTarget(1024),
            Event::ValidNotify,
            Event::SharesAccepted(3),
            Event::ShareRejected(b"stale".to_vec()),
            Event::SetupFailed,
        ]);
        assert_eq!(t.rec.calls, ["valid_notify 0", "setup_failed 0"]);
        let s = pool_summary(&t.stats, 0);
        assert_eq!((s.accepted, s.rejected, s.pool_difficulty), (3, 1, 1024));
        // SetTarget changed the standard channel's difficulty in place
        let (pool_diff, _, job_id, _, _, mask) = job(0).unwrap();
        assert_eq!((pool_diff, job_id.as_str(), mask), (1024, "5", 0x1fffe000));
    }

    #[test]
    fn sv2_huge_accepted_count_is_counted_at_once() {
        // the count is added at once, not share by share
        let mut t = V2::new();
        t.drain(vec![
            Event::SharesAccepted(u32::MAX),
            Event::SharesAccepted(u32::MAX),
        ]);
        assert_eq!(pool_summary(&t.stats, 0).accepted, 2 * u32::MAX as u64);
    }
}
