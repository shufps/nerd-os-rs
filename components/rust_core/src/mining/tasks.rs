//! The two mining task loops (port of `create_jobs_task()` in
//! `create_jobs_task.cpp` and `ASIC_result_task()` in
//! `asic_result_task.cpp`): building jobs from the pools' work and sending
//! them to the ASICs and CAN slaves, and checking the nonces the ASICs
//! return (counter registers go to the hashrate monitor, `mining/hashrate.rs`).
//! The ASIC driver, CAN, board, statistics and the pool manager are reached
//! through hooks (`tasks.rs`).
//!
//! Behaviour notes:
//! - the job timer is a deadline in the loop instead of a FreeRTOS timer
//!   signalling a condition variable; when the deadline passes while the
//!   task is busy, the job is built right after;
//! - the duplicate nonce counter (a `u64` written by the result task and
//!   read by the HTTP and influx tasks) is behind a lock.

use core::ffi::{c_int, CStr};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::cutil::fixed;
use crate::log::log;
use crate::mining::hashrate;
use crate::mining::jobs::{self, BmJob, CAN_SLAVE_MAX, JOB_TABLES};
use crate::mining::nonce_difficulty;

// ---------------------------------------------------------------------------
// Job creation

/// Set by `trigger_job_creation()` (new work), cleared by the job task.
static TRIGGER: Mutex<bool> = Mutex::new(false);
static WAKE: Condvar = Condvar::new();

/// Wakes the job task (new work from a pool).
pub fn trigger() {
    *TRIGGER.lock().unwrap_or_else(|e| e.into_inner()) = true;
    WAKE.notify_one();
}

/// Waits until `deadline` or a trigger; true for a trigger.
fn wait(deadline: Instant) -> bool {
    let mut t = TRIGGER.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if *t {
            *t = false;
            return true;
        }
        let now = Instant::now();
        if now >= deadline {
            return false;
        }
        t = WAKE
            .wait_timeout(t, deadline - now)
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
}

/// What the job task needs from the rest of the firmware.
pub trait JobEnv {
    /// Suspends the task if the board is shut down (overheat etc.).
    fn suspend_if_shutdown(&mut self);
    fn job_interval_ms(&mut self) -> u32;
    /// The pool to mine for next; None while there is no pool manager or
    /// no ASIC chain.
    fn next_pool(&mut self) -> Option<c_int>;
    fn select_asic_diff(&mut self, pool: c_int, pool_diff: u32) -> u32;
    fn set_difficulty_mask(&mut self, difficulty: u32);
    /// Sends the job to the chain; the job ID the chips will answer with.
    fn send_work(&mut self, extranonce2: u32, job: &BmJob) -> u8;
    fn slave_active(&mut self, slave: u8) -> bool;
    fn send_slave_job(&mut self, slave: u8, job_id: u8, job: &BmJob);
}

/// extranonce2 of a CAN slave: slave ID in the top 7 bits (`can_sender.h`).
pub fn slave_extranonce2(slave: u8, counter: u32) -> u32 {
    ((slave as u32 & 0x7f) << 25) | (counter & 0x01ff_ffff)
}

/// The job task's counters.
pub struct JobTask {
    extranonce2: u32,
    slave_counters: [u32; CAN_SLAVE_MAX],
    last_send: Option<Instant>,
}

impl Default for JobTask {
    fn default() -> Self {
        Self {
            extranonce2: 0,
            slave_counters: [0; CAN_SLAVE_MAX],
            last_send: None,
        }
    }
}

impl JobTask {
    /// One job: built for the next pool, sent to the chain and to every
    /// active CAN slave, and stored for the results. False without work.
    pub fn send_next(&mut self, env: &mut impl JobEnv) -> bool {
        let Some(pool) = env.next_pool() else {
            return false;
        };
        let Some(job) = jobs::build(pool, self.extranonce2, true, |d| {
            env.select_asic_diff(pool, d)
        }) else {
            return false;
        };
        env.set_difficulty_mask(job.asic_diff);
        self.last_send = Some(Instant::now());
        let id = env.send_work(self.extranonce2, &job);
        JOB_TABLES[0].store(job, id);
        self.extranonce2 = self.extranonce2.wrapping_add(1);

        for slave in 0..CAN_SLAVE_MAX as u8 {
            if !env.slave_active(slave) {
                continue;
            }
            let counter = &mut self.slave_counters[slave as usize];
            let e2 = slave_extranonce2(slave, *counter);
            *counter = counter.wrapping_add(1);
            if let Some(job) = jobs::build(pool, e2, false, |d| env.select_asic_diff(pool, d)) {
                env.send_slave_job(slave, id, &job);
                JOB_TABLES[1 + slave as usize].store(job, id);
            }
        }
        true
    }

