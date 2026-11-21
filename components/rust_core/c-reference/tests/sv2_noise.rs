//! Compares the Rust Noise transport with the former `sv2_noise.c` (compiled
//! for the host, functions renamed to `ref_sv2_noise_*`) through the same C
//! API, against a Noise NX responder implemented here.
//!
//! Both run with the same scripted random source and transport (the esp_*
//! stand-ins of rust_core's host build), so with identical behaviour they
//! send identical bytes, derive identical keys and decrypt identical frames.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::{c_char, c_int, c_void};

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::crypto::{encrypt, hkdf2};
use rust_core::mining::sha256;
use rust_core::secp::{self, Context, Keypair, Opaque64};
use rust_core::stratum_v2::noise::PROTOCOL_NAME;
use rust_core::stratum_v2::protocol::encode_frame_header;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

type Ctx = c_void;
type Transport = *mut c_void;

extern "C" {
    fn esp_host_set_hooks(
        read: extern "C" fn(Transport, *mut c_char, c_int, c_int) -> c_int,
        write: extern "C" fn(Transport, *mut c_char, c_int, c_int) -> c_int,
        random: extern "C" fn(*mut c_void, usize),
    );

    fn ref_sv2_noise_create() -> *mut Ctx;
    fn ref_sv2_noise_destroy(ctx: *mut Ctx);
    fn ref_sv2_noise_handshake(ctx: *mut Ctx, t: Transport, authority: *const u8) -> c_int;
    fn ref_sv2_noise_send(ctx: *mut Ctx, t: Transport, frame: *const u8, len: c_int) -> c_int;
    fn ref_sv2_noise_recv(
        ctx: *mut Ctx,
        t: Transport,
        hdr: *mut u8,
        payload: *mut u8,
        max: c_int,
        len: *mut c_int,
    ) -> c_int;

    fn sv2_noise_create() -> *mut Ctx;
    fn sv2_noise_destroy(ctx: *mut Ctx);
    fn sv2_noise_handshake(ctx: *mut Ctx, t: Transport, authority: *const u8, now: u32) -> c_int;
    fn sv2_noise_send(ctx: *mut Ctx, t: Transport, frame: *const u8, len: c_int) -> c_int;
    fn sv2_noise_recv(
        ctx: *mut Ctx,
        t: Transport,
        hdr: *mut u8,
        payload: *mut u8,
        max: c_int,
        len: *mut c_int,
    ) -> c_int;
}

// ---- scripted transport and random source (per test thread) ----

#[derive(Clone, Copy, PartialEq)]
enum WriteMode {
    Normal,
    /// esp_transport_write returns 0, as on a poll timeout
    Timeout,
    /// at most 7 bytes per call
    Partial,
}

/// Answers the initiator's first 64 bytes with the handshake message.
type Responder = Box<dyn FnMut(&[u8; 64]) -> Vec<u8>>;

struct Script {
    rng: u64,
    written: Vec<u8>,
    to_read: VecDeque<u8>,
    write_mode: WriteMode,
    responder: Option<Responder>,
}

thread_local! {
    static SCRIPT: RefCell<Script> = RefCell::new(Script {
        rng: 1,
        written: Vec::new(),
        to_read: VecDeque::new(),
        write_mode: WriteMode::Normal,
        responder: None,
    });
}

extern "C" fn hook_read(_t: Transport, buf: *mut c_char, len: c_int, _timeout: c_int) -> c_int {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        if s.to_read.is_empty() && s.written.len() >= 64 {
            if let Some(mut responder) = s.responder.take() {
                let act1: [u8; 64] = s.written[..64].try_into().unwrap();
                let act2 = responder(&act1);
                s.to_read.extend(act2);
            }
        }
        let n = (len as usize).min(s.to_read.len());
        for i in 0..n {
            // SAFETY: buf holds len bytes.
            unsafe { *buf.add(i) = s.to_read.pop_front().unwrap() as c_char };
        }
        if n == 0 {
            -1
        } else {
            n as c_int
        }
    })
}

