//! The CAN slave (port of the protocol side of `can_slave_task.cpp`): the
//! negotiation with the master (HELLO until ASSIGN, re-negotiation after a
//! master boot or 10 s without jobs), the reassembly of the jobs, the job
//! store per ASIC job ID, the settings commands, and the filter of the
//! nonces sent back. The TWAI driver, the ASICs, the settings and the
//! telemetry: `can.rs`.
//!
//! Behaviour notes:
//! - a frame's payload is at most the 8 data bytes (DLC handling as on the
//!   master);
//! - the job store is behind a lock (the receive task writes a job while the
//!   result task reads it);
//! - an ASSIGN with an ID outside 1..31 (e.g. the unassigned 0xFF) is
//!   ignored.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::log::log;
use crate::mining::{nonce_difficulty, reversed, swap_words};
use crate::util::cutil::fixed;

const TAG: &CStr = c"can_slave";

pub const ID_ASSIGN: u32 = 0x101;
pub const ID_MASTER_BOOT: u32 = 0x102;
pub const ID_JOB_BASE: u32 = 0x200;
pub const ID_SETTINGS_BASE: u32 = 0x500;
pub const SEQ_LAST: u8 = 0xff;
pub const UNASSIGNED: u8 = 0xff;
/// `BM1368_job` (packed)
pub const JOB_LEN: usize = 82;
/// the job and its pool difficulty
const JOB_PAYLOAD_LEN: usize = JOB_LEN + 4;
const RX_MAX: usize = 128;
const HELLO_MS: u64 = 1000;
const JOB_TIMEOUT_MS: u64 = 10_000;

/// What the C++ side has to do after a frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// `CAN_SLAVE_EVENT_*`
    pub kind: u8,
    /// ASSIGNED: the new ID; COMMAND: the payload length
    pub id: u8,
    /// COMMAND: command byte and arguments
    pub cmd: [u8; 7],
    /// JOB: the `BM1368_job` for the ASICs
    pub job: [u8; JOB_LEN],
}

const _: () = assert!(core::mem::size_of::<Event>() == 91);

pub const EVENT_NONE: u8 = 0;
pub const EVENT_ASSIGNED: u8 = 1;
pub const EVENT_JOB: u8 = 2;
pub const EVENT_COMMAND: u8 = 3;

impl Event {
    pub const NONE: Event = Event {
        kind: EVENT_NONE,
        id: 0,
        cmd: [0; 7],
        job: [0; JOB_LEN],
    };
}

pub struct Slave {
    mac: [u8; 6],
    id: u8,
    /// None: send a HELLO now
    last_hello_ms: Option<u64>,
    last_job_ms: u64,
    rx: Vec<u8>,
    in_frame: bool,
    /// per ASIC job ID: the job and its pool difficulty
    jobs: Vec<Option<([u8; JOB_LEN], u32)>>,
}

impl Slave {
    pub fn new(mac: [u8; 6]) -> Self {
        Self {
            mac,
            id: UNASSIGNED,
            last_hello_ms: None,
            last_job_ms: 0,
            rx: Vec::with_capacity(RX_MAX),
            in_frame: false,
            jobs: vec![None; 256],
        }
    }

    fn renegotiate(&mut self) {
        self.id = UNASSIGNED;
        self.last_hello_ms = None;
        self.in_frame = false;
        self.rx.clear();
    }

    /// True if a HELLO is due (every second while unassigned).
    pub fn hello_due(&mut self, now_ms: u64) -> bool {
        if self.id != UNASSIGNED {
            return false;
        }
        match self.last_hello_ms {
            Some(t) if now_ms.saturating_sub(t) < HELLO_MS => false,
            _ => {
                self.last_hello_ms = Some(now_ms);
                true
            }
        }
    }

