//! Golden tests of the Noise transport (`stratum_v2::noise`, behind the
//! `sv2_noise_*` C API) against a Noise NX responder implemented here:
//! handshakes (also failing ones), sent and received frames for a scripted
//! random source and transport, against the outputs of the former
//! `sv2_noise.c` (tests/golden/sv2_noise.txt, recorded by the differential
//! tests).

use std::cell::RefCell;
use std::collections::VecDeque;

use rust_core::crypto::{encrypt, hkdf2};
use rust_core::mining::sha256;
use rust_core::pool::stratum_v2::noise::{
    self_test, Cipher, Handshake, Wire, ACT2_LEN, PROTOCOL_NAME,
};
use rust_core::pool::stratum_v2::protocol::{
    encode_frame_header, parse_frame_header, FRAME_HEADER_SIZE,
};
use rust_core::secp::{self, Context, Keypair, Opaque64};

mod golden;
use golden::Rec;

// ---- scripted transport and random source (per test thread) ----

#[derive(Clone, Copy, PartialEq)]
enum WriteMode {
    Normal,
    /// the transport write returns 0, as on a poll timeout
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

/// One transport read (`esp_transport_read()` of the host build with the
/// scripted hook): up to `buf.len()` bytes, -1 if there are none.
fn transport_read(buf: &mut [u8]) -> i32 {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        if s.to_read.is_empty() && s.written.len() >= 64 {
            if let Some(mut responder) = s.responder.take() {
                let act1: [u8; 64] = s.written[..64].try_into().unwrap();
                let act2 = responder(&act1);
                s.to_read.extend(act2);
            }
        }
        let n = buf.len().min(s.to_read.len());
        for b in &mut buf[..n] {
            *b = s.to_read.pop_front().unwrap();
        }
        if n == 0 {
            -1
        } else {
            n as i32
        }
    })
}

/// One transport write (`esp_transport_write()` with the scripted hook).
fn transport_write(buf: &[u8]) -> i32 {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let n = match s.write_mode {
            WriteMode::Normal => buf.len(),
            WriteMode::Timeout => return 0,
            WriteMode::Partial => buf.len().min(7),
        };
        s.written.extend_from_slice(&buf[..n]);
        n as i32
    })
}

/// `esp_fill_random()` with the scripted hook.
fn random32() -> [u8; 32] {
    SCRIPT.with(|s| {
        let mut s = s.borrow_mut();
        let mut out = [0u8; 32];
        for b in &mut out {
            s.rng ^= s.rng >> 12;
            s.rng ^= s.rng << 25;
            s.rng ^= s.rng >> 27;
            *b = (s.rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 56) as u8;
        }
        out
    })
}

/// The transport as the firmware's `EspWire` uses it.
struct ScriptWire;

impl Wire for ScriptWire {
    fn read_exact(&mut self, buf: &mut [u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            let r = transport_read(&mut buf[done..]);
            if r <= 0 {
                return false;
            }
            done += r as usize;
        }
        true
    }
    fn write_all(&mut self, buf: &[u8]) -> bool {
        let mut done = 0;
        while done < buf.len() {
            let r = transport_write(&buf[done..]);
            if r <= 0 {
                return false;
            }
            done += r as usize;
        }
        true
    }
}