    /// The task loop: a job every job interval and on new work.
    pub fn run(&mut self, env: &mut impl JobEnv) -> ! {
        let mut interval = env.job_interval_ms();
        let mut deadline = Instant::now() + Duration::from_millis(interval as u64);
        loop {
            env.suspend_if_shutdown();
            if !wait(deadline) {
                deadline += Duration::from_millis(interval as u64);
                // behind (the task was busy): one job now, then the period
                let now = Instant::now();
                if deadline < now {
                    deadline = now + Duration::from_millis(interval as u64);
                }
            }
            // job interval changed via the UI
            let current = env.job_interval_ms();
            if current != interval {
                interval = current;
                deadline = Instant::now() + Duration::from_millis(interval as u64);
                continue;
            }
            self.send_next(env);
        }
    }
}

// ---------------------------------------------------------------------------
// Results

/// `task_result`
#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct TaskResult {
    pub job_id: u8,
    pub nonce: u32,
    pub rolled_version: u32,
    pub asic_nr: c_int,
    pub data: u32,
    pub reg: u8,
    pub is_reg_resp: u8,
}

const _: () = assert!(core::mem::size_of::<TaskResult>() == 24);

/// What the result task needs from the rest of the firmware.
pub trait ResultEnv {
    fn suspend_if_shutdown(&mut self);
    /// The next response of the chain; None on a timeout.
    fn process_work(&mut self) -> Option<TaskResult>;
    fn set_chip_temp(&mut self, asic_nr: c_int, temp: f32);
    fn asic_max_difficulty(&mut self) -> u32;
    /// The best session difficulty as "1.23M" (for the log).
    fn best_session_diff(&mut self) -> String;
    /// Counts a share for the hashrate (per chip).
    fn push_share(&mut self, asic_nr: c_int);
    #[allow(clippy::too_many_arguments)]
    fn submit_share(
        &mut self,
        pool: c_int,
        job_id: &CStr,
        extranonce2: &CStr,
        ntime: u32,
        nonce: u32,
        version_rolled: u32,
        version: u32,
    );
    /// Best difficulty and found block check.
    fn check_difficulty(&mut self, pool: c_int, difficulty: f64, nbits: u32);
}

/// The last 32 nonce/version pairs (`SimpleRing64<32>`), to spot duplicates.
#[derive(Default)]
pub struct Seen {
    buf: [u64; 32],
    head: usize,
    len: usize,
}

impl Seen {
    /// False if `key` is among the last 32; otherwise remembers it.
    pub fn insert(&mut self, key: u64) -> bool {
        let start = (self.head + 32 - self.len) % 32;
        if (0..self.len).any(|i| self.buf[(start + i) % 32] == key) {
            return false;
        }
        self.buf[self.head] = key;
        self.head = (self.head + 1) % 32;
        self.len = (self.len + 1).min(32);
        true
    }
}

static DUPLICATES: Mutex<u64> = Mutex::new(0);

/// Nonces the chips returned twice (`getDuplicateHWNonces()`).
pub fn duplicates() -> u64 {
    *DUPLICATES.lock().unwrap_or_else(|e| e.into_inner())
}

/// Chip temperature from register 0xb4 (valid if bit 31 is set).
pub fn chip_temp(data: u32) -> Option<f32> {
    (data & 0x8000_0000 != 0).then_some((data & 0xffff) as f32 * 0.171342 - 299.5144)
}

const TAG: &CStr = c"asic_result";

/// The result task's state.
#[derive(Default)]
pub struct ResultTask {
    seen: Seen,
}

