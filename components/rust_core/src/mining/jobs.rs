//! The work the miner hands to the ASICs (port of `MiningInfoV1` in
//! `create_jobs_task.cpp`, `MiningInfoV2Standard/Extended` in
//! `mining_info_v2.cpp`, `create_jobs_sv2.cpp` and `AsicJobs` in
//! `asic_jobs.h`): per pool the latest work from the pool, built into
//! `bm_job`s on request, and the tables that map the job IDs sent to the
//! ASICs (and CAN slaves) back to their jobs.
//!
//! Each pool has a Stratum v1 work and, once used, an SV2 standard and an
//! SV2 extended channel work; the one updated last is the active one, as
//! the C++ code switched `miningInfo[pool]` between its instances.

use core::ffi::{c_char, c_int, CStr};
use std::ffi::CString;
use std::sync::Mutex;

use crate::log::log;
use crate::mining::JobHashes;
use crate::pool::stratum_v1::job::build_parts;
use crate::pool::stratum_v1::MiningNotify;
use crate::pool::stratum_v2::jobs::{hashes, ExtendedJob};
use crate::util::cutil::until_nul;

const TAG: &CStr = c"create_jobs_task";

/// Slots of a job table: the job IDs the drivers use are below 0x80.
pub const MAX_ASIC_JOBS: usize = 128;
/// Job tables: the local ASICs, then one per CAN slave.
pub const CAN_SLAVE_MAX: usize = 32;
pub const TABLES: usize = 1 + CAN_SLAVE_MAX;

/// `bm_job`
#[repr(C)]
#[derive(Clone, Copy)]
pub struct BmJob {
    pub version: u32,
    pub version_mask: u32,
    pub prev_block_hash: [u8; 32],
    pub prev_block_hash_be: [u8; 32],
    pub merkle_root: [u8; 32],
    pub merkle_root_be: [u8; 32],
    pub ntime: u32,
    pub target: u32,
    pub starting_nonce: u32,
    pub pool_diff: u32,
    pub asic_diff: u32,
    pub pool_id: c_int,
    pub jobid: *mut c_char,
    pub extranonce2: *mut c_char,
}

// Must match the static assertions in jobs.h.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<BmJob>() == 168);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<BmJob>() == 176);

/// The fields of a job before it becomes a [`Job`].
struct Built {
    version: u32,
    version_mask: u32,
    hashes: JobHashes,
    ntime: u32,
    nbits: u32,
    pool_diff: u32,
    job_id: Vec<u8>,
    extranonce2: Vec<u8>,
}

/// `bytes` as a C string, cut at the first NUL (as the C copy was).
fn c_string(bytes: &[u8]) -> CString {
    CString::new(until_nul(bytes)).unwrap_or_default()
}

/// A job of the pipeline: the `bm_job` the drivers send, with its own job ID
/// and extranonce2 (the `bm_job`'s string fields point at them).
pub struct Job {
    bm: BmJob,
    job_id: CString,
    extranonce2: CString,
}

// SAFETY: the only pointers in `bm` point into the job's own strings, which
// move with it (their heap buffers stay where they are).
unsafe impl Send for Job {}

impl Job {
    fn new(mut bm: BmJob, job_id: CString, extranonce2: CString) -> Self {
        bm.jobid = job_id.as_ptr() as *mut c_char;
        bm.extranonce2 = extranonce2.as_ptr() as *mut c_char;
        Self {
            bm,
            job_id,
            extranonce2,
        }
    }

    fn built(b: Built, pool: c_int, asic_diff: u32) -> Self {
        let bm = BmJob {
            version: b.version,
            version_mask: b.version_mask,
            prev_block_hash: b.hashes.prev_block_hash,
            prev_block_hash_be: b.hashes.prev_block_hash_be,
            merkle_root: b.hashes.merkle_root,
            merkle_root_be: b.hashes.merkle_root_be,
            ntime: b.ntime,
            target: b.nbits,
            starting_nonce: 0,
            pool_diff: b.pool_diff,
            asic_diff,
            pool_id: pool,
            jobid: core::ptr::null_mut(),
            extranonce2: core::ptr::null_mut(),
        };
        Self::new(bm, c_string(&b.job_id), c_string(&b.extranonce2))
    }