    /// A received frame (`dlc` as the driver reports it, can exceed 8).
    pub fn frame(&mut self, id: u32, dlc: u8, data: &[u8; 8], now_ms: u64) -> Event {
        if id == ID_MASTER_BOOT {
            log!(Info, TAG, "Master boot detected → re-negotiating");
            self.renegotiate();
            return Event::NONE;
        }
        if id == ID_ASSIGN && dlc == 7 {
            if data[..6] == self.mac {
                let new_id = data[6];
                if !(1..32).contains(&new_id) {
                    log!(Warn, TAG, "ASSIGN with invalid CAN ID {new_id}, ignored");
                    return Event::NONE;
                }
                log!(Info, TAG, "Assigned CAN ID={new_id}");
                self.id = new_id;
                self.last_job_ms = now_ms;
                return Event {
                    kind: EVENT_ASSIGNED,
                    id: new_id,
                    ..Event::NONE
                };
            }
            return Event::NONE;
        }
        if self.id == UNASSIGNED {
            return Event::NONE;
        }
        if now_ms.saturating_sub(self.last_job_ms) >= JOB_TIMEOUT_MS {
            log!(Warn, TAG, "No job for 10s → re-negotiating");
            self.renegotiate();
            return Event::NONE;
        }

        // at most the 7 payload bytes the frame has
        let payload = &data[1..(dlc.max(1) as usize).min(8)];

        if id == ID_SETTINGS_BASE | self.id as u32 {
            if dlc >= 2 && data[0] == SEQ_LAST {
                let mut cmd = [0u8; 7];
                cmd[..payload.len()].copy_from_slice(payload);
                return Event {
                    kind: EVENT_COMMAND,
                    id: payload.len() as u8,
                    cmd,
                    ..Event::NONE
                };
            }
            return Event::NONE;
        }

        if id != ID_JOB_BASE | self.id as u32 || dlc < 1 {
            return Event::NONE;
        }
        let seq = data[0];
        if seq == 0 {
            if self.in_frame {
                log!(
                    Warn,
                    TAG,
                    "new job seq=0 while in_frame, discarding {} bytes",
                    self.rx.len()
                );
            }
            self.rx.clear();
            self.in_frame = true;
        } else if !self.in_frame {
            return Event::NONE;
        }
        if self.rx.len() + payload.len() > RX_MAX {
            log!(Warn, TAG, "reassembly overflow, discarding");
            self.in_frame = false;
            return Event::NONE;
        }
        self.rx.extend_from_slice(payload);
        if seq != SEQ_LAST {
            return Event::NONE;
        }
        self.in_frame = false;
        // a complete message resets the timeout
        self.last_job_ms = now_ms;
        if self.rx.len() != JOB_PAYLOAD_LEN {
            log!(
                Warn,
                TAG,
                "unexpected job size {} (expected {JOB_PAYLOAD_LEN})",
                self.rx.len()
            );
            return Event::NONE;
        }
        let mut job = [0u8; JOB_LEN];
        job.copy_from_slice(&self.rx[..JOB_LEN]);
        let pool_diff = u32::from_le_bytes(self.rx[JOB_LEN..].try_into().unwrap());
        self.jobs[job[0] as usize] = Some((job, pool_diff));
        log!(
            Info,
            TAG,
            "RX JOB job_id={:02X} pool_diff={pool_diff} → sendRawJob",
            job[0]
        );
        Event {
            kind: EVENT_JOB,
            job,
            ..Event::NONE
        }
    }

    /// A nonce of the ASICs: send it to the master unless the stored job
    /// says it is below the pool difficulty.
    pub fn check_nonce(&self, job_id: u8, nonce: u32, rolled_version: u32) -> bool {
        let Some((job, pool_diff)) = self.jobs[job_id as usize] else {
            log!(
                Info,
                TAG,
                "NONCE nonce={nonce:08X} job_id={job_id:02X} (no job stored) → TX to master"
            );
            return true;
        };
        let diff = job_difficulty(&job, nonce, rolled_version);
        let send = diff >= pool_diff as f64;
        log!(
            Info,
            TAG,
            "NONCE job={job_id:02X} nonce={nonce:08X} diff={}/{pool_diff} → {}",
            fixed(diff, 1),
            if send { "TX to master" } else { "drop" }
        );
        send
    }
}

