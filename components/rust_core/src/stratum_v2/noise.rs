//! Noise_NX_Secp256k1+EllSwift_ChaChaPoly_SHA256, the encrypted transport of
//! Stratum v2, as initiator. C API: `sv2_noise.h` (unchanged).
//!
//! Port of `components/stratum_v2/sv2_noise.c`, verified against it by
//! `c-reference/tests/sv2_noise.rs` (through the C API). [`Handshake`] and
//! [`Cipher`] are pure (bytes in, bytes out); [`handshake`], [`Sender::send`]
//! and [`Receiver::recv`] add the socket I/O through a [`Wire`] (on the device
//! [`EspWire`], `esp_transport_read/write`). The two directions are separate
//! values, so the share-submitting task and the receiving task each lock only
//! their own. Behaviour notes:
//!
//! * a write that times out is an error, one that completes only partially
//!   is retried for the remainder,
//! * no key material is logged.

use core::ffi::{c_char, c_int, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};

use super::protocol::{parse_frame_header, FRAME_HEADER_SIZE};
use crate::crypto::{decrypt_into, encrypt, hkdf2, random32, wipe, TAG_LEN};
use crate::log::log;
use crate::mining::sha256;
use crate::secp::Context;

pub const PROTOCOL_NAME: &[u8] = b"Noise_NX_Secp256k1+EllSwift_ChaChaPoly_SHA256";

/// SHA-256 of [`PROTOCOL_NAME`], checked at runtime as a sanity check of the
/// (hardware) SHA-256, as the C code did.
const PROTOCOL_HASH: [u8; 32] = [
    46, 180, 120, 129, 32, 142, 158, 238, 31, 102, 159, 103, 198, 110, 231, 14, 169, 234, 136, 9,
    13, 80, 63, 232, 48, 220, 75, 200, 62, 41, 191, 16,
];

/// Responder message: ephemeral key (64), encrypted static key (64 + 16),
/// encrypted certificate (74 + 16).
pub const ACT2_LEN: usize = 234;
const CERT_LEN: usize = 74;
/// Encrypted frame header: 6 bytes plus tag.
pub const ENC_HEADER_LEN: usize = FRAME_HEADER_SIZE + TAG_LEN;

const TAG: &CStr = c"sv2_noise";

fn mix_hash(h: &mut [u8; 32], data: &[u8]) {
    *h = sha256(&[&h[..], data].concat());
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeError {
    SelfTest,
    ProtocolHash,
    EphemeralKey,
    Ecdh,
    StaticKeyMac,
    CertificateMac,
    ServerKey,
    AuthorityKey,
    CertificateSignature,
    /// The validity period starts after the current time.
    CertificateNotYetValid(Certificate),
    /// The validity period ended before the current time.
    CertificateExpired(Certificate),
}

/// The server certificate from the handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Certificate {
    pub version: u16,
    pub valid_from: u32,
    pub not_valid_after: u32,
}

/// Allowed difference between our clock and the pool's when the certificate
/// validity period is checked. Pools issue short-lived certificates starting
/// at the time they are made.
pub const CLOCK_SKEW_S: u32 = 600;

impl Certificate {
    /// Whether `now` (Unix time) is within the validity period, with
    /// [`CLOCK_SKEW_S`] tolerance on both ends.
    pub fn check_validity(&self, now: u32) -> Result<(), HandshakeError> {
        if now.saturating_add(CLOCK_SKEW_S) < self.valid_from {
            Err(HandshakeError::CertificateNotYetValid(*self))
        } else if now.saturating_sub(CLOCK_SKEW_S) > self.not_valid_after {
            Err(HandshakeError::CertificateExpired(*self))
        } else {
            Ok(())
        }
    }
}

/// Initiator state between sending its ephemeral key and receiving the
/// responder's answer.
pub struct Handshake<'a> {
    secp: &'a Context,
    h: [u8; 32],
    ck: [u8; 32],
    e_priv: [u8; 32],
    e_pub: [u8; 64],
}

