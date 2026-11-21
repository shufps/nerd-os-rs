//! Job construction for Stratum v2 (port of `MiningInfoV2Standard/Extended::
//! buildBmJob` in `main/tasks/mining_info_v2.cpp`): the bm_job hash fields
//! from SV2 data, which is in internal byte order, and for extended channels
//! the coinbase transaction and merkle root.

use crate::mining::{merkle_root, swap_words, JobHashes};

/// Most extranonce bytes an extended channel can have (B0_32 in the spec).
pub const MAX_EXTRANONCE: usize = 32;

/// Hash fields from a merkle root and prevhash in internal byte order. SV2
/// sends the prevhash unswapped; `JobHashes::new` expects it as `mining.notify`
/// sends it (word-swapped), so swap first.
pub fn hashes(merkle_root: [u8; 32], prev_hash: &[u8; 32]) -> JobHashes {
    JobHashes::new(merkle_root, &swap_words(prev_hash))
}

/// extranonce2 for the counter: `size` bytes, big-endian, the counter's low
/// bytes at the end (zero padded; for `size` < 4 the high bytes are dropped).
pub fn extranonce2(counter: u32, size: usize) -> Vec<u8> {
    let be = counter.to_be_bytes();
    let mut out = vec![0u8; size];
    let n = size.min(4);
    out[size - n..].copy_from_slice(&be[4 - n..]);
    out
}

/// Extended channel: coinbase = prefix | extranonce prefix | extranonce2 |
/// suffix, merkle root over the merkle path. None if the extranonce does not
/// fit (`extranonce_size` > 32).
pub struct ExtendedJob<'a> {
    pub coinbase_prefix: &'a [u8],
    pub extranonce_prefix: &'a [u8],
    pub extranonce_size: usize,
    pub coinbase_suffix: &'a [u8],
    pub merkle_path: &'a [[u8; 32]],
    pub prev_hash: &'a [u8; 32],
}

impl ExtendedJob<'_> {
    /// Hash fields and extranonce2 (sent with the share) for `counter`.
    pub fn build(&self, counter: u32) -> Option<(JobHashes, Vec<u8>)> {
        if self.extranonce_size > MAX_EXTRANONCE {
            return None;
        }
        let en2 = extranonce2(counter, self.extranonce_size);
        let coinbase = [
            self.coinbase_prefix,
            self.extranonce_prefix,
            &en2,
            self.coinbase_suffix,
        ]
        .concat();
        let root = merkle_root(&coinbase, self.merkle_path);
        Some((hashes(root, self.prev_hash), en2))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extranonce2_layout() {
        assert_eq!(extranonce2(0x0102_0304, 6), [0, 0, 1, 2, 3, 4]);
        assert_eq!(extranonce2(0x0102_0304, 2), [3, 4]);
        assert_eq!(extranonce2(7, 0), Vec::<u8>::new());
    }

    #[test]
    fn rejects_oversized_extranonce() {
        let job = ExtendedJob {
            coinbase_prefix: b"",
            extranonce_prefix: b"",
            extranonce_size: 33,
            coinbase_suffix: b"",
            merkle_path: &[],
            prev_hash: &[0; 32],
        };
        assert!(job.build(1).is_none());
    }
}