extern "C" fn hook_write(_t: Transport, buf: *mut c_char, len: c_int, _timeout: c_int) -> c_int {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let n = match s.write_mode {
            WriteMode::Normal => len as usize,
            WriteMode::Timeout => return 0,
            WriteMode::Partial => (len as usize).min(7),
        };
        // SAFETY: buf holds len bytes.
        let data = unsafe { std::slice::from_raw_parts(buf as *const u8, n) };
        s.written.extend_from_slice(data);
        n as c_int
    })
}

extern "C" fn hook_random(buf: *mut c_void, len: usize) {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        for i in 0..len {
            s.rng ^= s.rng >> 12;
            s.rng ^= s.rng << 25;
            s.rng ^= s.rng >> 27;
            // SAFETY: buf holds len bytes.
            unsafe {
                *(buf as *mut u8).add(i) = (s.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8
            };
        }
    })
}

fn reset(seed: u64, responder: Option<Responder>) {
    // SAFETY: hooks are plain functions using thread-local state.
    unsafe { esp_host_set_hooks(hook_read, hook_write, hook_random) };
    SCRIPT.with(|s| {
        *s.borrow_mut() = Script {
            rng: seed | 1,
            written: Vec::new(),
            to_read: VecDeque::new(),
            write_mode: WriteMode::Normal,
            responder,
        };
    });
}

fn take_written() -> Vec<u8> {
    SCRIPT.with(|s| std::mem::take(&mut s.borrow_mut().written))
}

// ---- Noise NX responder ----

struct ResponderKeys {
    e_priv: [u8; 32],
    s_priv: [u8; 32],
    authority_priv: [u8; 32],
    valid_from: u32,
    not_valid_after: u32,
}

/// Keys the responder ends up with (send = initiator's receive key).
#[derive(Default, Clone, Copy)]
struct SessionKeys {
    to_initiator: [u8; 32],
}

fn authority_xonly(secp: &Context, authority_priv: &[u8; 32]) -> [u8; 32] {
    // SAFETY: valid context and buffers.
    unsafe {
        let mut kp = Keypair([0; 96]);
        assert_eq!(
            secp::secp256k1_keypair_create(secp.raw(), &mut kp, authority_priv.as_ptr()),
            1
        );
        let mut xonly = Opaque64([0; 64]);
        let mut parity = 0;
        secp::secp256k1_keypair_xonly_pub(secp.raw(), &mut xonly, &mut parity, &kp);
        let mut out = [0u8; 32];
        secp::secp256k1_xonly_pubkey_serialize(secp.raw(), out.as_mut_ptr(), &xonly);
        out
    }
}

/// Builds the responder message for `act1`; stores the session keys in `out`.
fn respond(keys: &ResponderKeys, act1: &[u8; 64], out: &RefCell<SessionKeys>) -> Vec<u8> {
    let secp = Context::new(&[7; 32]).unwrap();
    let mix = |h: &mut [u8; 32], d: &[u8]| *h = sha256(&[&h[..], d].concat());
    let mut h = sha256(PROTOCOL_NAME);
    let mut ck = h;
    mix(&mut h, b"");
    mix(&mut h, act1);
    mix(&mut h, b"");

    let re = secp.ellswift_create(&keys.e_priv, &[1; 32]).unwrap();
    mix(&mut h, &re);
    let shared = secp.ellswift_xdh(act1, &re, &keys.e_priv, 1).unwrap();
    let (ck1, temp_k) = hkdf2(&ck, &shared);
    ck = ck1;

    let s_pub = secp.ellswift_create(&keys.s_priv, &[2; 32]).unwrap();
    let c1 = encrypt(&temp_k, 0, &h, &s_pub);
    mix(&mut h, &c1);

    let shared2 = secp.ellswift_xdh(act1, &s_pub, &keys.s_priv, 1).unwrap();
    let (ck2, temp_k2) = hkdf2(&ck, &shared2);
    ck = ck2;

    let mut cert = Vec::new();
    cert.extend(0u16.to_le_bytes());
    cert.extend(keys.valid_from.to_le_bytes());
    cert.extend(keys.not_valid_after.to_le_bytes());
    let server_xonly = secp.ellswift_xonly(&s_pub).unwrap();
    let msg = sha256(&[&cert[..], &server_xonly].concat());
    let mut sig = [0u8; 64];
    // SAFETY: valid context and buffers.
    unsafe {
        let mut kp = Keypair([0; 96]);
        assert_eq!(
            secp::secp256k1_keypair_create(secp.raw(), &mut kp, keys.authority_priv.as_ptr()),
            1
        );
        assert_eq!(
            secp::secp256k1_schnorrsig_sign32(
                secp.raw(),
                sig.as_mut_ptr(),
                msg.as_ptr(),
                &kp,
                [3u8; 32].as_ptr()
            ),
            1
        );
    }
    cert.extend(sig);
    let c2 = encrypt(&temp_k2, 0, &h, &cert);

    let (_to_responder, to_initiator) = hkdf2(&ck, b"");
    *out.borrow_mut() = SessionKeys { to_initiator };
    [&re[..], &c1, &c2].concat()
}