impl ResultTask {
    /// Handles one response of the chain.
    pub fn handle(&mut self, env: &mut impl ResultEnv, r: TaskResult) {
        if r.is_reg_resp != 0 {
            match r.reg {
                0xb4 => {
                    if let Some(t) = chip_temp(r.data) {
                        log!(Info, TAG, "asic {} temp: {}", r.asic_nr, fixed(t as f64, 3));
                        env.set_chip_temp(r.asic_nr, t);
                    }
                }
                hashrate::REG_NONCE_TOTAL_CNT => hashrate::on_counter(r.asic_nr, r.data),
                hashrate::REG_HW_ERROR_CNT => hashrate::on_errors(r.asic_nr, r.data),
                _ => {}
            }
            return;
        }

        let Some(j) = JOB_TABLES[0].clone_job(r.job_id) else {
            return;
        };
        // the chips return the rolled bits only
        let version = r.rolled_version | j.version;
        let diff = nonce_difficulty(
            version,
            &j.prev_block_hash,
            &j.merkle_root,
            j.ntime,
            j.target,
            r.nonce,
        );
        let pool_str = if j.pool_id != 0 { "Sec" } else { "Pri" };
        let (job_id, extranonce2) = (j.job_id(), j.extranonce2());
        let max = env.asic_max_difficulty() as f64;

        // only nonces >= the ASIC maximum (or the pool difficulty, which can
        // be lower in dual pool mode) are logged
        if diff >= max || diff >= j.pool_diff as f64 {
            log!(
                Info,
                TAG,
                "({pool_str}) Job ID: {:02X} AsicNr: {} Ver: {:08X} Nonce {:08X}; Extranonce2 {} diff {}/{}/{}",
                r.job_id,
                r.asic_nr,
                version,
                r.nonce,
                extranonce2.to_string_lossy(),
                fixed(diff, 1),
                j.pool_diff,
                env.best_session_diff()
            );
        }

        let duplicate = !self.seen.insert(((r.nonce as u64) << 32) | version as u64);
        if duplicate {
            log!(Warn, TAG, "({pool_str}) duplicate share detected!");
            *DUPLICATES.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        }
        if !duplicate && diff >= max {
            env.push_share(r.asic_nr);
        }
        // duplicates go to the pool too (and are counted as rejected there)
        if diff >= j.pool_diff as f64 {
            env.submit_share(
                j.pool_id,
                job_id,
                extranonce2,
                j.ntime,
                r.nonce,
                version,
                j.version,
            );
        }
        env.check_difficulty(j.pool_id, diff, j.target);
    }

