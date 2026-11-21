//! Job math shared by the Stratum protocols: double SHA-256, merkle root,
//! byte orders of the header fields and the difficulty of a found nonce.
//! C API: `mining_rs.h`.
//!
//! Port of parts of the former `components/bm1397/mining.cpp`, verified
//! against its outputs by `tests/mining.rs`.

/// SHA-256. On the device this is mbedTLS, which uses the ESP32 SHA
/// accelerator (as the C code did); on the host the `sha2` crate.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    #[cfg(target_os = "espidf")]
    {
        extern "C" {
            fn mbedtls_sha256(
                input: *const u8,
                len: usize,
                output: *mut u8,
                is224: core::ffi::c_int,
            ) -> core::ffi::c_int;
        }
        let mut out = [0u8; 32];
        // SAFETY: valid input slice and 32-byte output buffer.
        unsafe { mbedtls_sha256(data.as_ptr(), data.len(), out.as_mut_ptr(), 0) };
        out
    }
    #[cfg(not(target_os = "espidf"))]
    {
        use sha2::Digest;
        sha2::Sha256::digest(data).into()
    }
}

pub fn sha256d(data: &[u8]) -> [u8; 32] {
    sha256(&sha256(data))
}

/// Merkle root from the coinbase transaction and the branches of
/// `mining.notify`, in the byte order of the hash output.
pub fn merkle_root(coinbase_tx: &[u8], branches: &[[u8; 32]]) -> [u8; 32] {
    branches.iter().fold(sha256d(coinbase_tx), |root, branch| {
        let mut both = [0u8; 64];
        both[..32].copy_from_slice(&root);
        both[32..].copy_from_slice(branch);
        sha256d(&both)
    })
}

/// Reverses the bytes of each 4-byte word (`swap_endian_words_bin`).
pub fn swap_words(data: &[u8; 32]) -> [u8; 32] {
    let mut out = *data;
    for word in out.chunks_exact_mut(4) {
        word.reverse();
    }
    out
}

pub fn reversed(data: &[u8; 32]) -> [u8; 32] {
    let mut out = *data;
    out.reverse();
    out
}

/// The four hash fields of a `bm_job`, in the byte orders the hashing code
/// (`*`) and the ASIC drivers (`*_be`) expect.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JobHashes {
    pub merkle_root: [u8; 32],
    pub merkle_root_be: [u8; 32],
    pub prev_block_hash: [u8; 32],
    pub prev_block_hash_be: [u8; 32],
}

impl JobHashes {
    /// As `construct_bm_job()` derives them from the merkle root (hash byte
    /// order) and the prevhash as sent in `mining.notify`.
    pub fn new(merkle_root: [u8; 32], notify_prev_hash: &[u8; 32]) -> Self {
        Self {
            merkle_root,
            merkle_root_be: reversed(&swap_words(&merkle_root)),
            prev_block_hash: swap_words(notify_prev_hash),
            prev_block_hash_be: reversed(notify_prev_hash),
        }
    }
}

/// 2^224 * 0xffff: the target of difficulty 1.
pub(crate) const TRUEDIFFONE: f64 =
    26959535291011309493156476344723991336010898738574164086137773096960.0;

/// A little-endian 256-bit value as f64, computed exactly like cgminer's
/// `le256todouble()` (same order of operations, so bit-identical).
pub(crate) fn le256_to_f64(v: &[u8; 32]) -> f64 {
    let word = |i: usize| u64::from_le_bytes(v[i * 8..i * 8 + 8].try_into().unwrap()) as f64;
    const BITS192: f64 = 6277101735386680763835789423207666416102355444464034512896.0;
    const BITS128: f64 = 340282366920938463463374607431768211456.0;
    const BITS64: f64 = 18446744073709551616.0;
    let mut d = word(3) * BITS192;
    d += word(2) * BITS128;
    d += word(1) * BITS64;
    d += word(0);
    d
}

/// Difficulty of the block header hash for `nonce` (`test_nonce_value()`).
/// `prev_block_hash` and `merkle_root` are the hashing-order fields of a job.
pub fn nonce_difficulty(
    version: u32,
    prev_block_hash: &[u8; 32],
    merkle_root: &[u8; 32],
    ntime: u32,
    nbits: u32,
    nonce: u32,
) -> f64 {
    let mut header = [0u8; 80];
    header[0..4].copy_from_slice(&version.to_le_bytes());
    header[4..36].copy_from_slice(prev_block_hash);
    header[36..68].copy_from_slice(merkle_root);
    header[68..72].copy_from_slice(&ntime.to_le_bytes());
    header[72..76].copy_from_slice(&nbits.to_le_bytes());
    header[76..80].copy_from_slice(&nonce.to_le_bytes());
    TRUEDIFFONE / le256_to_f64(&sha256d(&header))
}

mod ffi {
    use super::*;

    /// # Safety
    /// `prev_block_hash` and `merkle_root` must point to 32 bytes each.
    #[no_mangle]
    pub unsafe extern "C" fn mining_nonce_difficulty(
        version: u32,
        prev_block_hash: *const [u8; 32],
        merkle_root: *const [u8; 32],
        ntime: u32,
        nbits: u32,
        nonce: u32,
    ) -> f64 {
        nonce_difficulty(
            version,
            &*prev_block_hash,
            &*merkle_root,
            ntime,
            nbits,
            nonce,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        crate::cutil::hex2bin(s.as_bytes(), &mut out);
        out
    }

    /// The genesis block: one transaction, so the merkle root is its txid.
    const GENESIS_COINBASE: &str = "01000000010000000000000000000000000000000000000000000000000000000000000000ffffffff4d04ffff001d0104455468652054696d65732030332f4a616e2f32303039204368616e63656c6c6f72206f6e206272696e6b206f66207365636f6e64206261696c6f757420666f722062616e6b73ffffffff0100f2052a01000000434104678afdb0fe5548271967f1a67130b7105cd6a828e03909a67962e0ea1f61deb649f6bc3f4cef38c4f35504e51ec112de5c384df7ba0b8d578a4c702b6bf11d5fac00000000";

    #[test]
    fn genesis_block() {
        let mut tx = vec![0u8; GENESIS_COINBASE.len() / 2];
        crate::cutil::hex2bin(GENESIS_COINBASE.as_bytes(), &mut tx);
        let root = merkle_root(&tx, &[]);
        // txid 4a5e1e4b...a33b is displayed byte-reversed
        assert_eq!(
            reversed(&root),
            unhex::<32>("4a5e1e4baab89f3a32518a88c31bc87f618f76673e2cc77ab2127b7afdeda33b")
        );

        // Header hash 000000000019d668...: difficulty about 2536 (truediffone / hash)
        let hashes = JobHashes::new(root, &[0; 32]);
        let d = nonce_difficulty(
            1,
            &hashes.prev_block_hash,
            &hashes.merkle_root,
            0x495fab29,
            0x1d00ffff,
            0x7c2bac1d,
        );
        assert!((2500.0..2600.0).contains(&d), "{d}");
    }

    #[test]
    fn byte_orders() {
        let mut x = [0u8; 32];
        for (i, b) in x.iter_mut().enumerate() {
            *b = i as u8;
        }
        assert_eq!(&swap_words(&x)[..8], &[3, 2, 1, 0, 7, 6, 5, 4]);
        assert_eq!(reversed(&x)[0], 31);
    }
}
