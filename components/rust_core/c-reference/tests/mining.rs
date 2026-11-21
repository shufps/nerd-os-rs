//! Compares the Rust job construction (`stratum_v1_build_job`) and nonce
//! check (`mining_nonce_difficulty`) with the former C/C++ code: a verbatim
//! copy of `MiningInfoV1::buildBmJob()` on top of the former
//! `calculate_merkle_root_hash()`/`construct_bm_job()`/`test_nonce_value()`.
//!
//! `DIFF_ITERATIONS` (default 100000) and `DIFF_SEED` override the run.

use std::cell::Cell;
use std::ffi::{c_char, CStr, CString};

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::mining::{nonce_difficulty, JobHashes};
use rust_core::stratum_v1::job::{Job, EXTRANONCE2_HEX_MAX};
use rust_core::stratum_v1::MiningNotify;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

/// `bm_job` (`include/jobs.h`)
#[repr(C)]
#[derive(Clone, Copy)]
struct BmJob {
    version: u32,
    version_mask: u32,
    prev_block_hash: [u8; 32],
    prev_block_hash_be: [u8; 32],
    merkle_root: [u8; 32],
    merkle_root_be: [u8; 32],
    ntime: u32,
    target: u32,
    starting_nonce: u32,
    pool_diff: u32,
    asic_diff: u32,
    pool_id: i32,
    jobid: *mut c_char,
    extranonce2: *mut c_char,
}

extern "C" {
    fn ref_build_job(
        notify: *mut MiningNotify,
        extranonce1: *const c_char,
        extranonce2_len: i32,
        extranonce2: u32,
        version_mask: u32,
        out: *mut BmJob,
        out_extranonce2: *mut c_char,
        out_extranonce2_len: usize,
    ) -> bool;
    fn ref_test_nonce_value(job: *const BmJob, nonce: u32, rolled_version: u32) -> f64;
    fn stratum_v1_build_job(
        notify: *const MiningNotify,
        extranonce1: *const c_char,
        extranonce2_len: i32,
        extranonce2: u32,
        job: *mut Job,
    ) -> bool;
}

struct Rng(Cell<u64>);

impl Rng {
    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn hex(&self, n: usize) -> String {
        const DIGITS: &[u8] = b"0123456789abcdefABCDEF";
        let mut s: String = (0..n)
            .map(|_| DIGITS[self.below(DIGITS.len())] as char)
            .collect();
        if self.chance(3) && n > 0 {
            // an invalid digit somewhere
            let i = self.below(n);
            s.replace_range(i..i + 1, "z");
        }
        s
    }
    fn bytes32(&self) -> [u8; 32] {
        std::array::from_fn(|_| self.next() as u8)
    }
}

/// Fixed (the golden file records the outputs for exactly these inputs).
fn iterations() -> (usize, u64) {
    (100_000, 0x7a11_b0b0_5eed_0002)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn hashes_text(h: &JobHashes) -> String {
    format!(
        "{} {} {} {}",
        hex(&h.merkle_root),
        hex(&h.merkle_root_be),
        hex(&h.prev_block_hash),
        hex(&h.prev_block_hash_be)
    )
}

fn zeroed_notify() -> MiningNotify {
    // SAFETY: all-zero is a valid MiningNotify (null pointers, zero numbers).
    unsafe { std::mem::zeroed() }
}

#[test]
fn build_job_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let (mut compared, mut high_digits) = (0, 0);
    let mut rec = Rec::new("mining", "build_job_matches_reference");

    for i in 0..n {
        let cb1 = CString::new(rng.hex(rng.below(200) + rng.below(2))).unwrap();
        let cb2 = CString::new(rng.hex(rng.below(300) + rng.below(2))).unwrap();
        let en1 = CString::new(rng.hex(2 * rng.below(9))).unwrap();
        let job_id = CString::new("1").unwrap();
        let en2_len = 1 + rng.below(16) as i32;
        let en2 = match rng.below(4) {
            0 => rng.below(256) as u32,
            1 => rng.next() as u32,
            2 => ((rng.below(127) as u32) << 25) | rng.below(1 << 25) as u32, // CAN slave counters
            _ => rng.below(1 << 20) as u32,
        };

        let mut notify = zeroed_notify();
        notify.job_id = job_id.as_ptr() as _;
        notify.coinbase_1 = cb1.as_ptr() as _;
        notify.coinbase_2 = cb2.as_ptr() as _;
        notify.prev_block_hash = rng.bytes32();
        notify.n_merkle_branches = rng.below(15);
        for b in &mut notify.merkle_branches[..notify.n_merkle_branches] {
            *b = rng.bytes32();
        }

        let mut job = Job {
            hashes: JobHashes::default(),
            extranonce2: [0; EXTRANONCE2_HEX_MAX],
        };
        // SAFETY: valid notify, strings and output.
        assert!(unsafe { stratum_v1_build_job(&notify, en1.as_ptr(), en2_len, en2, &mut job) });
        let new_en2 = unsafe { CStr::from_ptr(job.extranonce2.as_ptr()) }
            .to_str()
            .unwrap()
            .to_string();

        // SAFETY: as the firmware calls it; BmJob is plain data.
        let mut reference: BmJob = unsafe { std::mem::zeroed() };
        let mut ref_en2 = [0 as c_char; 64];
        unsafe {
            ref_build_job(
                &mut notify,
                en1.as_ptr(),
                en2_len,
                en2,
                0,
                &mut reference,
                ref_en2.as_mut_ptr(),
                64,
            )
        };
        let ref_en2 = unsafe { CStr::from_ptr(ref_en2.as_ptr()) }
            .to_str()
            .unwrap()
            .to_string();

        let digits = 2 * en2_len as usize;
        let full = format!("{en2:x}");
        if full.len() > digits {
            // Deliberate difference: for a counter that outgrows
            // extranonce2_len the reference keeps the high digits, Rust the low.
            assert_eq!(ref_en2, full[..digits], "case {i}");
            assert_eq!(new_en2, full[full.len() - digits..], "case {i}");
            rec.add(format!("{i} high {new_en2}"));
            high_digits += 1;
            continue;
        }

        assert_eq!(new_en2, ref_en2, "case {i}");
        let r = &reference;
        let expected = JobHashes {
            merkle_root: r.merkle_root,
            merkle_root_be: r.merkle_root_be,
            prev_block_hash: r.prev_block_hash,
            prev_block_hash_be: r.prev_block_hash_be,
        };
        assert_eq!(
            job.hashes, expected,
            "case {i}: cb1={cb1:?} en1={en1:?} en2={en2:#x}/{en2_len} cb2={cb2:?}"
        );
        rec.add(format!("{i} {new_en2} {}", hashes_text(&job.hashes)));
        compared += 1;
    }
    eprintln!("{compared} identical, {high_digits} with a counter beyond extranonce2_len (deliberate difference)");
    assert!(compared > n / 2);
    rec.finish();
}