// ---- the two implementations behind one interface ----

struct Api {
    create: unsafe extern "C" fn() -> *mut Ctx,
    destroy: unsafe extern "C" fn(*mut Ctx),
    handshake: unsafe extern "C" fn(*mut Ctx, Transport, *const u8) -> c_int,
    send: unsafe extern "C" fn(*mut Ctx, Transport, *const u8, c_int) -> c_int,
    recv: unsafe extern "C" fn(*mut Ctx, Transport, *mut u8, *mut u8, c_int, *mut c_int) -> c_int,
}

const REFERENCE: Api = Api {
    create: ref_sv2_noise_create,
    destroy: ref_sv2_noise_destroy,
    handshake: ref_sv2_noise_handshake,
    send: ref_sv2_noise_send,
    recv: ref_sv2_noise_recv,
};
/// The current handshake without a synced clock, like the reference.
unsafe extern "C" fn handshake_time_unknown(
    ctx: *mut Ctx,
    t: Transport,
    authority: *const u8,
) -> c_int {
    sv2_noise_handshake(ctx, t, authority, 0)
}

const CURRENT: Api = Api {
    create: sv2_noise_create,
    destroy: sv2_noise_destroy,
    handshake: handshake_time_unknown,
    send: sv2_noise_send,
    recv: sv2_noise_recv,
};

/// Everything observable from one session.
#[derive(Debug, PartialEq)]
struct Outcome {
    handshake: c_int,
    act1: Vec<u8>,
    sent: Vec<(c_int, Vec<u8>)>,
    received: Vec<(c_int, Vec<u8>, Vec<u8>)>,
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// The outcome as recorded in the golden file.
fn outcome_text(o: &Outcome) -> String {
    let mut s = format!("handshake {}\nact1 {}", o.handshake, hex(&o.act1));
    for (r, bytes) in &o.sent {
        s += &format!("\nsent {r} {}", hex(bytes));
    }
    for (r, hdr, payload) in &o.received {
        s += &format!("\nreceived {r} {} {}", hex(hdr), hex(payload));
    }
    s
}

fn keys(seed: u8) -> ResponderKeys {
    ResponderKeys {
        e_priv: [seed.wrapping_add(10); 32],
        s_priv: [seed.wrapping_add(20); 32],
        authority_priv: [seed.wrapping_add(30); 32],
        valid_from: 1_700_000_000,
        not_valid_after: 2_000_000_000,
    }
}

/// Runs a session: handshake (optionally with a wrong authority key or a
/// corrupted responder message), then sends and receives frames.
fn session(
    api: &Api,
    seed: u64,
    authority: Option<[u8; 32]>,
    corrupt: Option<usize>,
    frames: &[Vec<u8>],
) -> Outcome {
    let rk = keys(seed as u8);
    let session_keys = std::rc::Rc::new(RefCell::new(SessionKeys::default()));
    let sk = session_keys.clone();
    reset(
        seed,
        Some(Box::new(move |act1: &[u8; 64]| {
            let mut act2 = respond(&rk, act1, &sk);
            if let Some(i) = corrupt {
                let n = act2.len();
                act2[i % n] ^= 0x40;
            }
            act2
        })),
    );
    let t = 0x1234 as Transport;
    // SAFETY: the C API as the firmware uses it.
    unsafe {
        let ctx = (api.create)();
        assert!(!ctx.is_null());
        let handshake = (api.handshake)(
            ctx,
            t,
            authority.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()),
        );
        let act1 = take_written();
        let mut outcome = Outcome {
            handshake,
            act1,
            sent: Vec::new(),
            received: Vec::new(),
        };
        if handshake == 0 {
            for f in frames {
                let r = (api.send)(ctx, t, f.as_ptr(), f.len() as c_int);
                outcome.sent.push((r, take_written()));
            }
            // The responder sends the same frames back.
            let key = session_keys.borrow().to_initiator;
            let mut nonce = 0;
            for f in frames {
                let mut wire = encrypt(&key, nonce, &[], &f[..6]);
                nonce += 1;
                if f.len() > 6 {
                    wire.extend(encrypt(&key, nonce, &[], &f[6..]));
                    nonce += 1;
                }
                SCRIPT.with(|s| s.borrow_mut().to_read.extend(wire));
                let (mut hdr, mut payload, mut len) = ([0u8; 6], vec![0u8; 4096], 0);
                let r = (api.recv)(
                    ctx,
                    t,
                    hdr.as_mut_ptr(),
                    payload.as_mut_ptr(),
                    payload.len() as c_int,
                    &mut len,
                );
                payload.truncate(len as usize);
                outcome.received.push((r, hdr.to_vec(), payload));
            }
        }
        (api.destroy)(ctx);
        outcome
    }
}

