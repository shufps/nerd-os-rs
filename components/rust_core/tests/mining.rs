//! Golden tests of the Stratum v1 job construction (`job::build`, behind
//! `stratum_v1_build_job`) and the nonce check (`nonce_difficulty`): hash
//! fields, extranonce2 and difficulties for random jobs, against the outputs
//! of the former C/C++ code (`MiningInfoV1::buildBmJob()`,
//! `test_nonce_value()`; tests/golden/mining.txt, recorded by the
//! differential tests).

use std::cell::Cell;
use std::ffi::CString;

use rust_core::mining::{nonce_difficulty, JobHashes};
use rust_core::pool::stratum_v1::job::build;
use rust_core::pool::stratum_v1::MiningNotify;

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
    rust_core::cutil::to_hex(b)
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
fn build_job_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let (mut compared, mut high_digits) = (0, 0);
    let mut rec = Rec::new("mining", "build_job_matches_reference");

    for i in 0..n {
        let cb1 = CString::new(rng.hex(rng.below(200) + rng.below(2))).unwrap();
        let cb2 = CString::new(rng.hex(rng.below(300) + rng.below(2))).unwrap();
        let en1 = CString::new(rng.hex(2 * rng.below(9))).unwrap();
        let en2_len = 1 + rng.below(16) as i32;
        let en2 = match rng.below(4) {
            0 => rng.below(256) as u32,
            1 => rng.next() as u32,
            2 => ((rng.below(127) as u32) << 25) | rng.below(1 << 25) as u32, // CAN slave counters
            _ => rng.below(1 << 20) as u32,
        };

        let prev_block_hash = rng.bytes32();
        let n_merkle = rng.below(15);
        let notify = MiningNotify {
            job_id: b"1".to_vec(),
            coinbase_1: cb1.as_bytes().to_vec(),
            coinbase_2: cb2.as_bytes().to_vec(),
            prev_block_hash,
            merkle_branches: (0..n_merkle).map(|_| rng.bytes32()).collect(),
            ..Default::default()
        };

        let (hashes, new_en2) = build(
            &notify,
            [cb1.as_bytes(), en1.as_bytes(), cb2.as_bytes()],
            en2_len,
            en2,
        )
        .expect("valid extranonce2_len");

        let digits = 2 * en2_len as usize;
        let full = format!("{en2:x}");
        if full.len() > digits {
            // for a counter that outgrows extranonce2_len Rust keeps the low
            // digits (the C code kept the high ones: deliberate difference)
            assert_eq!(new_en2, full[full.len() - digits..], "case {i}");
            rec.add(format!("{i} high {new_en2}"));
            high_digits += 1;
            continue;
        }

        rec.add(format!("{i} {new_en2} {}", hashes_text(&hashes)));
        compared += 1;
    }
    eprintln!("{compared} compared, {high_digits} with a counter beyond extranonce2_len");
    assert!(compared > n / 2);
    rec.finish();
}

#[test]
fn nonce_difficulty_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x55));
    let mut rec = Rec::new("mining", "nonce_difficulty_matches_reference");
    for i in 0..n {
        let prev_block_hash = rng.bytes32();
        let merkle_root = rng.bytes32();
        let ntime = rng.next() as u32;
        let target = rng.next() as u32;
        let (nonce, version) = (rng.next() as u32, rng.next() as u32);

        let got = nonce_difficulty(
            version,
            &prev_block_hash,
            &merkle_root,
            ntime,
            target,
            nonce,
        );
        rec.add(format!("{i} {got:?} {:#x}", got.to_bits()));
    }
    rec.finish();
}

#[test]
fn genesis_block_through_both() {
    // Genesis header fields in job order.
    let tx = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";
    let (cb1, en1, cb2) = (&tx[..100], &tx[100..108], &tx[116..]);
    let en2 = u32::from_str_radix(&tx[108..116], 16).unwrap();
    let notify = MiningNotify::default();

    let (hashes, _) = build(
        &notify,
        [cb1.as_bytes(), en1.as_bytes(), cb2.as_bytes()],
        4,
        en2,
    )
    .unwrap();
    let mut rec = Rec::new("mining", "genesis_block_through_both");
    rec.add(hashes_text(&hashes));

    let got = nonce_difficulty(
        1,
        &hashes.prev_block_hash,
        &hashes.merkle_root,
        0x495fab29,
        0x1d00ffff,
        0x7c2bac1d,
    );
    rec.add(format!("{got:?} {:#x}", got.to_bits()));
    assert!((2500.0..2600.0).contains(&got), "{got}");
    rec.finish();
}
