//! Stratum v2 session state (port of the message handling in
//! `main/stratum/stratum_task_v2.cpp`): channel setup checks, pending jobs
//! waiting for their prevhash, the extranonce prefix in force per job, the
//! target, share submission. The connection loop (`connection.rs`) feeds
//! received frames in; the pool manager turns the resulting [`Event`]s into
//! calls to the statistics and the job pipeline (`pool/dispatch.rs`).
//!
//! Behaviour notes:
//! * an extended channel's `extranonce_size` (a u16 from the pool) must be
//!   1..=32: 0 would give identical jobs, more does not fit the submit,
//! * the extranonce2 of a share is decoded like `hex2bin` (invalid digits are
//!   0).

use std::collections::VecDeque;

use super::protocol as p;
use crate::log::log;

pub const PENDING_JOBS: usize = 8;
const MSG_SETUP_CONNECTION_SUCCESS: u8 = 0x01;
const MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS: u8 = 0x11;
const MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS: u8 = 0x14;
const MSG_NEW_MINING_JOB: u8 = 0x15;
const MSG_NEW_EXTENDED_MINING_JOB: u8 = 0x1f;
const MSG_SUBMIT_SHARES_SUCCESS: u8 = 0x1c;
const MSG_SUBMIT_SHARES_ERROR: u8 = 0x1d;
const MSG_SET_EXTRANONCE_PREFIX: u8 = 0x19;
const MSG_SET_NEW_PREV_HASH: u8 = 0x20;
const MSG_SET_TARGET: u8 = 0x21;
const SETUP_SUCCESS_REQUIRES_FIXED_VERSION: u32 = 1 << 0;
/// Accepted extended channel extranonce sizes (B0_32 in the submit).
pub const EXTRANONCE_SIZE: core::ops::RangeInclusive<u16> = 1..=32;
/// The C++ code capped a suspicious accepted count to this.
const MAX_ACCEPTED_PER_MESSAGE: u32 = 1000;