    pub fn job_id(&self) -> &CStr {
        &self.job_id
    }

    pub fn extranonce2(&self) -> &CStr {
        &self.extranonce2
    }
}

impl core::ops::Deref for Job {
    type Target = BmJob;
    fn deref(&self) -> &BmJob {
        &self.bm
    }
}

impl Clone for Job {
    fn clone(&self) -> Self {
        Self::new(self.bm, self.job_id.clone(), self.extranonce2.clone())
    }
}

// ---------------------------------------------------------------------------
// Job tables

/// Job ID → job; allocated on first use, so unused CAN slave tables cost
/// nothing.
pub struct Table(Mutex<Vec<Option<Job>>>);

impl Table {
    const fn new() -> Self {
        Self(Mutex::new(Vec::new()))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Vec<Option<Job>>) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Stores `job` under `id`, replacing the job still in the slot.
    pub fn store(&self, job: Job, id: u8) {
        if id as usize >= MAX_ASIC_JOBS {
            // not reachable: the drivers send IDs below 0x80
            return;
        }
        self.with(|t| {
            if t.is_empty() {
                t.resize_with(MAX_ASIC_JOBS, || None);
            }
            t[id as usize] = Some(job);
        });
    }

    /// A copy of the job sent with `id` (the ID comes from a chip or a CAN
    /// frame, 0..255).
    pub fn clone_job(&self, id: u8) -> Option<Job> {
        self.with(|t| t.get(id as usize).cloned().flatten())
    }

    /// Drops the jobs of `pool`; how many there were.
    pub fn clean(&self, pool: c_int) -> c_int {
        self.with(|t| {
            let mut n = 0;
            for slot in t.iter_mut() {
                if slot.as_ref().is_some_and(|j| j.pool_id == pool) {
                    *slot = None;
                    n += 1;
                }
            }
            n
        })
    }
}

pub static JOB_TABLES: [Table; TABLES] = [const { Table::new() }; TABLES];

fn master() -> &'static Table {
    &JOB_TABLES[0]
}

// ---------------------------------------------------------------------------
// Work per pool

/// Stratum v1 work: the last notify and the extranonce from the subscribe
/// result or `mining.set_extranonce` (active with the next notify).
struct V1 {
    job_id: Option<Vec<u8>>,
    coinbase_1: Option<Vec<u8>>,
    coinbase_2: Option<Vec<u8>>,
    prev_block_hash: [u8; 32],
    merkle_branches: Vec<[u8; 32]>,
    version: u32,
    nbits: u32,
    /// 0: no valid work
    ntime: u32,
    extranonce: Option<Vec<u8>>,
    extranonce2_len: i32,
    next_extranonce: Option<(Vec<u8>, i32)>,
    difficulty: u32,
    /// The difficulty of the last notify (`mining.set_difficulty` applies to
    /// the next notify)
    active_difficulty: u32,
    version_mask: u32,
}

impl V1 {
    const fn new() -> Self {
        Self {
            job_id: None,
            coinbase_1: None,
            coinbase_2: None,
            prev_block_hash: [0; 32],
            merkle_branches: Vec::new(),
            version: 0,
            nbits: 0,
            ntime: 0,
            extranonce: None,
            extranonce2_len: 0,
            next_extranonce: None,
            difficulty: 8192,
            active_difficulty: 8192,
            version_mask: 0,
        }
    }

    fn is_valid(&self) -> bool {
        // a job needs both the notify and the extranonce1 from the subscribe
        // result; after invalidate() a notify can arrive without one
        self.ntime != 0
            && self.extranonce.is_some()
            && self.job_id.is_some()
            && self.coinbase_1.is_some()
            && self.coinbase_2.is_some()
    }

    fn invalidate(&mut self) {
        self.ntime = 0;
        self.extranonce = None;
        self.next_extranonce = None;
        self.job_id = None;
        self.coinbase_1 = None;
        self.coinbase_2 = None;
    }