fn frame(seed: u64, i: usize) -> Vec<u8> {
    let len = ((seed as usize).wrapping_mul(31).wrapping_add(i * 97)) % 300;
    let mut f = encode_frame_header(0x8000, 0x1a, len as u32).to_vec();
    f.extend((0..len).map(|j| (j as u64 ^ seed) as u8));
    f
}

#[test]
fn handshake_and_frames_match_reference() {
    let secp = Context::new(&[9; 32]).unwrap();
    let mut rec = Rec::new("sv2_noise", "handshake_and_frames_match_reference");
    for seed in 1..40u64 {
        let authority = authority_xonly(&secp, &keys(seed as u8).authority_priv);
        let frames: Vec<Vec<u8>> = (0..5).map(|i| frame(seed, i)).collect();
        for auth in [Some(authority), None] {
            let reference = session(&REFERENCE, seed, auth, None, &frames);
            let current = session(&CURRENT, seed, auth, None, &frames);
            assert_eq!(
                reference.handshake, 0,
                "seed {seed}: reference handshake failed"
            );
            assert_eq!(
                current,
                reference,
                "seed {seed}, authority {}",
                auth.is_some()
            );
            rec.add(format!(
                "seed {seed} authority {}\n{}",
                auth.is_some(),
                outcome_text(&current)
            ));
            // The frames came back intact.
            for ((r, hdr, payload), f) in current.received.iter().zip(&frames) {
                assert_eq!((*r, &hdr[..], &payload[..]), (0, &f[..6], &f[6..]));
            }
        }
    }
    rec.finish();
}

#[test]
fn handshake_failures_match_reference() {
    let secp = Context::new(&[9; 32]).unwrap();
    let mut rec = Rec::new("sv2_noise", "handshake_failures_match_reference");
    for seed in 1..20u64 {
        let authority = authority_xonly(&secp, &keys(seed as u8).authority_priv);
        let wrong = authority_xonly(&secp, &[0x55; 32]);
        let mut invalid_key = [0xffu8; 32]; // not an x coordinate on the curve
        invalid_key[0] = 0xff;
        let cases: [(Option<[u8; 32]>, Option<usize>); 5] = [
            (Some(wrong), None),
            (Some(invalid_key), None),
            (Some(authority), Some(70 + seed as usize)), // encrypted static key
            (Some(authority), Some(150 + seed as usize)), // encrypted certificate
            (None, Some(seed as usize)),                 // responder ephemeral key
        ];
        for (auth, corrupt) in cases {
            let reference = session(&REFERENCE, seed, auth, corrupt, &[]);
            let current = session(&CURRENT, seed, auth, corrupt, &[]);
            assert_eq!(current, reference, "seed {seed}, case {auth:?} {corrupt:?}");
            rec.add(format!(
                "seed {seed} authority {:?} corrupt {corrupt:?}\n{}",
                auth.map(|a| hex(&a)),
                outcome_text(&current)
            ));
            assert_eq!(
                current.handshake, -1,
                "seed {seed}, case {corrupt:?}: must fail"
            );
        }
    }
    rec.finish();
}