const TAG: &core::ffi::CStr = c"stratum_v2";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrevHash {
    pub hash: [u8; 32],
    pub ntime: u32,
    pub nbits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StandardJob {
    pub job_id: u32,
    pub version: u32,
    pub merkle_root: [u8; 32],
    pub prev_hash: [u8; 32],
    pub ntime: u32,
    pub nbits: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtendedJob {
    pub job_id: u32,
    pub version: u32,
    pub prev_hash: [u8; 32],
    /// min_ntime, 0 for a future job until its prevhash arrives
    pub ntime: u32,
    pub nbits: u32,
    pub merkle_path: Vec<[u8; 32]>,
    pub coinbase_prefix: Vec<u8>,
    pub coinbase_suffix: Vec<u8>,
    /// The prefix in force when the job arrived (spec 5.3.10: a change
    /// applies to jobs sent after it).
    pub extranonce_prefix: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// nbits of a new prevhash
    NetworkDifficulty(u32),
    /// pool difficulty of the opened channel
    PoolDifficulty(u32),
    /// pool difficulty from SetTarget
    SetTarget(u32),
    StandardJob {
        job: StandardJob,
        pool_difficulty: u32,
        clean: bool,
    },
    ExtendedJob {
        job: ExtendedJob,
        extranonce_size: u8,
        pool_difficulty: u32,
        clean: bool,
    },
    SharesAccepted(u32),
    ShareRejected(Vec<u8>),
    /// The connection is unusable (e.g. the pool forbids version rolling).
    SetupFailed,
    /// A prevhash arrived: the pool sends valid work.
    ValidNotify,
}

pub struct Session {
    extended: bool,
    channel_id: u32,
    sequence_number: u32,
    target: [u8; 32],
    prev: Option<PrevHash>,
    extranonce_prefix: Vec<u8>,
    extranonce_size: u8,
    pending_standard: [Option<(u32, u32, [u8; 32])>; PENDING_JOBS],
    pending_extended: [Option<ExtendedJob>; PENDING_JOBS],
    pub(crate) events: VecDeque<Event>,
}

impl Session {
    pub fn new(extended: bool) -> Self {
        Self {
            extended,
            channel_id: 0,
            sequence_number: 0,
            target: [0; 32],
            prev: None,
            extranonce_prefix: Vec::new(),
            extranonce_size: 0,
            pending_standard: Default::default(),
            pending_extended: Default::default(),
            events: VecDeque::new(),
        }
    }

    pub fn next_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    fn pool_difficulty(&self) -> u32 {
        p::target_to_pdiff(&self.target)
    }

    /// The answer to SetupConnection.
    pub fn setup_success(&mut self, msg_type: u8, payload: &[u8]) -> bool {
        if msg_type != MSG_SETUP_CONNECTION_SUCCESS {
            log!(
                Error,
                TAG,
                "SetupConnection rejected by pool (msg_type=0x{msg_type:02x})"
            );
            return false;
        }
        let Some((version, flags)) = p::setup_connection_success(payload) else {
            log!(Error, TAG, "Failed to parse SetupConnectionSuccess");
            return false;
        };
        if flags & SETUP_SUCCESS_REQUIRES_FIXED_VERSION != 0 {
            log!(
                Error,
                TAG,
                "Pool requires a fixed block version, but the ASICs always roll version bits (flags=0x{flags:08x})"
            );
            return false;
        }
        log!(
            Info,
            TAG,
            "Pool accepted connection: SV2 version={version}, flags=0x{flags:08x}"
        );
        true
    }

    /// The answer to OpenStandard/ExtendedMiningChannel.
    pub fn open_channel_success(&mut self, msg_type: u8, payload: &[u8]) -> bool {
        let expected = if self.extended {
            MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS
        } else {
            MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS
        };
        if msg_type != expected {
            log!(Error, TAG, "OpenChannel rejected by pool (msg_type=0x{msg_type:02x}, expected=0x{expected:02x})");
            return false;
        }
        let Some(s) = p::open_channel_success(payload, self.extended) else {
            log!(Error, TAG, "Failed to parse OpenChannelSuccess");
            return false;
        };
        if self.extended {
            if !EXTRANONCE_SIZE.contains(&s.extranonce_size) {
                log!(
                    Error,
                    TAG,
                    "Pool assigned an unusable extranonce_size {} (1..=32)",
                    s.extranonce_size
                );
                return false;
            }
            self.extranonce_size = s.extranonce_size as u8;
            self.extranonce_prefix = s.extranonce_prefix.to_vec();
            log!(
                Info,
                TAG,
                "Extended channel: extranonce_size={}, prefix_len={}, prefix={}",
                s.extranonce_size,
                s.extranonce_prefix.len(),
                hex(s.extranonce_prefix)
            );
        }
        self.channel_id = s.channel_id;
        self.target = s.target;
        let pdiff = self.pool_difficulty();
        log!(
            Info,
            TAG,
            "Mining channel opened: channel_id={}, type={}, difficulty={pdiff}",
            s.channel_id,
            if self.extended {
                "extended"
            } else {
                "standard"
            }
        );
        self.events.push_back(Event::PoolDifficulty(pdiff));
        true
    }

    /// A frame from the main loop.
    pub fn handle(&mut self, msg_type: u8, payload: &[u8]) {
        match msg_type {
            MSG_NEW_MINING_JOB => self.new_mining_job(payload),
            MSG_NEW_EXTENDED_MINING_JOB => self.new_extended_job(payload),
            MSG_SET_EXTRANONCE_PREFIX => self.set_extranonce_prefix(payload),
            MSG_SET_NEW_PREV_HASH => self.set_new_prev_hash(payload),
            MSG_SET_TARGET => self.set_target(payload),
            MSG_SUBMIT_SHARES_SUCCESS => self.submit_success(payload),
            MSG_SUBMIT_SHARES_ERROR => self.submit_error(payload),
            _ => log!(
                Warn,
                TAG,
                "Unknown SV2 message type: 0x{msg_type:02x} (len={})",
                payload.len()
            ),
        }
    }

    fn emit_standard(&mut self, job: StandardJob) {
        let pool_difficulty = self.pool_difficulty();
        self.events.push_back(Event::StandardJob {
            job,
            pool_difficulty,
            clean: true,
        });
    }

    fn emit_extended(&mut self, job: ExtendedJob) {
        let pool_difficulty = self.pool_difficulty();
        let extranonce_size = self.extranonce_size;
        self.events.push_back(Event::ExtendedJob {
            job,
            extranonce_size,
            pool_difficulty,
            clean: true,
        });
    }

    fn new_mining_job(&mut self, payload: &[u8]) {
        let Some(j) = p::new_mining_job(payload) else {
            log!(Error, TAG, "Failed to parse NewMiningJob");
            return;
        };
        log!(
            Info,
            TAG,
            "New mining job: id={}, version={:08x}, future={}",
            j.job_id,
            j.version,
            if j.min_ntime.is_some() { "no" } else { "yes" }
        );
        match (j.min_ntime, &self.prev) {
            (Some(ntime), Some(prev)) => {
                let job = StandardJob {
                    job_id: j.job_id,
                    version: j.version,
                    merkle_root: j.merkle_root,
                    prev_hash: prev.hash,
                    ntime,
                    nbits: prev.nbits,
                };
                self.emit_standard(job);
            }
            _ => {
                self.pending_standard[j.job_id as usize % PENDING_JOBS] =
                    Some((j.job_id, j.version, j.merkle_root));
            }
        }
    }

    fn new_extended_job(&mut self, payload: &[u8]) {
        let Ok(j) = p::new_extended_mining_job(payload) else {
            log!(Error, TAG, "Failed to parse NewExtendedMiningJob");
            return;
        };
        log!(
            Info,
            TAG,
            "New extended mining job: id={}, version={:08x}, merkle_branches={}",
            j.job_id,
            j.version,
            j.merkle_path.len()
        );
        // Spec 5.3.16: without version_rolling_allowed the version must be
        // used as sent; the ASICs roll it regardless, so every share would be
        // invalid. Drop the connection instead of silently mining nothing.
        if !j.version_rolling_allowed {
            log!(
                Error,
                TAG,
                "Dropping extended job {}: pool does not allow version rolling",
                j.job_id
            );
            self.events.push_back(Event::SetupFailed);
            return;
        }
        let mut job = ExtendedJob {
            job_id: j.job_id,
            version: j.version,
            prev_hash: [0; 32],
            ntime: j.min_ntime.unwrap_or(0),
            nbits: 0,
            merkle_path: j.merkle_path,
            coinbase_prefix: j.coinbase_prefix.to_vec(),
            coinbase_suffix: j.coinbase_suffix.to_vec(),
            extranonce_prefix: self.extranonce_prefix.clone(),
        };
        // Like the C++ code, a job counts as current if it has a non-zero
        // min_ntime.
        match (&self.prev, job.ntime > 0) {
            (Some(prev), true) => {
                job.prev_hash = prev.hash;
                job.nbits = prev.nbits;
                self.emit_extended(job);
            }
            _ => {
                let slot = job.job_id as usize % PENDING_JOBS;
                self.pending_extended[slot] = Some(job);
            }
        }
    }

    fn set_extranonce_prefix(&mut self, payload: &[u8]) {
        let Some((channel_id, prefix)) = p::set_extranonce_prefix(payload) else {
            log!(Error, TAG, "Failed to parse SetExtranoncePrefix");
            return;
        };
        let old = if self.extranonce_prefix.is_empty() {
            "(none)".into()
        } else {
            hex(&self.extranonce_prefix)
        };
        let new = if prefix.is_empty() {
            "(empty)".into()
        } else {
            hex(prefix)
        };
        // Jobs already received keep their prefix; only later jobs use the new one.
        log!(Info, TAG, "SetExtranoncePrefix: {old} -> {new} (channel {channel_id}, applies to jobs from here on)");
        self.extranonce_prefix = prefix.to_vec();
    }

    fn set_new_prev_hash(&mut self, payload: &[u8]) {
        let Some((_channel_id, job_id, hash, ntime, nbits)) = p::set_new_prev_hash(payload) else {
            log!(Error, TAG, "Failed to parse SetNewPrevHash");
            return;
        };
        log!(
            Info,
            TAG,
            "New prev_hash: job_id={job_id}, ntime={ntime}, nbits={nbits:08x}"
        );
        self.events.push_back(Event::NetworkDifficulty(nbits));
        let first = self.prev.is_none();
        self.prev = Some(PrevHash { hash, ntime, nbits });
        let slot = job_id as usize % PENDING_JOBS;

        // Standard channel: the job this prevhash is for, and on the first
        // prevhash all others that were waiting.
        let standard = |(id, version, merkle_root): (u32, u32, [u8; 32])| StandardJob {
            job_id: id,
            version,
            merkle_root,
            prev_hash: hash,
            ntime,
            nbits,
        };
        if let Some(pending) = self.pending_standard[slot].filter(|&(id, _, _)| id == job_id) {
            self.pending_standard[slot] = None;
            self.emit_standard(standard(pending));
        }
        if first {
            for i in 0..PENDING_JOBS {
                if let Some(pending) = self.pending_standard[i].filter(|&(id, _, _)| id != job_id) {
                    self.pending_standard[i] = None;
                    self.emit_standard(standard(pending));
                }
            }
        }

        // Extended channel: the same.
        let activate = |mut job: ExtendedJob| {
            job.prev_hash = hash;
            job.ntime = ntime;
            job.nbits = nbits;
            job
        };
        if self.pending_extended[slot]
            .as_ref()
            .is_some_and(|j| j.job_id == job_id)
        {
            let job = self.pending_extended[slot].take().unwrap();
            self.emit_extended(activate(job));
        }
        if first {
            for i in 0..PENDING_JOBS {
                if self.pending_extended[i]
                    .as_ref()
                    .is_some_and(|j| j.job_id != job_id)
                {
                    let job = self.pending_extended[i].take().unwrap();
                    self.emit_extended(activate(job));
                }
            }
        }

        self.events.push_back(Event::ValidNotify);
    }

    fn set_target(&mut self, payload: &[u8]) {
        let Some((_channel_id, target)) = p::set_target(payload) else {
            log!(Error, TAG, "Failed to parse SetTarget");
            return;
        };
        self.target = target;
        let pdiff = self.pool_difficulty();
        log!(Info, TAG, "Set pool difficulty: {pdiff}");
        self.events.push_back(Event::SetTarget(pdiff));
    }

    fn submit_success(&mut self, payload: &[u8]) {
        let Some((_channel_id, mut accepted)) = p::submit_shares_success(payload) else {
            return;
        };
        if accepted > MAX_ACCEPTED_PER_MESSAGE {
            log!(
                Warn,
                TAG,
                "Suspicious accepted_count {accepted}, capping to {MAX_ACCEPTED_PER_MESSAGE}"
            );
            accepted = MAX_ACCEPTED_PER_MESSAGE;
        }
        self.events.push_back(Event::SharesAccepted(accepted));
    }

    fn submit_error(&mut self, payload: &[u8]) {
        if let Some((_channel_id, _seq, code)) = p::submit_shares_error(payload) {
            // The C++ code read the code into a 64-byte buffer.
            let code = &code[..code.len().min(63)];
            log!(
                Warn,
                TAG,
                "Share rejected: {}",
                String::from_utf8_lossy(code)
            );
            self.events.push_back(Event::ShareRejected(code.to_vec()));
        }
    }

    /// The SubmitShares frame for a share; None if it cannot be built (the
    /// sequence number advances anyway, as in the C++ code).
    ///
    /// `job_id` is the decimal job ID string of the bm_job, `extranonce2_hex`
    /// its extranonce2 (empty for standard jobs).
    pub fn submit(
        &mut self,
        job_id: &[u8],
        extranonce2_hex: &[u8],
        ntime: u32,
        nonce: u32,
        version: u32,
    ) -> Option<Vec<u8>> {
        let seq = self.sequence_number;
        self.sequence_number = self.sequence_number.wrapping_add(1);
        let job_id = strtoul32_dec(job_id);
        if self.extended && !extranonce2_hex.is_empty() {
            let mut en2 = vec![0u8; extranonce2_hex.len() / 2];
            crate::cutil::hex2bin(extranonce2_hex, &mut en2);
            // The C++ code built the frame into a 63-byte buffer: at most 32
            // extranonce bytes fit.
            if en2.len() > 32 {
                return None;
            }
            Some(p::submit_shares_extended(
                self.channel_id,
                seq,
                job_id,
                nonce,
                ntime,
                version,
                &en2,
            ))
        } else {
            Some(p::submit_shares_standard(
                self.channel_id,
                seq,
                job_id,
                nonce,
                ntime,
                version,
            ))
        }
    }
}

/// `strtoul(s, NULL, 10)` with a 32-bit `unsigned long` (job IDs are our own
/// "%lu" strings, so only the plain case matters).
fn strtoul32_dec(s: &[u8]) -> u32 {
    let s = crate::cutil::until_nul(s);
    let mut v: u32 = 0;
    for &c in s.iter().skip_while(|c| c.is_ascii_whitespace()) {
        if !c.is_ascii_digit() {
            break;
        }
        v = match v
            .checked_mul(10)
            .and_then(|v| v.checked_add((c - b'0') as u32))
        {
            Some(v) => v,
            None => return u32::MAX,
        };
    }
    v
}

fn hex(b: &[u8]) -> String {
    crate::cutil::to_hex(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(s: &mut Session) -> Vec<Event> {
        std::iter::from_fn(|| s.next_event()).collect()
    }

    fn open_extended(size: u16, prefix: &[u8]) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend(1u32.to_le_bytes());
        m.extend(7u32.to_le_bytes()); // channel id
        let mut target = [0u8; 32];
        target[27] = 0x01; // difficulty > 1
        m.extend(target);
        m.extend(size.to_le_bytes());
        m.push(prefix.len() as u8);
        m.extend(prefix);
        m.extend(0u32.to_le_bytes());
        m
    }

    fn ext_job(job_id: u32, min_ntime: Option<u32>, rolling: bool) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend(7u32.to_le_bytes());
        m.extend(job_id.to_le_bytes());
        match min_ntime {
            Some(t) => {
                m.push(1);
                m.extend(t.to_le_bytes());
            }
            None => m.push(0),
        }
        m.extend(0x2000_0000u32.to_le_bytes());
        m.push(rolling as u8);
        m.push(1); // one merkle branch
        m.extend([0xab; 32]);
        m.extend(3u16.to_le_bytes());
        m.extend([1, 2, 3]);
        m.extend(2u16.to_le_bytes());
        m.extend([4, 5]);
        m
    }

    fn prev_hash(job_id: u32, ntime: u32) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend(7u32.to_le_bytes());
        m.extend(job_id.to_le_bytes());
        m.extend([0xcd; 32]);
        m.extend(ntime.to_le_bytes());
        m.extend(0x1703_4219u32.to_le_bytes());
        m
    }

    fn prefix_msg(prefix: &[u8]) -> Vec<u8> {
        let mut m = 7u32.to_le_bytes().to_vec();
        m.push(prefix.len() as u8);
        m.extend(prefix);
        m
    }

    #[test]
    fn extranonce_size_is_checked() {
        for size in [0u16, 33, 255, 256, 1000] {
            let mut s = Session::new(true);
            assert!(
                !s.open_channel_success(
                    MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
                    &open_extended(size, b"")
                ),
                "{size}"
            );
        }
        let mut s = Session::new(true);
        assert!(s.open_channel_success(
            MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
            &open_extended(8, b"\x01\x02")
        ));
        assert!(matches!(events(&mut s)[..], [Event::PoolDifficulty(_)]));
        assert!(!s.open_channel_success(
            MSG_OPEN_STANDARD_MINING_CHANNEL_SUCCESS,
            &open_extended(8, b"")
        ));
    }

    #[test]
    fn future_jobs_wait_for_their_prevhash() {
        let mut s = Session::new(true);
        s.open_channel_success(
            MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
            &open_extended(8, b"\xaa"),
        );
        events(&mut s);

        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(1, None, true));
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(2, None, true));
        assert!(events(&mut s).is_empty());

        // The first prevhash activates the job it names and all others.
        s.handle(MSG_SET_NEW_PREV_HASH, &prev_hash(2, 1000));
        let ev = events(&mut s);
        let ids: Vec<u32> = ev
            .iter()
            .filter_map(|e| match e {
                Event::ExtendedJob {
                    job,
                    extranonce_size: 8,
                    clean: true,
                    ..
                } => {
                    assert_eq!(
                        (job.ntime, job.nbits, job.prev_hash),
                        (1000, 0x1703_4219, [0xcd; 32])
                    );
                    Some(job.job_id)
                }
                _ => None,
            })
            .collect();
        assert_eq!(ids, [2, 1]);
        assert!(matches!(
            ev.first(),
            Some(Event::NetworkDifficulty(0x1703_4219))
        ));
        assert!(matches!(ev.last(), Some(Event::ValidNotify)));

        // A current job after that is delivered right away.
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(3, Some(1001), true));
        assert!(
            matches!(&events(&mut s)[..], [Event::ExtendedJob { job, .. }] if job.ntime == 1001)
        );

        // A later prevhash only activates its own job.
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(4, None, true));
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(5, None, true));
        s.handle(MSG_SET_NEW_PREV_HASH, &prev_hash(5, 1002));
        let ids: Vec<u32> = events(&mut s)
            .iter()
            .filter_map(|e| {
                if let Event::ExtendedJob { job, .. } = e {
                    Some(job.job_id)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(ids, [5]);
    }

    #[test]
    fn extranonce_prefix_is_pinned_at_arrival() {
        let mut s = Session::new(true);
        s.open_channel_success(
            MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
            &open_extended(8, b"\x01"),
        );
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(1, None, true));
        s.handle(MSG_SET_EXTRANONCE_PREFIX, &prefix_msg(b"\x02\x02"));
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(2, None, true));
        s.handle(MSG_SET_NEW_PREV_HASH, &prev_hash(1, 1000));
        let prefixes: Vec<(u32, Vec<u8>)> = events(&mut s)
            .into_iter()
            .filter_map(|e| {
                if let Event::ExtendedJob { job, .. } = e {
                    Some((job.job_id, job.extranonce_prefix))
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(prefixes, [(1, vec![1]), (2, vec![2, 2])]);
    }

    #[test]
    fn version_rolling_must_be_allowed() {
        let mut s = Session::new(true);
        s.handle(MSG_NEW_EXTENDED_MINING_JOB, &ext_job(1, Some(5), false));
        assert_eq!(events(&mut s), [Event::SetupFailed]);
    }

    #[test]
    fn shares() {
        let mut s = Session::new(true);
        s.open_channel_success(
            MSG_OPEN_EXTENDED_MINING_CHANNEL_SUCCESS,
            &open_extended(8, b""),
        );
        events(&mut s);
        let f = s.submit(b"12", b"0000000000000001", 3, 4, 5).unwrap();
        assert_eq!(&f[..6], &[0x00, 0x80, 0x1b, 33, 0, 0]);
        assert_eq!(&f[6..14], &[7, 0, 0, 0, 0, 0, 0, 0]); // channel 7, sequence 0
        assert_eq!(&f[14..18], &12u32.to_le_bytes());
        assert_eq!(&f[30..], &[8, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert!(s.submit(b"1", &[b'0'; 66], 0, 0, 0).is_none());
        let f = s.submit(b"1", b"", 0, 0, 0).unwrap(); // standard frame, sequence 2
        assert_eq!((f[2], &f[10..14]), (0x1a, &2u32.to_le_bytes()[..]));

        let mut ok = 7u32.to_le_bytes().to_vec();
        ok.extend([0; 4]);
        ok.extend(5000u32.to_le_bytes());
        ok.extend([0; 8]);
        s.handle(MSG_SUBMIT_SHARES_SUCCESS, &ok);
        let mut err = 7u32.to_le_bytes().to_vec();
        err.extend([0; 4]);
        err.push(5);
        err.extend(b"stale");
        s.handle(MSG_SUBMIT_SHARES_ERROR, &err);
        assert_eq!(
            events(&mut s),
            [
                Event::SharesAccepted(1000),
                Event::ShareRejected(b"stale".to_vec())
            ]
        );
    }

    #[test]
    fn decimal_job_ids() {
        assert_eq!(strtoul32_dec(b"4294967295"), u32::MAX);
        assert_eq!(strtoul32_dec(b"4294967296"), u32::MAX);
        assert_eq!(strtoul32_dec(b" 42x"), 42);
        assert_eq!(strtoul32_dec(b""), 0);
    }
}