    fn notify(&mut self, n: &MiningNotify) {
        // a pending extranonce switch takes effect now
        if let Some((e, len)) = self.next_extranonce.take() {
            self.extranonce = Some(e);
            self.extranonce2_len = len;
        }
        self.job_id = Some(n.job_id.clone());
        self.coinbase_1 = Some(n.coinbase_1.clone());
        self.coinbase_2 = Some(n.coinbase_2.clone());
        self.prev_block_hash = n.prev_block_hash;
        self.merkle_branches = n.merkle_branches.clone();
        self.version = n.version;
        self.nbits = n.target;
        self.ntime = n.ntime;
        self.active_difficulty = self.difficulty;
    }

    fn build(&self, extranonce2: u32) -> Option<Built> {
        let (Some(job_id), Some(cb1), Some(cb2), Some(en1)) = (
            &self.job_id,
            &self.coinbase_1,
            &self.coinbase_2,
            &self.extranonce,
        ) else {
            return None;
        };
        let Some((hashes, en2)) = build_parts(
            &self.prev_block_hash,
            &self.merkle_branches,
            [cb1, en1, cb2],
            self.extranonce2_len,
            extranonce2,
        ) else {
            log!(
                Error,
                TAG,
                "Unable to build job (extranonce2_len {})",
                self.extranonce2_len
            );
            return None;
        };
        Some(Built {
            version: self.version,
            version_mask: self.version_mask,
            hashes,
            ntime: self.ntime,
            nbits: self.nbits,
            pool_diff: self.active_difficulty,
            job_id: job_id.clone(),
            extranonce2: en2.into_bytes(),
        })
    }
}

/// SV2 standard channel: the pool sends merkle root and prevhash; a job is
/// sent to the ASICs once (version rolling covers the rest).
struct Standard {
    job_id: u32,
    version: u32,
    merkle_root: [u8; 32],
    prev_hash: [u8; 32],
    ntime: u32,
    nbits: u32,
    version_mask: u32,
    difficulty: u32,
    sent: bool,
}

impl Standard {
    fn build(&mut self) -> Built {
        // never resend a standard channel job
        self.sent = true;
        Built {
            version: self.version,
            version_mask: self.version_mask,
            hashes: hashes(self.merkle_root, &self.prev_hash),
            ntime: self.ntime,
            nbits: self.nbits,
            pool_diff: self.difficulty,
            job_id: self.job_id.to_string().into_bytes(),
            extranonce2: Vec::new(),
        }
    }
}

/// SV2 extended channel: the miner builds the coinbase from the pool's
/// prefix, extranonce prefix, its extranonce2 and the suffix.
struct Extended {
    job_id: u32,
    version: u32,
    prev_hash: [u8; 32],
    ntime: u32,
    nbits: u32,
    version_mask: u32,
    difficulty: u32,
    coinbase_prefix: Vec<u8>,
    coinbase_suffix: Vec<u8>,
    extranonce_prefix: Vec<u8>,
    extranonce_size: u8,
    merkle_path: Vec<[u8; 32]>,
}