impl<'a> Handshake<'a> {
    /// Starts the handshake; the returned 64 bytes go to the responder.
    pub fn start(
        secp: &'a Context,
        e_priv: [u8; 32],
        aux: &[u8; 32],
    ) -> Result<(Self, [u8; 64]), HandshakeError> {
        let h = sha256(PROTOCOL_NAME);
        if h != PROTOCOL_HASH {
            return Err(HandshakeError::ProtocolHash);
        }
        let mut hs = Self {
            secp,
            h,
            ck: h,
            e_priv,
            e_pub: [0; 64],
        };
        mix_hash(&mut hs.h, b""); // empty prologue
        hs.e_pub = secp
            .ellswift_create(&hs.e_priv, aux)
            .ok_or(HandshakeError::EphemeralKey)?;
        mix_hash(&mut hs.h, &hs.e_pub); // token e
        mix_hash(&mut hs.h, b""); // EncryptAndHash(empty payload) without a key
        let e_pub = hs.e_pub;
        Ok((hs, e_pub))
    }

    /// Processes the responder's message. With `authority`, the server
    /// certificate must carry a valid Schnorr signature by that key and, with
    /// `now` (Unix time, when the clock is known), be valid at that time.
    pub fn finish(
        mut self,
        act2: &[u8; ACT2_LEN],
        authority: Option<&[u8; 32]>,
        now: Option<u32>,
    ) -> Result<(Cipher, Certificate), HandshakeError> {
        let re: &[u8; 64] = act2[..64].try_into().unwrap();
        mix_hash(&mut self.h, re);

        // ee
        let shared = self
            .secp
            .ellswift_xdh(&self.e_pub, re, &self.e_priv, 0)
            .ok_or(HandshakeError::Ecdh)?;
        let (ck, mut temp_k) = hkdf2(&self.ck, &shared);
        self.ck = ck;

        // s: the responder's static key, encrypted
        let mut rs = [0u8; 64];
        let ok = decrypt_into(&temp_k, 0, &self.h, &act2[64..144], &mut rs);
        wipe(&mut temp_k);
        if !ok {
            return Err(HandshakeError::StaticKeyMac);
        }
        mix_hash(&mut self.h, &act2[64..144]);

        // es
        let shared2 = self
            .secp
            .ellswift_xdh(&self.e_pub, &rs, &self.e_priv, 0)
            .ok_or(HandshakeError::Ecdh)?;
        let (ck, mut temp_k2) = hkdf2(&self.ck, &shared2);
        self.ck = ck;

        // certificate: version u16, valid_from u32, not_valid_after u32, signature
        let mut cert = [0u8; CERT_LEN];
        let ok = decrypt_into(&temp_k2, 0, &self.h, &act2[144..ACT2_LEN], &mut cert);
        wipe(&mut temp_k2);
        if !ok {
            return Err(HandshakeError::CertificateMac);
        }
        let certificate = Certificate {
            version: u16::from_le_bytes([cert[0], cert[1]]),
            valid_from: u32::from_le_bytes(cert[2..6].try_into().unwrap()),
            not_valid_after: u32::from_le_bytes(cert[6..10].try_into().unwrap()),
        };

        if let Some(authority) = authority {
            let server_key = self
                .secp
                .ellswift_xonly(&rs)
                .ok_or(HandshakeError::ServerKey)?;
            let msg = sha256(&[&cert[..10], &server_key].concat());
            let sig: &[u8; 64] = cert[10..].try_into().unwrap();
            match self.secp.schnorr_verify(sig, &msg, authority) {
                None => return Err(HandshakeError::AuthorityKey),
                Some(false) => return Err(HandshakeError::CertificateSignature),
                Some(true) => {}
            }
            // The certificate must be valid now (when the time is known).
            if let Some(now) = now {
                certificate.check_validity(now)?;
            }
        }

        let (send_key, recv_key) = hkdf2(&self.ck, b"");
        Ok((
            Cipher {
                send: Sender {
                    key: send_key,
                    nonce: 0,
                },
                recv: Receiver {
                    key: recv_key,
                    nonce: 0,
                },
            },
            certificate,
        ))
    }
}

