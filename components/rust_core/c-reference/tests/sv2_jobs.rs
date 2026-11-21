//! Compares the Rust SV2 job construction (`sv2_job_hashes`,
//! `sv2_build_extended_job`) with the former `MiningInfoV2Standard/Extended::
//! buildBmJob()` (c-sv2/mining_info_v2.cpp, compiled for the host).
//!
//! The reference supports `extranonce_size` up to 32 (its buffer size), so
//! sizes stay within 0..=32 here; the Rust side rejects larger sizes (unit
//! test in jobs.rs).

use std::cell::Cell;
use std::ffi::{c_char, CStr};

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::mining::JobHashes;
use rust_core::stratum_v2::ExtJob;

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

impl BmJob {
    fn hashes(&self) -> JobHashes {
        JobHashes {
            merkle_root: self.merkle_root,
            merkle_root_be: self.merkle_root_be,
            prev_block_hash: self.prev_block_hash,
            prev_block_hash_be: self.prev_block_hash_be,
        }
    }
}

extern "C" {
    fn ref_sv2_standard_job(merkle_root: *const u8, prev_hash: *const u8, out: *mut BmJob);
    fn ref_sv2_extended_job(
        job: *const ExtJob,
        extranonce_prefix: *const u8,
        extranonce_prefix_len: u8,
        extranonce_size: u8,
        counter: u32,
        out: *mut BmJob,
        extranonce2: *mut c_char,
        extranonce2_len: usize,
    );
    fn sv2_job_hashes(merkle_root: *const u8, prev_hash: *const u8, out: *mut JobHashes);
    fn sv2_build_extended_job(
        coinbase_prefix: *const u8,
        coinbase_prefix_len: u16,
        extranonce_prefix: *const u8,
        extranonce_prefix_len: u8,
        extranonce_size: u8,
        coinbase_suffix: *const u8,
        coinbase_suffix_len: u16,
        merkle_path: *const [u8; 32],
        merkle_path_count: u8,
        prev_hash: *const u8,
        counter: u32,
        out: *mut JobHashes,
        extranonce2_hex: *mut c_char,
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
    fn bytes(&self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn bytes32(&self) -> [u8; 32] {
        std::array::from_fn(|_| self.next() as u8)
    }
}

/// Fixed (the golden file records the outputs for exactly these inputs).
fn iterations() -> (usize, u64) {
    (100_000, 0x005f_20b5_0000_0006)
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

fn zeroed<T>() -> T {
    // SAFETY: used for plain-data C structs only.
    unsafe { std::mem::zeroed() }
}

#[test]
fn standard_jobs_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let mut rec = Rec::new("sv2_jobs", "standard_jobs_match_reference");
    for i in 0..n {
        let (merkle, prev) = (rng.bytes32(), rng.bytes32());
        let mut reference: BmJob = zeroed();
        let mut current = JobHashes::default();
        // SAFETY: 32-byte inputs, writable outputs.
        unsafe {
            ref_sv2_standard_job(merkle.as_ptr(), prev.as_ptr(), &mut reference);
            sv2_job_hashes(merkle.as_ptr(), prev.as_ptr(), &mut current);
        }
        assert_eq!(current, reference.hashes(), "case {i}");
        rec.add(format!("{i} {}", hashes_text(&current)));
    }
    rec.finish();
}

#[test]
fn extended_jobs_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x66));
    let mut rec = Rec::new("sv2_jobs", "extended_jobs_match_reference");
    for i in 0..n {
        let prefix = rng.bytes(rng.below(300));
        let suffix = rng.bytes(rng.below(300));
        let en_prefix = rng.bytes(rng.below(33));
        let en_size = rng.below(33) as u8;
        let counter = if rng.below(2) == 0 {
            rng.next() as u32
        } else {
            rng.below(1000) as u32
        };
        let merkle: Vec<[u8; 32]> = (0..rng.below(21)).map(|_| rng.bytes32()).collect();
        let prev = rng.bytes32();

        let mut job: ExtJob = zeroed();
        job.job_id = i as u32;
        job.prev_hash = prev;
        job.ntime = 1000;
        job.merkle_path_count = merkle.len() as u8;
        job.merkle_path[..merkle.len()].copy_from_slice(&merkle);
        job.coinbase_prefix = prefix.as_ptr() as *mut u8;
        job.coinbase_prefix_len = prefix.len() as u16;
        job.coinbase_suffix = suffix.as_ptr() as *mut u8;
        job.coinbase_suffix_len = suffix.len() as u16;

        let mut reference: BmJob = zeroed();
        let mut ref_en2 = [0 as c_char; 80];
        let mut current = JobHashes::default();
        let mut en2 = [0 as c_char; 65];
        // SAFETY: valid job and buffers; the reference copies what it needs.
        let ok = unsafe {
            ref_sv2_extended_job(
                &job,
                en_prefix.as_ptr(),
                en_prefix.len() as u8,
                en_size,
                counter,
                &mut reference,
                ref_en2.as_mut_ptr(),
                ref_en2.len(),
            );
            sv2_build_extended_job(
                prefix.as_ptr(),
                prefix.len() as u16,
                en_prefix.as_ptr(),
                en_prefix.len() as u8,
                en_size,
                suffix.as_ptr(),
                suffix.len() as u16,
                merkle.as_ptr(),
                merkle.len() as u8,
                prev.as_ptr(),
                counter,
                &mut current,
                en2.as_mut_ptr(),
            )
        };
        assert!(ok, "case {i}");
        // SAFETY: NUL-terminated by both.
        let (a, b) = unsafe {
            (
                CStr::from_ptr(ref_en2.as_ptr()),
                CStr::from_ptr(en2.as_ptr()),
            )
        };
        assert_eq!(b, a, "case {i}: extranonce2");
        assert_eq!(current, reference.hashes(), "case {i}");
        rec.add(format!(
            "{i} {ok} {} {}",
            b.to_str().unwrap(),
            hashes_text(&current)
        ));
    }
    rec.finish();
}