fn reset(seed: u64, responder: Option<Responder>) {
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

// ---- the initiator as `noise::handshake()` and the C API run it ----

/// `noise::handshake()` with the scripted random source: the context seed,
/// the ephemeral key and the aux randomness, in that order.
fn handshake(authority: Option<&[u8; 32]>, now: Option<u32>) -> Option<Cipher> {
    let secp = Context::new(&random32())?;
    if !self_test(&secp) {
        return None;
    }
    let e_priv = random32();
    let aux = random32();
    let (hs, e_pub) = Handshake::start(&secp, e_priv, &aux).ok()?;
    let mut wire = ScriptWire;
    if !wire.write_all(&e_pub) {
        return None;
    }
    let mut act2 = [0u8; ACT2_LEN];
    if !wire.read_exact(&mut act2) {
        return None;
    }
    hs.finish(&act2, authority, now)
        .ok()
        .map(|(cipher, _)| cipher)
}

/// `sv2_noise_send()`: 0 or -1.
fn send(cipher: &mut Cipher, frame: &[u8]) -> i32 {
    if frame.len() < FRAME_HEADER_SIZE {
        return -1;
    }
    if cipher.send.send(&mut ScriptWire, frame) {
        0
    } else {
        -1
    }
}

/// `sv2_noise_recv()` into a 4096 byte buffer: result, header, payload.
fn recv(cipher: &mut Cipher) -> (i32, Vec<u8>, Vec<u8>) {
    let mut payload = vec![0u8; 4096];
    match cipher.recv.recv(&mut ScriptWire, &mut payload) {
        Some(hdr) => {
            payload.truncate(parse_frame_header(&hdr).msg_length as usize);
            (0, hdr.to_vec(), payload)
        }
        None => (-1, vec![0; 6], Vec::new()),
    }
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

/// Everything observable from one session.
#[derive(Debug, PartialEq)]
struct Outcome {
    handshake: i32,
    act1: Vec<u8>,
    sent: Vec<(i32, Vec<u8>)>,
    received: Vec<(i32, Vec<u8>, Vec<u8>)>,
}

fn hex(b: &[u8]) -> String {
    rust_core::cutil::to_hex(b)
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
    // without a synced clock
    let cipher = handshake(authority.as_ref(), None);
    let act1 = take_written();
    let mut outcome = Outcome {
        handshake: if cipher.is_some() { 0 } else { -1 },
        act1,
        sent: Vec::new(),
        received: Vec::new(),
    };
    if let Some(mut cipher) = cipher {
        for f in frames {
            let r = send(&mut cipher, f);
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
            outcome.received.push(recv(&mut cipher));
        }
    }
    outcome
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
            let current = session(seed, auth, None, &frames);
            assert_eq!(current.handshake, 0, "seed {seed}: handshake failed");
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
            let current = session(seed, auth, corrupt, &[]);
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
/// by the rest (the C code returned after the first call).
#[test]
fn send_handles_timeouts_and_partial_writes() {
    let secp = Context::new(&[9; 32]).unwrap();
    let authority = authority_xonly(&secp, &keys(1).authority_priv);
    let f = frame(1, 3);
    let mut results = Vec::new();
    for mode in [WriteMode::Timeout, WriteMode::Partial] {
        let rk = keys(1);
        let sk = std::rc::Rc::new(RefCell::new(SessionKeys::default()));
        let sk2 = sk.clone();
        reset(
            1,
            Some(Box::new(move |act1: &[u8; 64]| respond(&rk, act1, &sk2))),
        );
        let mut cipher = handshake(Some(&authority), None).expect("handshake");
        take_written();
        SCRIPT.with(|s| s.borrow_mut().write_mode = mode);
        let r = send(&mut cipher, &f);
        results.push((r, take_written().len()));
    }
    let expected_len = 22 + (f.len() - 6) + 16;
    assert_eq!(results, vec![(-1, 0), (0, expected_len)]);
    let mut rec = Rec::new("sv2_noise", "send_handles_timeouts_and_partial_writes");
    rec.add(format!("{results:?}"));
    rec.finish();
}

/// With an authority key and a known time, the certificate must be valid at
/// that time (the C code did not check the period).
#[test]
fn certificate_validity_period_is_checked() {
    let secp = Context::new(&[9; 32]).unwrap();
    let rk = keys(3);
    let (from, until) = (rk.valid_from, rk.not_valid_after);
    let authority = authority_xonly(&secp, &rk.authority_priv);
    let run = |auth: Option<&[u8; 32]>, now: Option<u32>| {
        let sk = std::rc::Rc::new(RefCell::new(SessionKeys::default()));
        reset(
            3,
            Some(Box::new(move |act1: &[u8; 64]| {
                respond(&keys(3), act1, &sk)
            })),
        );
        if handshake(auth, now).is_some() {
            0
        } else {
            -1
        }
    };
    let skew = rust_core::pool::stratum_v2::noise::CLOCK_SKEW_S;
    let mut rec = Rec::new("sv2_noise", "certificate_validity_period_is_checked");
    for (now, ok) in [
        (from, true),
        (until, true),
        (from - skew, true),
        (until + skew, true),
        (from - skew - 1, false),
        (until + skew + 1, false),
    ] {
        let with = run(Some(&authority), Some(now));
        assert_eq!(with == 0, ok, "now {now}");
        // without an authority key the certificate is not trusted anyway
        let without = run(None, Some(now));
        assert_eq!(without, 0, "now {now}");
        rec.add(format!("now {now}: {with} {without}"));
    }
    rec.finish();
}