/// The nonce difficulty from a `BM1368_job` (its hash fields are in the
/// ASIC byte order: reversed, then word-swapped back into the hashing
/// order); the chips return the rolled bits, combined with the job's
/// version.
pub fn job_difficulty(job: &[u8; JOB_LEN], nonce: u32, rolled_version: u32) -> f64 {
    let field = |at: usize| -> [u8; 32] { job[at..at + 32].try_into().unwrap() };
    let le = |at: usize| u32::from_le_bytes(job[at..at + 4].try_into().unwrap());
    let merkle = swap_words(&reversed(&field(14)));
    let prev = swap_words(&reversed(&field(46)));
    nonce_difficulty(
        rolled_version ^ le(78),
        &prev,
        &merkle,
        le(10),
        le(6),
        nonce,
    )
}

static SLAVE: Mutex<Option<Slave>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut Slave) -> T) -> Option<T> {
    SLAVE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_mut()
        .map(f)
}

pub(crate) mod api {
    use super::*;

    pub fn can_slave_init(mac: [u8; 6]) {
        log!(
            Info,
            TAG,
            "CAN slave MAC={}",
            mac.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        );
        *SLAVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(Slave::new(mac));
    }

    pub fn can_slave_hello_due(now_ms: u64) -> bool {
        with(|s| s.hello_due(now_ms)).unwrap_or(false)
    }

    /// A received frame: what to do.
    pub fn can_slave_frame(id: u32, dlc: u8, data: &[u8; 8], now_ms: u64) -> Event {
        with(|s| s.frame(id, dlc, data, now_ms)).unwrap_or(Event::NONE)
    }

    /// The assigned CAN ID, `CAN_SLAVE_ID_UNASSIGNED` (0xFF) before.
    pub fn can_slave_id() -> u8 {
        with(|s| s.id).unwrap_or(UNASSIGNED)
    }

