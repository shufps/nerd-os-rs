//! What the pool manager does with pool messages (port of
//! `StratumManager::dispatch()` for Stratum v1 and
//! `StratumTaskV2::drainEvents()` for Stratum v2): statistics and stored
//! extranonce and the job sources (`mining/jobs.rs`) here, connection flags and
//! coinbase verification through the manager's [`V1Hooks`] / [`V2Hooks`].
//!
//! `SubmitShares.Success` counts its shares at once instead of once per
//! share: the count comes from the pool and can be as large as `0xFFFFFFFF`.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;

use crate::log::log;
use crate::mining::jobs;
use crate::pool::coinbase_verify::Handle as Coinbase;
use crate::pool::stats::Handle as Stats;
use crate::pool::stratum_v1::method::*;
use crate::pool::stratum_v1::session::Session as V1Session;
use crate::pool::stratum_v1::{Message, MiningNotify};
use crate::pool::stratum_v2::session::{Event, ExtendedJob, Session as V2Session};

/// The manager's side of a Stratum v1 message. `pool` is the pool the
/// message came from.
pub trait V1Hooks {
    /// The subscribe response was unusable: reconnect.
    fn setup_failed(&mut self, pool: c_int);
    /// Whether no notify was handled since connecting; clears the flag.
    fn take_first_job(&mut self, pool: c_int) -> bool;
    /// The pool sends valid work (pool selection).
    fn valid_notify(&mut self, pool: c_int);
    /// Coinbase verification of a notify (needs the pool config).
    fn process_coinbase(&mut self, pool: c_int, notify: &MiningNotify);
}

/// The manager's side of the Stratum v2 session events.
pub trait V2Hooks {
    /// Coinbase verification, then `jobs::sv2_extended()`.
    fn extended_job(
        &mut self,
        pool: c_int,
        job: ExtendedJob,
        clean: bool,
        extranonce_size: u8,
        pool_difficulty: u32,
    );
    /// The connection is unusable: reconnect.
    fn setup_failed(&mut self, pool: c_int);
    /// The pool sends valid work (pool selection).
    fn valid_notify(&mut self, pool: c_int);
}

/// `s` for a log line ("" for None).
fn text(s: Option<&[u8]>) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(s.unwrap_or_default())
}

