//! Symmetric crypto: HMAC-SHA256 and the Noise HKDF on top of
//! [`mining::sha256`](crate::mining::sha256), SHA-1 and HMAC-SHA1 for TOTP
//! (both hardware accelerated on the device), ChaCha20-Poly1305 (RustCrypto,
//! same code on device and host), and the random source.

use chacha20poly1305::aead::{AeadInPlace, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce, Tag};

use crate::mining::sha256;

pub const TAG_LEN: usize = 16;

/// HMAC (RFC 2104) for a hash with 64-byte blocks.
fn hmac<const N: usize>(hash: fn(&[u8]) -> [u8; N], key: &[u8], data: &[u8]) -> [u8; N] {
    let mut block = [0u8; 64];
    if key.len() > 64 {
        block[..N].copy_from_slice(&hash(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let pad = |byte: u8| block.map(|b| b ^ byte);
    let inner = hash(&[&pad(0x36)[..], data].concat());
    hash(&[&pad(0x5c)[..], &inner].concat())
}

pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    hmac(sha256, key, data)
}

pub fn sha1(data: &[u8]) -> [u8; 20] {
    #[cfg(target_os = "espidf")]
    {
        extern "C" {
            fn mbedtls_sha1(input: *const u8, len: usize, output: *mut u8) -> core::ffi::c_int;
        }
        let mut out = [0u8; 20];
        // SAFETY: valid input slice and 20-byte output buffer.
        unsafe { mbedtls_sha1(data.as_ptr(), data.len(), out.as_mut_ptr()) };
        out
    }
    #[cfg(not(target_os = "espidf"))]
    {
        use sha1::Digest;
        sha1::Sha1::digest(data).into()
    }
}

pub fn hmac_sha1(key: &[u8], data: &[u8]) -> [u8; 20] {
    hmac(sha1, key, data)
}

/// Comparison whose time does not depend on where the inputs differ.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let diff = a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y));
    // SAFETY: plain read of a local; keeps the optimizer from short-cutting.
    unsafe { core::ptr::read_volatile(&diff) == 0 }
}

/// Noise HKDF with two outputs, keyed by the chaining key `ck`.
pub fn hkdf2(ck: &[u8; 32], ikm: &[u8]) -> ([u8; 32], [u8; 32]) {
    let prk = hmac_sha256(ck, ikm);
    let out1 = hmac_sha256(&prk, &[0x01]);
    let out2 = hmac_sha256(&prk, &[&out1[..], &[0x02]].concat());
    (out1, out2)
}

/// Noise nonce: 4 zero bytes and the 64-bit counter, little-endian.
fn nonce(counter: u64) -> Nonce {
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&counter.to_le_bytes());
    n.into()
}

/// ChaCha20-Poly1305: ciphertext with the 16-byte tag appended.
pub fn encrypt(key: &[u8; 32], counter: u64, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN);
    out.extend_from_slice(plaintext);
    let tag = ChaCha20Poly1305::new(key.into())
        .encrypt_in_place_detached(&nonce(counter), aad, &mut out)
        .expect("plaintext length within ChaCha20 limits");
    out.extend_from_slice(&tag);
    out
}

/// Decrypts ciphertext with appended tag into `out` (`ciphertext.len() - 16`
/// bytes); false if the tag does not verify.
pub fn decrypt_into(
    key: &[u8; 32],
    counter: u64,
    aad: &[u8],
    ciphertext: &[u8],
    out: &mut [u8],
) -> bool {
    let Some(pt_len) = ciphertext.len().checked_sub(TAG_LEN) else {
        return false;
    };
    let (ct, tag) = ciphertext.split_at(pt_len);
    let out = &mut out[..pt_len];
    out.copy_from_slice(ct);
    let ok = ChaCha20Poly1305::new(key.into())
        .decrypt_in_place_detached(&nonce(counter), aad, out, Tag::from_slice(tag))
        .is_ok();
    if !ok {
        out.fill(0);
    }
    ok
}