impl Drop for Handshake<'_> {
    fn drop(&mut self) {
        wipe(&mut self.e_priv);
        wipe(&mut self.ck);
        wipe(&mut self.h);
    }
}

/// Transport keys after the handshake.
pub struct Cipher {
    pub send: Sender,
    pub recv: Receiver,
}

/// The sending direction: key and nonce.
pub struct Sender {
    key: [u8; 32],
    nonce: u64,
}

/// The receiving direction: key and nonce.
pub struct Receiver {
    key: [u8; 32],
    nonce: u64,
}

impl Sender {
    fn next(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let n = self.nonce;
        self.nonce += 1;
        encrypt(&self.key, n, &[], plaintext)
    }

    /// An SV2 frame as sent: encrypted header, then the encrypted payload
    /// (if any). None if `frame` is shorter than a frame header.
    pub fn encrypt_frame(&mut self, frame: &[u8]) -> Option<(Vec<u8>, Option<Vec<u8>>)> {
        if frame.len() < FRAME_HEADER_SIZE {
            return None;
        }
        let header = self.next(&frame[..FRAME_HEADER_SIZE]);
        let payload =
            (frame.len() > FRAME_HEADER_SIZE).then(|| self.next(&frame[FRAME_HEADER_SIZE..]));
        Some((header, payload))
    }

    /// Encrypts and sends a complete SV2 frame (header and payload).
    pub fn send(&mut self, wire: &mut impl Wire, frame: &[u8]) -> bool {
        let Some((header, payload)) = self.encrypt_frame(frame) else {
            return false;
        };
        wire.write_all(&header) && payload.map_or(true, |p| wire.write_all(&p))
    }
}

impl Receiver {
    fn next_nonce(&mut self) -> u64 {
        let n = self.nonce;
        self.nonce += 1;
        n
    }

    /// Decrypts the next encrypted frame header (the nonce advances even on
    /// failure, as in the C code; the connection is dropped then anyway).
    pub fn decrypt_header(
        &mut self,
        enc: &[u8; ENC_HEADER_LEN],
    ) -> Option<[u8; FRAME_HEADER_SIZE]> {
        let n = self.next_nonce();
        let mut hdr = [0u8; FRAME_HEADER_SIZE];
        decrypt_into(&self.key, n, &[], enc, &mut hdr).then_some(hdr)
    }

    /// Decrypts the next payload (ciphertext plus tag) into `out`.
    pub fn decrypt_payload(&mut self, enc: &[u8], out: &mut [u8]) -> bool {
        let n = self.next_nonce();
        decrypt_into(&self.key, n, &[], enc, out)
    }

    /// Receives and decrypts the next frame: its header, with the payload in
    /// the front of `buf`. None on any failure (also a payload larger than
    /// `buf`).
    pub fn recv(
        &mut self,
        wire: &mut impl Wire,
        buf: &mut [u8],
    ) -> Option<[u8; FRAME_HEADER_SIZE]> {
        let mut enc_hdr = [0u8; ENC_HEADER_LEN];
        if !wire.read_exact(&mut enc_hdr) {
            return None;
        }
        let Some(hdr) = self.decrypt_header(&enc_hdr) else {
            log!(Error, TAG, "Failed to decrypt frame header");
            return None;
        };
        let len = parse_frame_header(&hdr).msg_length as usize;
        if len == 0 {
            return Some(hdr);
        }
        if len > buf.len() {
            log!(Error, TAG, "Payload too large: {len} > {}", buf.len());
            return None;
        }
        let mut enc = Vec::new();
        if enc.try_reserve_exact(len + TAG_LEN).is_err() {
            return None;
        }
        enc.resize(len + TAG_LEN, 0);
        if !wire.read_exact(&mut enc) {
            return None;
        }
        if !self.decrypt_payload(&enc, &mut buf[..len]) {
            log!(Error, TAG, "Failed to decrypt payload");
            return None;
        }
        Some(hdr)
    }
}