    pub fn can_slave_check_nonce(job_id: u8, nonce: u32, rolled_version: u32) -> bool {
        with(|s| s.check_nonce(job_id, nonce, rolled_version)).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC: [u8; 6] = [1, 2, 3, 4, 5, 6];

    fn assign(s: &mut Slave, id: u8, now: u64) -> Event {
        let mut d = [0u8; 8];
        d[..6].copy_from_slice(&MAC);
        d[6] = id;
        s.frame(ID_ASSIGN, 7, &d, now)
    }

    fn send(s: &mut Slave, base: u32, msg: &[u8], now: u64) -> Event {
        let chunks: Vec<&[u8]> = msg.chunks(7).collect();
        let mut ev = Event::NONE;
        for (i, c) in chunks.iter().enumerate() {
            let mut d = [0u8; 8];
            d[0] = if i == chunks.len() - 1 {
                SEQ_LAST
            } else {
                i as u8
            };
            d[1..1 + c.len()].copy_from_slice(c);
            ev = s.frame(base | s.id as u32, 1 + c.len() as u8, &d, now);
        }
        ev
    }

    #[test]
    fn negotiation() {
        let mut s = Slave::new(MAC);
        assert!(s.hello_due(0));
        assert!(!s.hello_due(500));
        assert!(s.hello_due(1000));
        // another MAC, invalid IDs: ignored
        let mut d = [9u8; 8];
        assert_eq!(s.frame(ID_ASSIGN, 7, &d, 0).kind, EVENT_NONE);
        assert_eq!(assign(&mut s, 0xff, 0).kind, EVENT_NONE);
        assert_eq!(assign(&mut s, 0, 0).kind, EVENT_NONE);
        let ev = assign(&mut s, 5, 1000);
        assert_eq!((ev.kind, ev.id, s.id), (EVENT_ASSIGNED, 5, 5));
        assert!(!s.hello_due(5000));
        // master boot: unassigned, HELLO right away
        d[0] = 0;
        s.frame(ID_MASTER_BOOT, 0, &d, 2000);
        assert_eq!(s.id, UNASSIGNED);
        assert!(s.hello_due(2001));
    }

    #[test]
    fn jobs_commands_and_timeout() {
        let mut s = Slave::new(MAC);
        assign(&mut s, 3, 0);
        let mut payload = vec![0u8; JOB_PAYLOAD_LEN];
        payload[0] = 0x18;
        payload[JOB_LEN..].copy_from_slice(&512u32.to_le_bytes());
        let ev = send(&mut s, ID_JOB_BASE, &payload, 100);
        assert_eq!((ev.kind, ev.job[0]), (EVENT_JOB, 0x18));
        assert_eq!(s.jobs[0x18].map(|j| j.1), Some(512));
        // wrong size: dropped
        assert_eq!(
            send(&mut s, ID_JOB_BASE, &payload[..80], 200).kind,
            EVENT_NONE
        );
        // a job for another slave: ignored
        let d = [SEQ_LAST, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(s.frame(ID_JOB_BASE | 4, 8, &d, 300).kind, EVENT_NONE);
        // settings command
        let d = [SEQ_LAST, 0x02, 0x10, 0x04, 0, 0, 0, 0];
        let ev = s.frame(ID_SETTINGS_BASE | 3, 4, &d, 400);
        assert_eq!(
            (ev.kind, ev.id, &ev.cmd[..3]),
            (EVENT_COMMAND, 3, &[0x02, 0x10, 0x04][..])
        );
        // DLC 15: at most 7 payload bytes
        let ev = s.frame(ID_SETTINGS_BASE | 3, 15, &d, 500);
        assert_eq!(ev.id, 7);
        // 10 s without a complete message (the last one at 200, even with
        // the wrong size): unassigned
        assert_eq!(s.frame(ID_JOB_BASE | 3, 8, &d, 10_100).kind, EVENT_NONE);
        assert_eq!(s.id, 3);
        assert_eq!(s.frame(ID_JOB_BASE | 3, 8, &d, 10_200).kind, EVENT_NONE);
        assert_eq!(s.id, UNASSIGNED);
    }

    #[test]
    fn nonce_filter_uses_the_stored_job() {
        let mut s = Slave::new(MAC);
        assign(&mut s, 2, 0);
        // unknown job: sent
        assert!(s.check_nonce(7, 1, 0));
        let mut payload = vec![0u8; JOB_PAYLOAD_LEN];
        payload[0] = 7;
        // pool difficulty 0: every nonce passes; u32::MAX: none
        send(&mut s, ID_JOB_BASE, &payload, 10);
        assert!(s.check_nonce(7, 1, 0));
        payload[JOB_LEN..].copy_from_slice(&u32::MAX.to_le_bytes());
        send(&mut s, ID_JOB_BASE, &payload, 20);
        assert!(!s.check_nonce(7, 1, 0));
    }

    #[test]
    fn job_difficulty_matches_the_bm_job_path() {
        // a job built like the master does: the hashing-order fields turned
        // into the ASIC order, and back
        let merkle: [u8; 32] = core::array::from_fn(|i| i as u8);
        let prev: [u8; 32] = core::array::from_fn(|i| 0x80 + i as u8);
        let mut job = [0u8; JOB_LEN];
        job[6..10].copy_from_slice(&0x1703_a30cu32.to_le_bytes());
        job[10..14].copy_from_slice(&0x66f0_0000u32.to_le_bytes());
        job[14..46].copy_from_slice(&reversed(&swap_words(&merkle)));
        job[46..78].copy_from_slice(&reversed(&swap_words(&prev)));
        job[78..82].copy_from_slice(&0x2000_0000u32.to_le_bytes());
        let expected = nonce_difficulty(0x2000_2000, &prev, &merkle, 0x66f0_0000, 0x1703_a30c, 42);
        assert_eq!(job_difficulty(&job, 42, 0x2000), expected);
    }
}