/// Acts on one parsed v1 message (`ok`: the parser's result).
#[allow(clippy::too_many_arguments)]
fn handle_v1(
    h: &mut impl V1Hooks,
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
            h.setup_failed(pool);
        }
        return;
    }

    let enonce = || m.extranonce_str.as_deref();
    let store_extranonce =
        || coinbase.with(|c| c.set_extranonce(pool, enonce(), m.extranonce_2_len));

    match m.method {
        MINING_NOTIFY => {
            let Some(n) = &m.mining_notification else {
                return;
            };
            stats.with(|s| s.set_network_difficulty(pool, n.target));
            h.process_coinbase(pool, n);
            let first = h.take_first_job(pool);
            jobs::v1_notify(pool, n, m.should_abandon_work || first);
            if n.ntime != 0 {
                h.valid_notify(pool);
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
                text(enonce()),
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
                text(enonce()),
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
fn handle_v2(
    h: &mut impl V2Hooks,
    stats: &Stats,
    pool: c_int,
    tag: &CStr,
    ev: Event,
    last_submit_us: u64,
    now_us: u64,
) {
    match ev {
        Event::NetworkDifficulty(nbits) => stats.with(|s| s.set_network_difficulty(pool, nbits)),
        Event::PoolDifficulty(d) => stats.with(|s| s.set_pool_difficulty(pool, d)),
        Event::SetTarget(d) => {
            stats.with(|s| s.set_pool_difficulty(pool, d));
            jobs::sv2_set_difficulty(pool, d);
        }
        // the standard channel rolls version bits 13-28
        Event::StandardJob {
            job,
            pool_difficulty,
            clean,
        } => jobs::sv2_standard(pool, &job, clean, 0x1fffe000, pool_difficulty),
        Event::ExtendedJob {
            job,
            extranonce_size,
            pool_difficulty,
            clean,
        } => h.extended_job(pool, job, clean, extranonce_size, pool_difficulty),
        Event::SharesAccepted(n) => {
            if last_submit_us > 0 {
                // tenths of a millisecond, without float formatting
                let tenths = now_us.saturating_sub(last_submit_us) / 100;
                log!(
                    Info,
                    tag,
                    "Shares accepted: {} ({}.{} ms)",
                    n,
                    tenths / 10,
                    tenths % 10
                );
            } else {
                log!(Info, tag, "Shares accepted: {}", n);
            }
            stats.with(|s| s.shares_accepted(pool, n, now_us));
        }
        Event::ShareRejected(_) => stats.with(|s| s.share_result(pool, false, now_us)),
        // drop the connection like a pool error instead of silently mining nothing
        Event::SetupFailed => h.setup_failed(pool),
        // used by the manager for pool selection
        Event::ValidNotify => h.valid_notify(pool),
    }
}

/// Parses `line` and acts on it (the former `StratumManager::dispatch()`
/// after its pool check, which stays with the caller and its lock).
#[allow(clippy::too_many_arguments)]
pub fn dispatch_v1(
    h: &mut impl V1Hooks,
    session: &Mutex<V1Session>,
    stats: &Stats,
    coinbase: &Coinbase,
    pool: c_int,
    tag: &CStr,
    line: &[u8],
    now_us: u64,
) {
    let mut m = Message::default();
    let ok = session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .parse(line, &mut m);
    crate::pool::stratum_v1::session::log_parsed(ok, &m);
    handle_v1(h, stats, coinbase, pool, tag, &m, ok, now_us);
}

/// Takes every pending session event and acts on it (the former
/// `StratumTaskV2::drainEvents()`).
pub fn drain_v2(
    h: &mut impl V2Hooks,
    session: &Mutex<V2Session>,
    stats: &Stats,
    pool: c_int,
    tag: &CStr,
    last_submit_us: u64,
    now_us: u64,
) {
    loop {
        // not locked while acting on the event: the hooks submit nothing,
        // but the job pipeline must not wait for a share submit either
        let ev = session
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_event();
        let Some(ev) = ev else { break };
        handle_v2(h, stats, pool, tag, ev, last_submit_us, now_us);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining::jobs::test_support::{fresh, take};
    use crate::pool::stats::PoolSummary;

    /// What the hooks saw, in order.
    #[derive(Default)]
    struct Rec {
        calls: Vec<String>,
        first_job: bool,
    }

    impl V1Hooks for Rec {
        fn setup_failed(&mut self, pool: c_int) {
            self.calls.push(format!("setup_failed {pool}"));
        }
        fn take_first_job(&mut self, _pool: c_int) -> bool {
            core::mem::replace(&mut self.first_job, false)
        }
        fn valid_notify(&mut self, pool: c_int) {
            self.calls.push(format!("valid_notify {pool}"));
        }
        fn process_coinbase(&mut self, pool: c_int, n: &MiningNotify) {
            let c1 = text(Some(&n.coinbase_1));
            self.calls.push(format!("coinbase {pool} {c1}"));
        }
    }

    impl V2Hooks for Rec {
        fn extended_job(&mut self, pool: c_int, job: ExtendedJob, _: bool, size: u8, d: u32) {
            self.calls
                .push(format!("extended {pool} {} {size} {d}", job.job_id));
        }
        fn setup_failed(&mut self, pool: c_int) {
            V1Hooks::setup_failed(self, pool)
        }
        fn valid_notify(&mut self, pool: c_int) {
            V1Hooks::valid_notify(self, pool)
        }
    }

    fn pool_summary(stats: &Stats, pool: c_int) -> PoolSummary {
        stats.pool(pool)
    }

    struct V1 {
        session: Mutex<V1Session>,
        stats: Stats,
        coinbase: Coinbase,
        rec: Rec,
        _jobs: std::sync::MutexGuard<'static, ()>,
    }

    impl V1 {
        fn new() -> Self {
            Self {
                session: Mutex::new(V1Session::default()),
                stats: Stats::new(true, 0, 0),
                coinbase: Coinbase::default(),
                rec: Rec::default(),
                _jobs: fresh(),
            }
        }

        fn line(&mut self, pool: c_int, line: &str, now_us: u64) {
            dispatch_v1(
                &mut self.rec,
                &self.session,
                &self.stats,
                &self.coinbase,
                pool,
                c"stratum",
                line.as_bytes(),
                now_us,
            );
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
        j.is_some().then(|| take(j))
    }

    #[test]
    fn notify_sets_network_difficulty_and_creates_the_job() {
        let mut t = V1::new();
        t.line(1, SUBSCRIBE, 0);
        // the first notify abandons the pool's old jobs
        jobs::sv2_standard(1, &old_job(), false, 0, 1);
        jobs::JOB_TABLES[0].store(jobs::build(1, 0, false, |d| d).unwrap(), 3);
        t.rec.first_job = true;
        t.line(1, &NOTIFY.replace('\n', ""), 0);
        assert_eq!(
            t.rec.calls,
            [
                "coinbase 1 01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff2003",
                "valid_notify 1",
            ]
        );
        assert!(jobs::JOB_TABLES[0].clone_job(3).is_none());
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

    fn old_job() -> crate::pool::stratum_v2::session::StandardJob {
        crate::pool::stratum_v2::session::StandardJob {
            job_id: 1,
            version: 0,
            merkle_root: [0; 32],
            prev_hash: [0; 32],
            ntime: 1,
            nbits: 0,
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
        session: Mutex<V2Session>,
        stats: Stats,
        rec: Rec,
        _jobs: std::sync::MutexGuard<'static, ()>,
    }

    impl V2 {
        fn new() -> Self {
            Self {
                session: Mutex::new(V2Session::new(true)),
                stats: Stats::new(false, 0, 0),
                rec: Rec::default(),
                _jobs: fresh(),
            }
        }

        fn drain(&mut self, events: Vec<Event>) {
            self.session.lock().unwrap().events.extend(events);
            drain_v2(
                &mut self.rec,
                &self.session,
                &self.stats,
                0,
                c"stratum_v2",
                0,
                1,
            );
        }
    }

    #[test]
    fn sv2_events() {
        let mut t = V2::new();
        t.drain(vec![
            Event::PoolDifficulty(512),
            Event::StandardJob {
                job: crate::pool::stratum_v2::session::StandardJob {
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
