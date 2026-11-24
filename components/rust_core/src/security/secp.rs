//! The parts of libsecp256k1 the Noise handshake needs. The library is the
//! firmware's existing C component (`components/libsecp256k1`, with the
//! ellswift, schnorrsig and extrakeys modules); on the host, build.rs compiles
//! the same sources.

use core::ffi::{c_int, c_uint, c_void};

#[repr(C)]
pub struct RawContext {
    _private: [u8; 0],
}

/// `secp256k1_pubkey` / `secp256k1_xonly_pubkey` (opaque, 64 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Opaque64(pub [u8; 64]);

/// `secp256k1_keypair` (opaque, 96 bytes)
#[repr(C)]
#[derive(Clone, Copy)]
pub struct Keypair(pub [u8; 96]);

pub type XdhHashFn =
    unsafe extern "C" fn(*mut u8, *const u8, *const u8, *const u8, *mut c_void) -> c_int;

pub const CONTEXT_NONE: c_uint = 1;

extern "C" {
    pub fn secp256k1_context_create(flags: c_uint) -> *mut RawContext;
    pub fn secp256k1_context_destroy(ctx: *mut RawContext);
    pub fn secp256k1_context_randomize(ctx: *mut RawContext, seed32: *const u8) -> c_int;
    pub fn secp256k1_ellswift_create(
        ctx: *const RawContext,
        ell64: *mut u8,
        seckey32: *const u8,
        auxrnd32: *const u8,
    ) -> c_int;
    pub fn secp256k1_ellswift_decode(
        ctx: *const RawContext,
        pubkey: *mut Opaque64,
        ell64: *const u8,
    ) -> c_int;
    #[allow(clippy::too_many_arguments)]
    pub fn secp256k1_ellswift_xdh(
        ctx: *const RawContext,
        output: *mut u8,
        ell_a64: *const u8,
        ell_b64: *const u8,
        seckey32: *const u8,
        party: c_int,
        hashfp: XdhHashFn,
        data: *mut c_void,
    ) -> c_int;
    pub static secp256k1_ellswift_xdh_hash_function_bip324: XdhHashFn;
    pub fn secp256k1_xonly_pubkey_from_pubkey(
        ctx: *const RawContext,
        xonly: *mut Opaque64,
        parity: *mut c_int,
        pubkey: *const Opaque64,
    ) -> c_int;
    pub fn secp256k1_xonly_pubkey_serialize(
        ctx: *const RawContext,
        out32: *mut u8,
        xonly: *const Opaque64,
    ) -> c_int;
    pub fn secp256k1_xonly_pubkey_parse(
        ctx: *const RawContext,
        xonly: *mut Opaque64,
        in32: *const u8,
    ) -> c_int;
    pub fn secp256k1_schnorrsig_verify(
        ctx: *const RawContext,
        sig64: *const u8,
        msg: *const u8,
        msglen: usize,
        pubkey: *const Opaque64,
    ) -> c_int;
    pub fn secp256k1_keypair_create(
        ctx: *const RawContext,
        keypair: *mut Keypair,
        seckey32: *const u8,
    ) -> c_int;
    pub fn secp256k1_keypair_xonly_pub(
        ctx: *const RawContext,
        pubkey: *mut Opaque64,
        parity: *mut c_int,
        keypair: *const Keypair,
    ) -> c_int;
    pub fn secp256k1_schnorrsig_sign32(
        ctx: *const RawContext,
        sig64: *mut u8,
        msg32: *const u8,
        keypair: *const Keypair,
        aux32: *const u8,
    ) -> c_int;
}

/// An owned, randomized secp256k1 context.
pub struct Context(*mut RawContext);

// SAFETY: a libsecp256k1 context may be used from any thread as long as it
// is not used concurrently; the owners (Noise contexts) are not shared.
unsafe impl Send for Context {}

impl Context {
    /// None if the library fails to create or randomize it.
    pub fn new(seed: &[u8; 32]) -> Option<Self> {
        // SAFETY: plain library calls; the context is destroyed on failure.
        unsafe {
            let ctx = secp256k1_context_create(CONTEXT_NONE);
            if ctx.is_null() {
                return None;
            }
            if secp256k1_context_randomize(ctx, seed.as_ptr()) != 1 {
                secp256k1_context_destroy(ctx);
                return None;
            }
            Some(Self(ctx))
        }
    }

    pub fn raw(&self) -> *const RawContext {
        self.0
    }

    /// 64-byte ElligatorSwift encoding of the public key of `seckey`.
    pub fn ellswift_create(&self, seckey: &[u8; 32], aux: &[u8; 32]) -> Option<[u8; 64]> {
        let mut ell = [0u8; 64];
        // SAFETY: valid context and buffers.
        let ok = unsafe {
            secp256k1_ellswift_create(self.0, ell.as_mut_ptr(), seckey.as_ptr(), aux.as_ptr())
        };
        (ok == 1).then_some(ell)
    }

    /// BIP-324 ElligatorSwift ECDH; `party` 0 if our key is `ell_a`, 1 if `ell_b`.
    pub fn ellswift_xdh(
        &self,
        ell_a: &[u8; 64],
        ell_b: &[u8; 64],
        seckey: &[u8; 32],
        party: c_int,
    ) -> Option<[u8; 32]> {
        let mut out = [0u8; 32];
        // SAFETY: valid context and buffers; the hash function is the library's.
        let ok = unsafe {
            secp256k1_ellswift_xdh(
                self.0,
                out.as_mut_ptr(),
                ell_a.as_ptr(),
                ell_b.as_ptr(),
                seckey.as_ptr(),
                party,
                secp256k1_ellswift_xdh_hash_function_bip324,
                core::ptr::null_mut(),
            )
        };
        (ok == 1).then_some(out)
    }

    /// x-only serialization of an ElligatorSwift-encoded public key.
    pub fn ellswift_xonly(&self, ell: &[u8; 64]) -> Option<[u8; 32]> {
        // SAFETY: valid context and buffers.
        unsafe {
            let mut pubkey = Opaque64([0; 64]);
            // Always succeeds: every 64-byte string decodes to a point.
            secp256k1_ellswift_decode(self.0, &mut pubkey, ell.as_ptr());
            let mut xonly = Opaque64([0; 64]);
            let mut parity = 0;
            if secp256k1_xonly_pubkey_from_pubkey(self.0, &mut xonly, &mut parity, &pubkey) != 1 {
                return None;
            }
            let mut out = [0u8; 32];
            secp256k1_xonly_pubkey_serialize(self.0, out.as_mut_ptr(), &xonly);
            Some(out)
        }
    }

    /// BIP-340 verification; None if `pubkey` is not a valid x-only key.
    pub fn schnorr_verify(&self, sig: &[u8; 64], msg: &[u8], pubkey: &[u8; 32]) -> Option<bool> {
        // SAFETY: valid context and buffers.
        unsafe {
            let mut pk = Opaque64([0; 64]);
            if secp256k1_xonly_pubkey_parse(self.0, &mut pk, pubkey.as_ptr()) != 1 {
                return None;
            }
            Some(
                secp256k1_schnorrsig_verify(self.0, sig.as_ptr(), msg.as_ptr(), msg.len(), &pk)
                    == 1,
            )
        }
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        // SAFETY: created by secp256k1_context_create, destroyed once.
        unsafe { secp256k1_context_destroy(self.0) }
    }
}