impl Drop for Sender {
    fn drop(&mut self) {
        wipe(&mut self.key);
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        wipe(&mut self.key);
    }
}

/// The C code's self-test of ElligatorSwift ECDH with keys 1 and 2.
pub fn self_test(secp: &Context) -> bool {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    a[31] = 1;
    b[31] = 2;
    let (Some(ell_a), Some(ell_b)) = (
        secp.ellswift_create(&a, &[0; 32]),
        secp.ellswift_create(&b, &[0; 32]),
    ) else {
        return false;
    };
    // The C code passed NULL as aux; the encoding differs, the secret must not.
    matches!(
        (secp.ellswift_xdh(&ell_a, &ell_b, &a, 0), secp.ellswift_xdh(&ell_a, &ell_b, &b, 1)),
        (Some(x), Some(y)) if x == y
    )
}

/// The byte stream under the Noise layer.
pub trait Wire {
    /// Fills all of `buf`; false on an error, a timeout or a closed peer.
    fn read_exact(&mut self, buf: &mut [u8]) -> bool;
    /// Sends all of `buf`; false on an error or a timeout.
    fn write_all(&mut self, buf: &[u8]) -> bool;
}

/// Runs the Noise_NX handshake as initiator over `wire`. With `authority`
/// (an x-only public key) the responder's certificate must be signed by it
/// and, with `now` (Unix time, when the clock is synced), valid at that time.
pub fn handshake(
    wire: &mut impl Wire,
    authority: Option<&[u8; 32]>,
    now: Option<u32>,
) -> Option<Cipher> {
    static SELF_TEST_DONE: AtomicBool = AtomicBool::new(false);
    let Some(secp) = Context::new(&random32()) else {
        log!(Error, TAG, "Failed to create secp256k1 context");
        return None;
    };
    if !SELF_TEST_DONE.load(Ordering::Relaxed) {
        if !self_test(&secp) {
            log!(
                Error,
                TAG,
                "secp256k1 library self-test FAILED - library may be misconfigured"
            );
            return None;
        }
        SELF_TEST_DONE.store(true, Ordering::Relaxed);
    }

    let e_priv = random32();
    let aux = random32();
    let (hs, e_pub) = match Handshake::start(&secp, e_priv, &aux) {
        Ok(started) => started,
        Err(e) => {
            log!(Error, TAG, "Noise handshake failed: {e:?}");
            return None;
        }
    };
    if !wire.write_all(&e_pub) {
        log!(Error, TAG, "Failed to send ephemeral key");
        return None;
    }
    let mut act2 = [0u8; ACT2_LEN];
    if !wire.read_exact(&mut act2) {
        log!(Error, TAG, "Failed to receive server Noise response");
        return None;
    }
    match hs.finish(&act2, authority, now) {
        Ok((cipher, cert)) => {
            log!(
                Info,
                TAG,
                "Server certificate: version={}, valid_from={}, not_valid_after={}",
                cert.version,
                cert.valid_from,
                cert.not_valid_after
            );
            if authority.is_some() {
                if now.is_none() {
                    log!(
                        Warn,
                        TAG,
                        "Time not synced, certificate validity period not checked"
                    );
                }
                log!(Info, TAG, "Server certificate verified OK");
            } else {
                log!(
                    Warn,
                    TAG,
                    "Skipping certificate verification (no authority pubkey)"
                );
            }
            Some(cipher)
        }
        Err(e) => {
            match (&e, now) {
                (
                    HandshakeError::CertificateNotYetValid(_)
                    | HandshakeError::CertificateExpired(_),
                    Some(now),
                ) => log!(Error, TAG, "Noise handshake failed: {e:?} at time {now}"),
                _ => log!(Error, TAG, "Noise handshake failed: {e:?}"),
            }
            None
        }
    }
}