impl Extended {
    fn build(&self, extranonce2: u32) -> Option<Built> {
        let job = ExtendedJob {
            coinbase_prefix: &self.coinbase_prefix,
            extranonce_prefix: &self.extranonce_prefix,
            extranonce_size: self.extranonce_size as usize,
            coinbase_suffix: &self.coinbase_suffix,
            merkle_path: &self.merkle_path,
            prev_hash: &self.prev_hash,
        };
        let Some((hashes, en2)) = job.build(extranonce2) else {
            log!(
                Error,
                c"mining_info_v2",
                "Unable to build extended job (extranonce_size {})",
                self.extranonce_size
            );
            return None;
        };
        Some(Built {
            version: self.version,
            version_mask: self.version_mask,
            hashes,
            ntime: self.ntime,
            nbits: self.nbits,
            pool_diff: self.difficulty,
            job_id: self.job_id.to_string().into_bytes(),
            // sent with the share
            extranonce2: crate::util::cutil::to_hex(&en2).into_bytes(),
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Active {
    V1,
    Standard,
    Extended,
}

struct PoolWork {
    v1: V1,
    standard: Option<Standard>,
    extended: Option<Extended>,
    active: Active,
    /// ntime of the last job built for the log ("New Work Received")
    last_ntime: u32,
}

impl PoolWork {
    const fn new() -> Self {
        Self {
            v1: V1::new(),
            standard: None,
            extended: None,
            active: Active::V1,
            last_ntime: 0,
        }
    }

    /// The v1 work, made the active one (a pool switched from SV2 to v1
    /// starts with an invalidated v1 work).
    fn v1(&mut self) -> &mut V1 {
        if self.active != Active::V1 {
            self.v1.invalidate();
            self.active = Active::V1;
        }
        &mut self.v1
    }

    fn is_valid(&self) -> bool {
        match self.active {
            Active::V1 => self.v1.is_valid(),
            Active::Standard => self
                .standard
                .as_ref()
                .is_some_and(|s| s.ntime != 0 && !s.sent),
            Active::Extended => self.extended.as_ref().is_some_and(|e| e.ntime != 0),
        }
    }

    fn ntime(&self) -> u32 {
        match self.active {
            Active::V1 => self.v1.ntime,
            Active::Standard => self.standard.as_ref().map_or(0, |s| s.ntime),
            Active::Extended => self.extended.as_ref().map_or(0, |e| e.ntime),
        }
    }

    fn job_id(&self) -> String {
        match self.active {
            Active::V1 => self
                .v1
                .job_id
                .as_deref()
                .map(|j| String::from_utf8_lossy(until_nul(j)).into_owned())
                .unwrap_or_default(),
            Active::Standard => self.standard.as_ref().map_or(0, |s| s.job_id).to_string(),
            Active::Extended => self.extended.as_ref().map_or(0, |e| e.job_id).to_string(),
        }
    }

    fn active_difficulty(&self) -> u32 {
        match self.active {
            Active::V1 => self.v1.active_difficulty,
            Active::Standard => self.standard.as_ref().map_or(0, |s| s.difficulty),
            Active::Extended => self.extended.as_ref().map_or(0, |e| e.difficulty),
        }
    }

    fn invalidate(&mut self) {
        match self.active {
            Active::V1 => self.v1.invalidate(),
            Active::Standard => {
                if let Some(s) = &mut self.standard {
                    s.ntime = 0;
                    s.job_id = 0;
                }
            }
            Active::Extended => {
                if let Some(e) = &mut self.extended {
                    e.ntime = 0;
                    e.job_id = 0;
                    e.coinbase_prefix = Vec::new();
                    e.coinbase_suffix = Vec::new();
                }
            }
        }
    }

    fn build(&mut self, extranonce2: u32) -> Option<Built> {
        match self.active {
            Active::V1 => self.v1.build(extranonce2),
            Active::Standard => self.standard.as_mut().map(Standard::build),
            Active::Extended => self.extended.as_ref()?.build(extranonce2),
        }
    }
}

static WORK: Mutex<[PoolWork; 2]> = Mutex::new([PoolWork::new(), PoolWork::new()]);

fn with_pool<T>(pool: c_int, f: impl FnOnce(&mut PoolWork) -> T) -> Option<T> {
    let i = usize::try_from(pool).ok().filter(|&i| i < 2)?;
    let mut w = WORK.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&mut w[i]))
}

fn pool_str(pool: c_int) -> &'static str {
    if pool != 0 {
        "Sec"
    } else {
        "Pri"
    }
}

/// Wakes the job creation task.
fn trigger() {
    #[cfg(not(target_os = "espidf"))]
    TRIGGERS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    crate::mining::tasks::trigger();
}

#[cfg(not(target_os = "espidf"))]
pub static TRIGGERS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// --- Stratum v1 (from the pool dispatch)

/// `mining.notify`; `abandon` drops the pool's jobs first.
pub fn v1_notify(pool: c_int, notify: &MiningNotify, abandon: bool) {
    if abandon {
        master().clean(pool);
    }
    with_pool(pool, |w| w.v1().notify(notify));
    trigger();
}

/// `mining.set_difficulty`; true if it changed (active with the next notify).
pub fn v1_set_difficulty(pool: c_int, difficulty: u32) -> bool {
    with_pool(pool, |w| {
        let v1 = w.v1();
        let changed = v1.difficulty != difficulty;
        v1.difficulty = difficulty;
        changed
    })
    .unwrap_or(false)
}

pub fn v1_set_version_mask(pool: c_int, mask: u32) {
    with_pool(pool, |w| w.v1().version_mask = mask);
}

/// The subscribe result's extranonce.
pub fn v1_set_extranonce(pool: c_int, extranonce: Option<&[u8]>, extranonce2_len: i32) {
    with_pool(pool, |w| {
        let v1 = w.v1();
        v1.extranonce = extranonce.map(<[u8]>::to_vec);
        v1.extranonce2_len = extranonce2_len;
    });
}

/// `mining.set_extranonce`: active with the next notify.
pub fn v1_set_next_extranonce(pool: c_int, extranonce: Option<&[u8]>, extranonce2_len: i32) {
    with_pool(pool, |w| {
        w.v1().next_extranonce = extranonce.map(|e| (e.to_vec(), extranonce2_len));
    });
}

// --- Stratum v2

/// A standard channel job (merkle root and prevhash from the pool).
pub fn sv2_standard(
    pool: c_int,
    job: &crate::pool::stratum_v2::session::StandardJob,
    clean: bool,
    version_mask: u32,
    difficulty: u32,
) {
    let ok = with_pool(pool, |w| {
        w.standard = Some(Standard {
            job_id: job.job_id,
            version: job.version,
            merkle_root: job.merkle_root,
            prev_hash: job.prev_hash,
            ntime: job.ntime,
            nbits: job.nbits,
            version_mask,
            difficulty,
            sent: false,
        });
        w.active = Active::Standard;
    });
    if ok.is_none() {
        return;
    }
    if clean {
        master().clean(pool);
    }
    log!(
        Info,
        c"create_jobs_sv2",
        "({}) SV2 standard job {} ready, diff={}",
        pool_str(pool),
        job.job_id,
        difficulty
    );
    trigger();
}

/// An extended channel job.
pub fn sv2_extended(
    pool: c_int,
    mut job: crate::pool::stratum_v2::session::ExtendedJob,
    clean: bool,
    extranonce_size: u8,
    version_mask: u32,
    difficulty: u32,
) {
    job.merkle_path
        .truncate(crate::pool::stratum_v2::protocol::MAX_MERKLE_BRANCHES);
    job.extranonce_prefix.truncate(32);
    let job_id = job.job_id;
    let ok = with_pool(pool, |w| {
        w.extended = Some(Extended {
            job_id: job.job_id,
            version: job.version,
            prev_hash: job.prev_hash,
            ntime: job.ntime,
            nbits: job.nbits,
            version_mask,
            difficulty,
            coinbase_prefix: job.coinbase_prefix,
            coinbase_suffix: job.coinbase_suffix,
            extranonce_prefix: job.extranonce_prefix,
            extranonce_size,
            merkle_path: job.merkle_path,
        });
        w.active = Active::Extended;
    });
    if ok.is_none() {
        return;
    }
    if clean {
        master().clean(pool);
    }
    log!(
        Info,
        c"create_jobs_sv2",
        "({}) SV2 extended job {} ready, diff={}",
        pool_str(pool),
        job_id,
        difficulty
    );
    trigger();
}

/// `SetTarget`: the standard channel keeps its job (the difficulty applies
/// to the next job from the pool); the extended channel builds new jobs.
pub fn sv2_set_difficulty(pool: c_int, difficulty: u32) {
    let resend = with_pool(pool, |w| {
        if let Some(s) = &mut w.standard {
            s.difficulty = difficulty;
            Some(("standard", false))
        } else if let Some(e) = &mut w.extended {
            e.difficulty = difficulty;
            Some(("extended", true))
        } else {
            None
        }
    })
    .flatten();
    if let Some((kind, resend)) = resend {
        log!(
            Info,
            c"create_jobs_sv2",
            "({}) SV2 {kind} difficulty updated to {difficulty}",
            pool_str(pool)
        );
        if resend {
            trigger();
        }
    }
}

/// The pool's work is no longer valid (disconnected, switched away).
pub fn invalidate(pool: c_int) {
    with_pool(pool, PoolWork::invalidate);
    master().clean(pool);
}

/// The next job for `pool`, or None without valid work. `select_diff` maps
/// the pool difficulty to the ASIC difficulty; `log_new_work` logs when the
/// work changed since the last such call.
pub fn build(
    pool: c_int,
    extranonce2: u32,
    log_new_work: bool,
    select_diff: impl FnOnce(u32) -> u32,
) -> Option<Job> {
    with_pool(pool, |w| {
        if !w.is_valid() {
            return None;
        }
        if log_new_work && w.last_ntime != w.ntime() {
            w.last_ntime = w.ntime();
            log!(
                Info,
                TAG,
                "({}) New Work Received {}",
                pool_str(pool),
                w.job_id()
            );
        }
        let asic_diff = select_diff(w.active_difficulty());
        w.build(extranonce2).map(|b| Job::built(b, pool, asic_diff))
    })
    .flatten()
}

/// The work and the tables are global: tests that use them hold this lock.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    static SERIAL: Mutex<()> = Mutex::new(());