/// Writes: a write timeout (0) is an error, and a partial write is followed
/// by the rest (the reference returns after the first call).
#[test]
fn send_handles_timeouts_and_partial_writes() {
    let secp = Context::new(&[9; 32]).unwrap();
    let authority = authority_xonly(&secp, &keys(1).authority_priv);
    let f = frame(1, 3);
    for (api, name) in [(&REFERENCE, "reference"), (&CURRENT, "current")] {
        let mut results = Vec::new();
        for mode in [WriteMode::Timeout, WriteMode::Partial] {
            let rk = keys(1);
            let sk = std::rc::Rc::new(RefCell::new(SessionKeys::default()));
            let sk2 = sk.clone();
            reset(
                1,
                Some(Box::new(move |act1: &[u8; 64]| respond(&rk, act1, &sk2))),
            );
            let t = 0x1234 as Transport;
            // SAFETY: the C API as the firmware uses it.
            unsafe {
                let ctx = (api.create)();
                assert_eq!((api.handshake)(ctx, t, authority.as_ptr()), 0);
                take_written();
                SCRIPT.with(|s| s.borrow_mut().write_mode = mode);
                let r = (api.send)(ctx, t, f.as_ptr(), f.len() as c_int);
                results.push((r, take_written().len()));
                (api.destroy)(ctx);
            }
        }
        let expected_len = 22 + (f.len() - 6) + 16;
        if name == "reference" {
            // Timeout: "success" with nothing sent. Partial: "success" with 7+7 bytes.
            assert_eq!(results, vec![(0, 0), (0, 14)], "{name}");
        } else {
            assert_eq!(results, vec![(-1, 0), (0, expected_len)], "{name}");
            let mut rec = Rec::new("sv2_noise", "send_handles_timeouts_and_partial_writes");
            rec.add(format!("{results:?}"));
            rec.finish();
        }
    }
}

/// With an authority key and a known time, the certificate must be valid at
/// that time (the reference does not check the period).
#[test]
fn certificate_validity_period_is_checked() {
    let secp = Context::new(&[9; 32]).unwrap();
    let rk = keys(3);
    let (from, until) = (rk.valid_from, rk.not_valid_after);
    let authority = authority_xonly(&secp, &rk.authority_priv);
    let handshake = |api: &Api, auth: Option<&[u8; 32]>, now: Option<u32>| {
        let sk = std::rc::Rc::new(RefCell::new(SessionKeys::default()));
        reset(
            3,
            Some(Box::new(move |act1: &[u8; 64]| {
                respond(&keys(3), act1, &sk)
            })),
        );
        let auth = auth.map_or(std::ptr::null(), |a| a.as_ptr());
        // SAFETY: the C API as the firmware uses it.
        unsafe {
            let ctx = (api.create)();
            let r = match now {
                Some(now) => sv2_noise_handshake(ctx, 0x1234 as Transport, auth, now),
                None => (api.handshake)(ctx, 0x1234 as Transport, auth),
            };
            (api.destroy)(ctx);
            r
        }
    };
    let skew = rust_core::stratum_v2::noise::CLOCK_SKEW_S;
    let mut rec = Rec::new("sv2_noise", "certificate_validity_period_is_checked");
    for (now, ok) in [
        (from, true),
        (until, true),
        (from - skew, true),
        (until + skew, true),
        (from - skew - 1, false),
        (until + skew + 1, false),
    ] {
        let with = handshake(&CURRENT, Some(&authority), Some(now));
        assert_eq!(with == 0, ok, "now {now}");
        // without an authority key the certificate is not trusted anyway
        let without = handshake(&CURRENT, None, Some(now));
        assert_eq!(without, 0, "now {now}");
        rec.add(format!("now {now}: {with} {without}"));
    }
    rec.finish();
    // the reference accepts an expired certificate
    assert_eq!(handshake(&REFERENCE, Some(&authority), None), 0);
}