    pub fn run(&mut self, env: &mut impl ResultEnv) -> ! {
        loop {
            env.suspend_if_shutdown();
            if let Some(r) = env.process_work() {
                self.handle(env, r);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// C API

pub(crate) mod api {
    use super::*;

    pub fn asic_result_duplicates() -> u64 {
        duplicates()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining::jobs::test_support::fresh;
    use crate::pool::stratum_v2::session::{ExtendedJob, StandardJob};

    #[derive(Default)]
    struct Fake {
        pool: Option<c_int>,
        slaves: Vec<u8>,
        sent: Vec<(u32, u32)>,
        slave_jobs: Vec<(u8, u8)>,
        next_id: u8,
        temps: Vec<(c_int, f32)>,
        pushed: Vec<c_int>,
        submitted: Vec<(c_int, String, u32, u32)>,
        checked: Vec<(c_int, u32)>,
    }

    impl JobEnv for Fake {
        fn suspend_if_shutdown(&mut self) {}
        fn job_interval_ms(&mut self) -> u32 {
            1000
        }
        fn next_pool(&mut self) -> Option<c_int> {
            self.pool
        }
        fn select_asic_diff(&mut self, _pool: c_int, pool_diff: u32) -> u32 {
            pool_diff / 2
        }
        fn set_difficulty_mask(&mut self, _difficulty: u32) {}
        fn send_work(&mut self, extranonce2: u32, job: &BmJob) -> u8 {
            self.sent.push((extranonce2, job.asic_diff));
            self.next_id = self.next_id.wrapping_add(8);
            self.next_id
        }
        fn slave_active(&mut self, slave: u8) -> bool {
            self.slaves.contains(&slave)
        }
        fn send_slave_job(&mut self, slave: u8, job_id: u8, _job: &BmJob) {
            self.slave_jobs.push((slave, job_id));
        }
    }

    impl ResultEnv for Fake {
        fn suspend_if_shutdown(&mut self) {}
        fn process_work(&mut self) -> Option<TaskResult> {
            None
        }
        fn set_chip_temp(&mut self, asic_nr: c_int, temp: f32) {
            self.temps.push((asic_nr, temp));
        }
        fn asic_max_difficulty(&mut self) -> u32 {
            // every nonce counts in the tests
            0
        }
        fn best_session_diff(&mut self) -> String {
            "1k".into()
        }
        fn push_share(&mut self, asic_nr: c_int) {
            self.pushed.push(asic_nr);
        }
        fn submit_share(
            &mut self,
            pool: c_int,
            job_id: &CStr,
            _extranonce2: &CStr,
            _ntime: u32,
            nonce: u32,
            version_rolled: u32,
            _version: u32,
        ) {
            let id = job_id.to_string_lossy().into_owned();
            self.submitted.push((pool, id, nonce, version_rolled));
        }
        fn check_difficulty(&mut self, pool: c_int, _difficulty: f64, nbits: u32) {
            self.checked.push((pool, nbits));
        }
    }

    fn standard(pool: c_int, job_id: u32, pool_diff: u32) {
        let job = StandardJob {
            job_id,
            version: 0x2000_0000,
            merkle_root: [3; 32],
            prev_hash: [4; 32],
            ntime: 0x66f0_0000,
            nbits: 0x1703_a30c,
        };
        jobs::sv2_standard(pool, &job, false, 0x1fffe000, pool_diff);
    }

    #[test]
    fn a_job_goes_to_the_chain_the_slaves_and_the_tables() {
        let _g = fresh();
        let mut env = Fake {
            pool: Some(1),
            slaves: vec![2],
            ..Default::default()
        };
        let mut task = JobTask::default();
        // no work yet
        assert!(!task.send_next(&mut env));

        let ext = ExtendedJob {
            job_id: 3,
            version: 0,
            prev_hash: [0; 32],
            ntime: 1,
            nbits: 0,
            merkle_path: Vec::new(),
            coinbase_prefix: Vec::new(),
            coinbase_suffix: Vec::new(),
            extranonce_prefix: Vec::new(),
        };
        jobs::sv2_extended(1, ext, false, 4, 0x1fffe000, 1024);
        assert!(task.send_next(&mut env));
        assert!(task.send_next(&mut env));
        assert_eq!(env.sent, [(0, 512), (1, 512)]);
        assert_eq!(env.slave_jobs, [(2, 8), (2, 16)]);
        // the master and the slave table know the jobs, with their extranonce2
        let job = JOB_TABLES[0].clone_job(16).unwrap();
        let slave = JOB_TABLES[3].clone_job(16).unwrap();
        assert_eq!(job.extranonce2(), c"00000001");
        // slave 2, counter 1
        assert_eq!(slave.extranonce2(), c"04000001");
        // no pool manager yet: nothing
        env.pool = None;
        assert!(!task.send_next(&mut env));
    }

    #[test]
    fn slave_extranonce2_layout() {
        assert_eq!(slave_extranonce2(0x7f, 0xffff_ffff), 0xffff_ffff);
        assert_eq!(slave_extranonce2(0x81, 2), 0x0200_0002);
    }

    #[test]
    fn nonce_results_are_checked_submitted_and_deduplicated() {
        let _g = fresh();
        // pool difficulty 0: every nonce is a share
        standard(0, 42, 0);
        let mut env = Fake {
            pool: Some(0),
            ..Default::default()
        };
        let mut jt = JobTask::default();
        assert!(jt.send_next(&mut env));
        let id = env.sent.len() as u8 * 8;

        let mut rt = ResultTask::default();
        let r = TaskResult {
            job_id: id,
            nonce: 0x1234_5678,
            rolled_version: 0x0000_2000,
            ..Default::default()
        };
        let before = duplicates();
        rt.handle(&mut env, r);
        rt.handle(&mut env, r);
        // both are submitted (the pool rejects the duplicate), but the
        // duplicate is not counted for the hashrate
        assert_eq!(env.submitted.len(), 2);
        assert_eq!(env.checked, [(0, 0x1703_a30c); 2]);
        assert_eq!(env.pushed, [0]);
        assert_eq!(duplicates(), before + 1);
        for (pool, job_id, nonce, version) in &env.submitted {
            assert_eq!(
                (*pool, job_id.as_str(), *nonce, *version),
                (0, "42", 0x1234_5678, 0x2000_2000)
            );
        }

        // unknown job ID: ignored
        rt.handle(
            &mut env,
            TaskResult {
                job_id: 99,
                ..Default::default()
            },
        );
        assert_eq!(env.checked.len(), 2);
    }

    #[test]
    fn register_responses() {
        let _g = fresh();
        let mut env = Fake::default();
        let mut rt = ResultTask::default();
        let reg = |reg, data| TaskResult {
            is_reg_resp: 1,
            reg,
            data,
            asic_nr: 3,
            ..Default::default()
        };
        rt.handle(&mut env, reg(0xb4, 0x8000_0000 | 2100));
        // without the valid bit: ignored
        rt.handle(&mut env, reg(0xb4, 2100));
        assert_eq!(env.temps.len(), 1);
        assert!((env.temps[0].1 - 60.304).abs() < 0.01);
        assert_eq!(chip_temp(2100), None);
    }

    #[test]
    fn seen_ring_forgets_after_32() {
        let mut s = Seen::default();
        assert!(s.insert(1));
        assert!(!s.insert(1));
        for k in 2..=33 {
            assert!(s.insert(k));
        }
        // 1 dropped out
        assert!(s.insert(1));
        assert!(!s.insert(33));
    }

    #[test]
    fn trigger_wakes_the_wait() {
        let _g = fresh();
        trigger();
        assert!(wait(Instant::now() + Duration::from_secs(5)));
        assert!(!wait(Instant::now() + Duration::from_millis(5)));
    }
}