    pub(crate) fn reset() {
        *WORK.lock().unwrap_or_else(|e| e.into_inner()) = [PoolWork::new(), PoolWork::new()];
        for t in &JOB_TABLES {
            t.with(Vec::clear);
        }
    }

    /// Locks the global job state and starts it empty.
    pub(crate) fn fresh() -> std::sync::MutexGuard<'static, ()> {
        let g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        g
    }

    /// The fields of a built job the tests look at.
    pub(crate) fn take(job: Option<Job>) -> (u32, u32, String, String, c_int, u32) {
        let j = job.expect("a job");
        (
            j.pool_diff,
            j.asic_diff,
            j.job_id().to_string_lossy().into_owned(),
            j.extranonce2().to_string_lossy().into_owned(),
            j.pool_id,
            j.version_mask,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::stratum_v2::session::{ExtendedJob, StandardJob};

    use super::test_support::{fresh as serial, take};

    fn notify(job_id: &[u8], cb1: &[u8], cb2: &[u8]) -> MiningNotify {
        MiningNotify {
            job_id: job_id.to_vec(),
            coinbase_1: cb1.to_vec(),
            coinbase_2: cb2.to_vec(),
            version: 0x2000_0000,
            target: 0x1703_a30c,
            ntime: 0x66f0_0000,
            ..Default::default()
        }
    }

    #[test]
    fn v1_needs_notify_and_extranonce() {
        let _s = serial();
        let (id, cb1, cb2) = (b"1d2e", b"0100", b"ffff");
        let n = notify(id, cb1, cb2);
        v1_notify(0, &n, false);
        assert!(build(0, 0, false, |d| d).is_none());

        v1_set_extranonce(0, Some(b"aabbccdd"), 4);
        // set_difficulty applies with the next notify
        assert!(v1_set_difficulty(0, 4096));
        assert!(!v1_set_difficulty(0, 4096));
        let job = build(0, 7, true, |d| {
            assert_eq!(d, 8192);
            512
        });
        assert_eq!(
            take(job),
            (8192, 512, "1d2e".into(), "00000007".into(), 0, 0)
        );

        v1_notify(0, &n, false);
        assert_eq!(take(build(0, 1, false, |d| d)).0, 4096);
    }

    #[test]
    fn v1_next_extranonce_waits_for_the_notify() {
        let _s = serial();
        let (id, cb1, cb2) = (b"1", b"01", b"02");
        let n = notify(id, cb1, cb2);
        v1_set_extranonce(1, Some(b"aa"), 2);
        v1_notify(1, &n, false);
        v1_set_next_extranonce(1, Some(b"bb"), 3);
        assert_eq!(take(build(1, 1, false, |d| d)).3, "0001");
        v1_notify(1, &n, false);
        assert_eq!(take(build(1, 1, false, |d| d)).3, "000001");
    }

    fn std_job(job_id: u32) -> StandardJob {
        StandardJob {
            job_id,
            version: 0x2000_0000,
            merkle_root: [1; 32],
            prev_hash: [2; 32],
            ntime: 0x66f0_0000,
            nbits: 0x1703_a30c,
        }
    }

    #[test]
    fn standard_job_is_sent_once_and_switching_back_to_v1_invalidates() {
        let _s = serial();
        sv2_standard(0, &std_job(42), false, 0x1fffe000, 1024);
        assert_eq!(
            take(build(0, 0, false, |d| d)),
            (1024, 1024, "42".into(), "".into(), 0, 0x1fffe000)
        );
        assert!(build(0, 0, false, |d| d).is_none());
        // SetTarget: no resend for the standard channel
        let before = TRIGGERS.load(core::sync::atomic::Ordering::Relaxed);
        sv2_set_difficulty(0, 2048);
        assert_eq!(TRIGGERS.load(core::sync::atomic::Ordering::Relaxed), before);
        sv2_standard(0, &std_job(43), false, 0x1fffe000, 2048);
        assert_eq!(take(build(0, 0, false, |d| d)).2, "43");

        // a v1 message makes v1 active again, starting invalid
        v1_set_version_mask(0, 0x1fffe000);
        assert!(build(0, 0, false, |d| d).is_none());
    }

    #[test]
    fn extended_job_builds_per_extranonce2() {
        let _s = serial();
        let ext = ExtendedJob {
            job_id: 7,
            version: 0,
            prev_hash: [0; 32],
            ntime: 1,
            nbits: 0,
            merkle_path: Vec::new(),
            coinbase_prefix: vec![0xab; 4],
            coinbase_suffix: vec![0xcd; 4],
            extranonce_prefix: vec![1, 2],
        };
        sv2_extended(1, ext, false, 6, 0x1fffe000, 512);
        let a = build(1, 1, false, |d| d);
        let b = build(1, 2, false, |d| d);
        let (ra, rb) = (
            a.as_ref().unwrap().merkle_root,
            b.as_ref().unwrap().merkle_root,
        );
        assert_ne!(ra, rb);
        assert_eq!(take(a).3, "000000000001");
        take(b);
        invalidate(1);
        assert!(build(1, 1, false, |d| d).is_none());
    }

    #[test]
    fn job_strings_follow_the_job() {
        let _s = serial();
        sv2_standard(0, &std_job(77), false, 0, 1);
        let job = build(0, 0, false, |d| d).unwrap();
        // moved and cloned: the C view still points at the job's own strings
        let moved = vec![job];
        let copy = moved[0].clone();
        drop(moved);
        assert_eq!(copy.bm.jobid as *const c_char, copy.job_id().as_ptr());
        assert_eq!(
            copy.bm.extranonce2 as *const c_char,
            copy.extranonce2().as_ptr()
        );
        // SAFETY: points at the job's own string (asserted above)
        assert_eq!(unsafe { CStr::from_ptr(copy.bm.jobid) }, c"77");
    }

    #[test]
    fn table_stores_clones_and_cleans() {
        let _s = serial();
        let t = &JOB_TABLES[3];
        sv2_standard(1, &std_job(9), false, 0, 1);
        let job = build(1, 0, false, |d| d);
        t.store(job.unwrap(), 5);
        let c = t.clone_job(5);
        assert_eq!(take(c).2, "9");
        assert!(t.clone_job(6).is_none());
        assert!(t.clone_job(200).is_none());
        // out-of-range ID: freed, not stored
        sv2_standard(1, &std_job(10), false, 0, 1);
        t.store(build(1, 0, false, |d| d).unwrap(), 200);
        assert_eq!(t.clean(0), 0);
        assert_eq!(t.clean(1), 1);
        assert!(t.clone_job(5).is_none());
    }

    #[test]
    fn clean_flag_and_abandon_clear_the_master_table() {
        let _s = serial();
        sv2_standard(0, &std_job(1), false, 0, 1);
        master().store(build(0, 0, false, |d| d).unwrap(), 1);
        sv2_standard(0, &std_job(2), true, 0, 1);
        assert!(master().clone_job(1).is_none());

        let (id, cb1, cb2) = (b"1", b"01", b"02");
        let n = notify(id, cb1, cb2);
        v1_set_extranonce(0, Some(b"aa"), 2);
        v1_notify(0, &n, false);
        master().store(build(0, 0, false, |d| d).unwrap(), 2);
        v1_notify(0, &n, true);
        assert!(master().clone_job(2).is_none());
    }
}