/// `esp_transport_handle_t`
pub type EspHandle = *mut c_void;

#[cfg(target_os = "espidf")]
unsafe fn esp_transport_read(
    t: EspHandle,
    buf: *mut c_char,
    len: c_int,
    timeout_ms: c_int,
) -> c_int {
    crate::idf::raw::esp_transport_read(t.cast(), buf, len, timeout_ms)
}
#[cfg(target_os = "espidf")]
unsafe fn esp_transport_write(
    t: EspHandle,
    buf: *const c_char,
    len: c_int,
    timeout_ms: c_int,
) -> c_int {
    crate::idf::raw::esp_transport_write(t.cast(), buf, len, timeout_ms)
}
#[cfg(not(target_os = "espidf"))]
extern "C" {
    fn esp_transport_read(t: EspHandle, buf: *mut c_char, len: c_int, timeout_ms: c_int) -> c_int;
    fn esp_transport_write(
        t: EspHandle,
        buf: *const c_char,
        len: c_int,
        timeout_ms: c_int,
    ) -> c_int;
}

const WRITE_TIMEOUT_MS: c_int = 5000;
const READ_TIMEOUT_MS: c_int = 3 * 60 * 1000;

/// An `esp_transport` connection as the [`Wire`].
pub struct EspWire(EspHandle);

impl EspWire {
    /// # Safety
    /// `t` is a connected transport that stays valid while this value is
    /// used.
    pub unsafe fn new(t: EspHandle) -> Self {
        Self(t)
    }
}

impl Wire for EspWire {
    fn read_exact(&mut self, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            // SAFETY: a valid transport (see new()); writes at most the
            // remaining length into buf.
            let r = unsafe {
                esp_transport_read(
                    self.0,
                    buf[done..].as_mut_ptr() as _,
                    (buf.len() - done) as c_int,
                    READ_TIMEOUT_MS,
                )
            };
            if r <= 0 {
                log!(Error, TAG, "recv failed: r={r}");
                return false;
            }
            done += r as usize;
        }
        true
    }

    /// 0 from esp_transport_write means the write timed out: that is an
    /// error, since the peer would miss bytes.
    fn write_all(&mut self, buf: &[u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            // SAFETY: a valid transport (see new()); reads at most the
            // remaining length from buf.
            let r = unsafe {
                esp_transport_write(
                    self.0,
                    buf[done..].as_ptr() as _,
                    (buf.len() - done) as c_int,
                    WRITE_TIMEOUT_MS,
                )
            };
            if r <= 0 {
                log!(Error, TAG, "send failed: ret={r}");
                return false;
            }
            done += r as usize;
        }
        true
    }
}

/// The C API (`sv2_noise.h`): what `c-reference/tests/sv2_noise.rs`
/// compares with the C implementation.
pub(crate) mod ffi {
    use super::*;

    /// `sv2_noise_ctx_t`
    pub struct NoiseCtx {
        cipher: Option<Cipher>,
    }

    #[no_mangle]
    pub extern "C" fn sv2_noise_create() -> *mut NoiseCtx {
        Box::into_raw(Box::new(NoiseCtx { cipher: None }))
    }

    /// # Safety
    /// `ctx` must come from `sv2_noise_create()` (or be NULL).
    #[no_mangle]
    pub unsafe extern "C" fn sv2_noise_destroy(ctx: *mut NoiseCtx) {
        if !ctx.is_null() {
            drop(Box::from_raw(ctx)); // wipes the keys
        }
    }