#[test]
fn nonce_difficulty_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x55));
    let mut rec = Rec::new("mining", "nonce_difficulty_matches_reference");
    for i in 0..n {
        // SAFETY: plain data.
        let mut job: BmJob = unsafe { std::mem::zeroed() };
        job.prev_block_hash = rng.bytes32();
        job.merkle_root = rng.bytes32();
        job.ntime = rng.next() as u32;
        job.target = rng.next() as u32;
        let (nonce, version) = (rng.next() as u32, rng.next() as u32);

        let expected = unsafe { ref_test_nonce_value(&job, nonce, version) };
        let got = nonce_difficulty(
            version,
            &job.prev_block_hash,
            &job.merkle_root,
            job.ntime,
            job.target,
            nonce,
        );
        assert_eq!(
            got.to_bits(),
            expected.to_bits(),
            "case {i}: {got} vs {expected}"
        );
        rec.add(format!("{i} {got:?} {:#x}", got.to_bits()));
    }
    rec.finish();
}

#[test]
fn genesis_block_through_both() {
    // Genesis header fields in job order; difficulty must match exactly.
    let tx = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";
    let (cb1, en1, cb2) = (&tx[..100], &tx[100..108], &tx[116..]);
    let en2 = u32::from_str_radix(&tx[108..116], 16).unwrap();
    let (cb1, en1, cb2) = (
        CString::new(cb1).unwrap(),
        CString::new(en1).unwrap(),
        CString::new(cb2).unwrap(),
    );
    let job_id = CString::new("g").unwrap();
    let mut notify = zeroed_notify();
    notify.job_id = job_id.as_ptr() as _;
    notify.coinbase_1 = cb1.as_ptr() as _;
    notify.coinbase_2 = cb2.as_ptr() as _;

    let mut job = Job {
        hashes: JobHashes::default(),
        extranonce2: [0; EXTRANONCE2_HEX_MAX],
    };
    assert!(unsafe { stratum_v1_build_job(&notify, en1.as_ptr(), 4, en2, &mut job) });
    let mut reference: BmJob = unsafe { std::mem::zeroed() };
    let mut s = [0 as c_char; 64];
    unsafe {
        ref_build_job(
            &mut notify,
            en1.as_ptr(),
            4,
            en2,
            0,
            &mut reference,
            s.as_mut_ptr(),
            64,
        )
    };
    assert_eq!(job.hashes.merkle_root, reference.merkle_root);
    let mut rec = Rec::new("mining", "genesis_block_through_both");
    rec.add(hashes_text(&job.hashes));

    reference.ntime = 0x495fab29;
    reference.target = 0x1d00ffff;
    let expected = unsafe { ref_test_nonce_value(&reference, 0x7c2bac1d, 1) };
    let got = nonce_difficulty(
        1,
        &job.hashes.prev_block_hash,
        &job.hashes.merkle_root,
        0x495fab29,
        0x1d00ffff,
        0x7c2bac1d,
    );
    assert_eq!(got.to_bits(), expected.to_bits());
    rec.add(format!("{got:?} {:#x}", got.to_bits()));
    assert!((2500.0..2600.0).contains(&got), "{got}");
    rec.finish();
}
