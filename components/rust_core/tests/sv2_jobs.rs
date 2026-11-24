//! Golden tests of the SV2 job construction (`hashes`, `ExtendedJob::build`,
//! behind `sv2_job_hashes` / `sv2_build_extended_job`): the hash fields and
//! extranonce2 for random jobs, against the outputs of the former C/C++ code
//! (`MiningInfoV2Standard/Extended::buildBmJob()`; tests/golden/sv2_jobs.txt,
//! recorded by the differential tests).

use std::cell::Cell;

use rust_core::cutil::to_hex;
use rust_core::mining::JobHashes;
use rust_core::pool::stratum_v2::jobs::{hashes, ExtendedJob};

mod golden;
use golden::Rec;

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
    to_hex(b)
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

#[test]
fn standard_jobs_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let mut rec = Rec::new("sv2_jobs", "standard_jobs_match_reference");
    for i in 0..n {
        let (merkle, prev) = (rng.bytes32(), rng.bytes32());
        let current = hashes(merkle, &prev);
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

        let job = ExtendedJob {
            coinbase_prefix: &prefix,
            extranonce_prefix: &en_prefix,
            extranonce_size: en_size as usize,
            coinbase_suffix: &suffix,
            merkle_path: &merkle,
            prev_hash: &prev,
        };
        let built = job.build(counter);
        let ok = built.is_some();
        assert!(ok, "case {i}");
        let (current, en2) = built.unwrap();
        rec.add(format!(
            "{i} {ok} {} {}",
            to_hex(&en2),
            hashes_text(&current)
        ));
    }
    rec.finish();
}