    /// `now`: Unix time, or 0 while the clock is not synced (the certificate
    /// validity period is not checked then).
    ///
    /// # Safety
    /// `ctx` and `transport` must be valid; `authority_pubkey` NULL or 32
    /// bytes.
    #[no_mangle]
    pub unsafe extern "C" fn sv2_noise_handshake(
        ctx: *mut NoiseCtx,
        transport: EspHandle,
        authority_pubkey: *const [u8; 32],
        now: u32,
    ) -> c_int {
        let authority = authority_pubkey.as_ref();
        let now = (now != 0).then_some(now);
        match handshake(&mut EspWire::new(transport), authority, now) {
            Some(cipher) => {
                (*ctx).cipher = Some(cipher);
                0
            }
            None => -1,
        }
    }

    /// # Safety
    /// `ctx` and `transport` must be valid and `frame` hold `frame_len`
    /// bytes.
    #[no_mangle]
    pub unsafe extern "C" fn sv2_noise_send(
        ctx: *mut NoiseCtx,
        transport: EspHandle,
        frame: *const u8,
        frame_len: c_int,
    ) -> c_int {
        let Some(cipher) = ctx.as_mut().and_then(|c| c.cipher.as_mut()) else {
            return -1;
        };
        if frame_len < FRAME_HEADER_SIZE as c_int {
            return -1;
        }
        let frame = core::slice::from_raw_parts(frame, frame_len as usize);
        if cipher.send.send(&mut EspWire::new(transport), frame) {
            0
        } else {
            -1
        }
    }

    /// # Safety
    /// `ctx` and `transport` must be valid, `hdr_out` 6 bytes, `payload_out`
    /// hold `max_payload_len` bytes, `payload_len_out` writable.
    #[no_mangle]
    pub unsafe extern "C" fn sv2_noise_recv(
        ctx: *mut NoiseCtx,
        transport: EspHandle,
        hdr_out: *mut [u8; FRAME_HEADER_SIZE],
        payload_out: *mut u8,
        max_payload_len: c_int,
        payload_len_out: *mut c_int,
    ) -> c_int {
        let Some(cipher) = ctx.as_mut().and_then(|c| c.cipher.as_mut()) else {
            return -1;
        };
        *payload_len_out = 0;
        let buf: &mut [u8] = if payload_out.is_null() || max_payload_len <= 0 {
            &mut []
        } else {
            core::slice::from_raw_parts_mut(payload_out, max_payload_len as usize)
        };
        match cipher.recv.recv(&mut EspWire::new(transport), buf) {
            Some(hdr) => {
                *hdr_out = hdr;
                *payload_len_out = parse_frame_header(&hdr).msg_length as c_int;
                0
            }
            None => -1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn certificate_validity_with_clock_skew() {
        let cert = Certificate {
            version: 0,
            valid_from: 1_000_000,
            not_valid_after: 1_043_200,
        };
        let check = |now| cert.check_validity(now);
        assert_eq!(check(1_000_000), Ok(()));
        assert_eq!(check(1_043_200), Ok(()));
        // our clock a little behind or ahead of the pool's
        assert_eq!(check(1_000_000 - CLOCK_SKEW_S), Ok(()));
        assert_eq!(check(1_043_200 + CLOCK_SKEW_S), Ok(()));
        assert_eq!(
            check(1_000_000 - CLOCK_SKEW_S - 1),
            Err(HandshakeError::CertificateNotYetValid(cert))
        );
        assert_eq!(
            check(1_043_200 + CLOCK_SKEW_S + 1),
            Err(HandshakeError::CertificateExpired(cert))
        );
        // no overflow at the ends of the range
        let open = Certificate {
            version: 0,
            valid_from: 0,
            not_valid_after: u32::MAX,
        };
        assert_eq!(open.check_validity(0), Ok(()));
        assert_eq!(open.check_validity(u32::MAX), Ok(()));
    }
}