/// Hardware random numbers on the device (`esp_fill_random`); on the host the
/// stand-in from host/esp_host.c, which tests can script.
pub fn fill_random(buf: &mut [u8]) {
    extern "C" {
        fn esp_fill_random(buf: *mut core::ffi::c_void, len: usize);
    }
    // SAFETY: writes exactly buf.len() bytes.
    unsafe { esp_fill_random(buf.as_mut_ptr() as _, buf.len()) }
}

pub fn random32() -> [u8; 32] {
    let mut b = [0u8; 32];
    fill_random(&mut b);
    b
}

/// Overwrites key material in a way the optimizer keeps.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: valid, aligned u8 pointer from the slice.
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hmac_sha1_rfc2202() {
        // RFC 2202 test cases 1 and 6 (key longer than a block)
        assert_eq!(
            hmac_sha1(&[0x0b; 20], b"Hi There"),
            *b"\xb6\x17\x31\x86\x55\x05\x72\x64\xe2\x8b\xc0\xb6\xfb\x37\x8c\x8e\xf1\x46\xbe\x00"
        );
        assert_eq!(
            hmac_sha1(
                &[0xaa; 80],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            ),
            *b"\xaa\x4a\xe5\xe1\x52\x72\xd0\x0e\x95\x70\x56\x37\xce\x8a\x3b\x55\xed\x40\x21\x12"
        );
        assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"ab", b"abc"));
    }

    fn unhex(s: &str) -> Vec<u8> {
        let mut out = vec![0u8; s.len() / 2];
        crate::cutil::hex2bin(s.as_bytes(), &mut out);
        out
    }

    /// RFC 4231 test case 2
    #[test]
    fn hmac_rfc4231() {
        assert_eq!(
            hmac_sha256(b"Jefe", b"what do ya want for nothing?").to_vec(),
            unhex("5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843")
        );
        // test case 6: key longer than the block size
        assert_eq!(
            hmac_sha256(
                &[0xaa; 131],
                b"Test Using Larger Than Block-Size Key - Hash Key First"
            )
            .to_vec(),
            unhex("60e431591ee0b67f0d8a26aacbf5b77f8e0bc6213728c5140546040f0ee37f54")
        );
    }

    /// RFC 8439 section 2.8.2
    #[test]
    fn chacha20poly1305_rfc8439() {
        let key: [u8; 32] =
            unhex("808182838485868788898a8b8c8d8e8f909192939495969798999a9b9c9d9e9f")
                .try_into()
                .unwrap();
        let aad = unhex("50515253c0c1c2c3c4c5c6c7");
        let pt = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        // The RFC nonce is 07000000 4041424344454647; build it the Noise way.
        let n = u64::from_le_bytes(unhex("4041424344454647").try_into().unwrap());
        let mut nonce_bytes = [0u8; 12];
        nonce_bytes[..4].copy_from_slice(&[7, 0, 0, 0]);
        nonce_bytes[4..].copy_from_slice(&n.to_le_bytes());
        let mut buf = pt.to_vec();
        let tag = ChaCha20Poly1305::new((&key).into())
            .encrypt_in_place_detached(&nonce_bytes.into(), &aad, &mut buf)
            .unwrap();
        assert_eq!(tag.to_vec(), unhex("1ae10b594f09e26a7e902ecbd0600691"));

        // Round trip through the Noise helpers (4 zero bytes in the nonce).
        let ct = encrypt(&key, 5, &aad, pt);
        let mut out = vec![0u8; pt.len()];
        assert!(decrypt_into(&key, 5, &aad, &ct, &mut out));
        assert_eq!(&out[..], &pt[..]);
        let mut bad = ct.clone();
        bad[0] ^= 1;
        assert!(!decrypt_into(&key, 5, &aad, &bad, &mut out));
        assert!(!decrypt_into(&key, 6, &aad, &ct, &mut out));
    }
}
