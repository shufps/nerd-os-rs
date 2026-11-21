//! Differential tests of the HTTP API authentication: the former code
//! (c-auth/auth_ref.cpp, verbatim) against rust_core::http_auth, plus the
//! RFC test vectors in the unit tests.

use std::cell::Cell;
use std::ffi::{c_char, CString};
use std::sync::Mutex;

use c_reference as _;
use rust_core::http_auth::{self, RateLimit, ReplayState};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn ref_auth_base32_encode(data: *const u8, len: usize, out: *mut c_char, cap: usize) -> usize;
    fn ref_auth_base32_decode(text: *const c_char, len: usize, out: *mut u8, cap: usize) -> usize;
    fn ref_auth_parse_otp6(s: *const c_char, out: *mut u32) -> bool;
    fn ref_auth_totp_verify(
        key: *const u8,
        keylen: usize,
        now: i64,
        code: u32,
        base: *mut i64,
        mask: *mut u8,
    ) -> bool;
    fn ref_auth_session_mint(
        key: *const [u8; 32],
        boot: u32,
        now: i64,
        ttl: u32,
        out: *mut c_char,
        cap: usize,
    ) -> usize;
    fn ref_auth_session_verify(
        key: *const [u8; 32],
        boot: u32,
        now: i64,
        token: *const c_char,
        len: usize,
    ) -> bool;
    fn ref_auth_rate_limit_blocked(now_ms: u64) -> bool;
    fn ref_auth_rate_limit_failure(now_ms: u64) -> bool;
    fn ref_auth_rate_limit_reset();
    fn ref_auth_inet_addr(s: *const c_char) -> u32;
    fn ref_auth_network_allowed(
        ap: bool,
        ip_be: u32,
        origin: *const c_char,
        hostname: *const c_char,
    ) -> bool;
}

/// The reference rate limit is global state.
static REF_STATE: Mutex<()> = Mutex::new(());

struct Rng(Cell<u64>);
impl Rng {
    fn next(&self) -> u64 {
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn pick<'a, T>(&self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
    /// Text from an alphabet (no NUL).
    fn text(&self, alphabet: &[u8], max: usize) -> Vec<u8> {
        (0..self.below(max as u64 + 1))
            .map(|_| *self.pick(alphabet))
            .collect()
    }
}

#[test]
fn base32_matches_reference() {
    let rng = Rng(Cell::new(0xb32));
    let mut rec = Rec::new("http_auth", "base32_matches_reference");
    for _ in 0..20_000 {
        let data = rng.bytes(rng.below(40) as usize);
        let mut out = [0 as c_char; 80];
        let n = unsafe {
            ref_auth_base32_encode(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len())
        };
        let reference = unsafe { std::ffi::CStr::from_ptr(out.as_ptr()) }.to_bytes()[..n].to_vec();
        let encoded = http_auth::base32_encode(&data);
        assert_eq!(encoded.clone().into_bytes(), reference);

        // decode: valid, lowercase, junk, too long for the buffer
        let text = rng.text(
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefgz234567=. -18\x01\xff",
            70,
        );
        let cap = rng.below(40) as usize;
        let (mut a, mut b) = (vec![0u8; cap], vec![0u8; cap]);
        let n_ref =
            unsafe { ref_auth_base32_decode(text.as_ptr() as _, text.len(), a.as_mut_ptr(), cap) };
        let n_new = http_auth::base32_decode(&text, &mut b);
        assert_eq!(n_new, n_ref, "{text:?} cap {cap}");
        assert_eq!(a[..n_ref], b[..n_new]);
        rec.add(format!(
            "{} -> {encoded}\n\"{}\" cap {cap} -> {n_new} {}",
            data.escape_ascii(),
            text.escape_ascii(),
            b[..n_new].escape_ascii()
        ));
    }
    rec.finish();
}

#[test]
fn otp6_parsing_matches_reference() {
    let rng = Rng(Cell::new(0x0716));
    let mut rec = Rec::new("http_auth", "otp6_parsing_matches_reference");
    for _ in 0..50_000 {
        let text = rng.text(b"0123456789 a-+\t", 10);
        let c = CString::new(text.clone()).unwrap();
        let mut v = 0;
        let ok = unsafe { ref_auth_parse_otp6(c.as_ptr(), &mut v) };
        let parsed = http_auth::parse_otp6(&text);
        assert_eq!(parsed, ok.then_some(v), "{text:?}");
        rec.add(format!("\"{}\" -> {parsed:?}", text.escape_ascii()));
    }
    rec.finish();
}

/// Sequences of codes (right, wrong, replayed, neighbouring steps) against
/// the same replay state, across step changes.
#[test]
fn totp_matches_reference() {
    let rng = Rng(Cell::new(0x7077));
    let mut rec = Rec::new("http_auth", "totp_matches_reference");
    for _ in 0..300 {
        let key = rng.bytes(1 + rng.below(32) as usize);
        let mut t: i64 = 1_700_000_000 + rng.below(1 << 20) as i64;
        let (mut base, mut mask) = (0i64, 0u8);
        let mut state = ReplayState::default();
        for _ in 0..40 {
            t += [0, 1, 15, 29, 30, 31, 60, 95, -30, -61][rng.below(10) as usize];
            let step = t / 30;
            let code = match rng.below(4) {
                0 => http_auth::hotp(&key, (step + rng.below(5) as i64 - 2) as u64),
                1 => http_auth::hotp(&key, step as u64),
                2 => rng.below(1_000_000) as u32,
                _ => http_auth::hotp(&key, (step - 1) as u64),
            };
            let ok_ref = unsafe {
                ref_auth_totp_verify(key.as_ptr(), key.len(), t, code, &mut base, &mut mask)
            };
            let ok_new = http_auth::totp_verify(&key, t, code, &mut state);
            assert_eq!(
                (ok_new, state.base_step, state.mask),
                (ok_ref, base, mask),
                "t={t} code={code}"
            );
            rec.add(format!(
                "key {:02x?} t {t} code {code} -> {ok_new} base {} mask {:#04x}",
                key, state.base_step, state.mask
            ));
        }
    }
    rec.finish();
}

#[test]
fn session_tokens_match_reference() {
    let rng = Rng(Cell::new(0x5e55));
    let mut rec = Rec::new("http_auth", "session_tokens_match_reference");
    for _ in 0..3000 {
        let key: [u8; 32] = rng.bytes(32).try_into().unwrap();
        let boot = rng.next() as u32;
        let now = 1_700_000_000 + rng.below(1 << 24) as i64;
        let ttl = *rng.pick(&[0u32, 1, 60, 86_400, u32::MAX]);
        let mut out = [0 as c_char; 96];
        let n = unsafe { ref_auth_session_mint(&key, boot, now, ttl, out.as_mut_ptr(), out.len()) };
        let reference = unsafe { std::ffi::CStr::from_ptr(out.as_ptr()) }.to_bytes()[..n].to_vec();
        let minted = http_auth::mint_session_token(&key, boot, now, ttl).map(String::into_bytes);
        assert_eq!(minted.clone().unwrap_or_default(), reference);
        rec.add(format!(
            "key {:02x?} boot {boot} now {now} ttl {ttl} -> {:?}",
            key,
            minted
                .as_ref()
                .map(|t| String::from_utf8_lossy(t).into_owned())
        ));

        // verify: the token, modified tokens, other times, keys and boot IDs
        let token = minted.unwrap_or_else(|| rng.text(b"ABCXYZ234567.", 80));
        for _ in 0..8 {
            let mut t = token.clone();
            match rng.below(6) {
                0 if !t.is_empty() => {
                    let i = rng.below(t.len() as u64) as usize;
                    t[i] = *rng.pick(b"AB27.=ax ");
                }
                1 if !t.is_empty() => t.truncate(rng.below(t.len() as u64) as usize),
                2 => t.extend(rng.text(b"A2. ", 5)),
                3 => t = rng.text(b"ABCDEFGHIJ234567.", 90),
                _ => {}
            }
            let (k, b) = if rng.below(4) == 0 {
                ([1u8; 32], boot ^ 1)
            } else {
                (key, boot)
            };
            let at =
                now + [-301i64, -1, 0, 1, 299, 300, 86_399, 86_400, 1 << 33][rng.below(9) as usize];
            let ok_ref = unsafe { ref_auth_session_verify(&k, b, at, t.as_ptr() as _, t.len()) };
            let ok = http_auth::verify_session_token(&k, b, at, &t);
            assert_eq!(ok, ok_ref, "{:?}", String::from_utf8_lossy(&t));
            rec.add(format!(
                "verify \"{}\" key {:02x} boot {b} at {at} -> {ok}",
                t.escape_ascii(),
                k[0]
            ));
        }
    }
    rec.finish();
}

#[test]
fn rate_limit_matches_reference() {
    let _g = REF_STATE.lock().unwrap_or_else(|e| e.into_inner());
    let rng = Rng(Cell::new(0x0a7e));
    let mut rec = Rec::new("http_auth", "rate_limit_matches_reference");
    for _ in 0..200 {
        unsafe { ref_auth_rate_limit_reset() };
        let mut rl = RateLimit::new();
        let mut t: u64 = rng.below(1 << 30);
        for _ in 0..100 {
            t += *rng.pick(&[
                0u64, 1, 500, 5_000, 14_999, 15_000, 59_999, 60_000, 299_999, 300_000, 400_000,
            ]);
            let blocked = rl.blocked(t);
            assert_eq!(blocked, unsafe { ref_auth_rate_limit_blocked(t) }, "t={t}");
            let mut out = format!("t {t} blocked {blocked}");
            if rng.below(3) != 0 {
                let failure = rl.record_failure(t);
                assert_eq!(failure, unsafe { ref_auth_rate_limit_failure(t) }, "t={t}");
                out += &format!(" failure {failure}");
            }
            rec.add(out);
        }
    }
    rec.finish();
}

fn ipv4_text(rng: &Rng) -> Vec<u8> {
    match rng.below(4) {
        // dotted forms with decimal, octal and hex parts, 1..4 parts
        0 | 1 => {
            let parts = 1 + rng.below(4);
            let mut s = Vec::new();
            for i in 0..parts {
                if i > 0 {
                    s.push(b'.');
                }
                let v = *rng.pick(&[
                    0u64,
                    1,
                    10,
                    127,
                    168,
                    172,
                    192,
                    255,
                    256,
                    65535,
                    65536,
                    0xffffff,
                    0x1000000,
                    rng.below(1 << 32),
                ]);
                let part = match rng.below(3) {
                    0 => format!("{v}"),
                    1 => format!("0{v:o}"),
                    _ => format!("0x{v:x}"),
                };
                s.extend(part.bytes());
            }
            if rng.below(5) == 0 {
                s.extend(rng.text(b" \t9x.a", 2));
            }
            s
        }
        _ => rng.text(b"0123456789.xXaAfFgG \t-", 16),
    }
}

#[test]
fn inet_addr_matches_lwip() {
    let rng = Rng(Cell::new(0x1e7));
    let mut rec = Rec::new("http_auth", "inet_addr_matches_lwip");
    for _ in 0..100_000 {
        let text = ipv4_text(&rng);
        let c = CString::new(text.clone()).unwrap();
        let reference = u32::from_be(unsafe { ref_auth_inet_addr(c.as_ptr()) });
        // INADDR_NONE is also 255.255.255.255
        let parsed = http_auth::parse_ipv4(&text);
        assert_eq!(
            parsed.unwrap_or(u32::MAX),
            reference,
            "{:?}",
            String::from_utf8_lossy(&text)
        );
        rec.add(format!("\"{}\" -> {:x?}", text.escape_ascii(), parsed));
    }
    rec.finish();
}

/// The same decision as before, except: an Origin naming the
/// device, localhost or a .local host no longer admits a public requester.
#[test]
fn network_check_matches_reference_except_origin() {
    let rng = Rng(Cell::new(0xc025));
    let mut rec = Rec::new("http_auth", "network_check_matches_reference_except_origin");
    let hostname = b"nerdqaxe".to_vec();
    let mut changed = 0;
    for _ in 0..100_000 {
        let ap = rng.below(10) == 0;
        let ip: u32 = *rng.pick(&[
            0x0a00_0001,
            0x0aff_ffff,
            0x0b00_0000,
            0xac10_0001,
            0xac20_0000,
            0xc0a8_0101,
            0xc0a9_0000,
            0x0808_0808,
            rng.next() as u32,
        ]);
        let origin = match rng.below(6) {
            0 => None,
            1 => Some(rng.text(b"htp:/localnerdqaxe.19268", 30)),
            _ => {
                let host = match rng.below(5) {
                    0 => hostname.clone(),
                    1 => b"localhost".to_vec(),
                    2 => [rng.text(b"abc", 4), b".local".to_vec()].concat(),
                    _ => ipv4_text(&rng),
                };
                let schemes: [&[u8]; 4] = [b"http://", b"http://", b"https://", b"HTTP://"];
                let tails: [&[u8]; 4] = [b"", b":80", b"/x", b":8080/a:b"];
                let (scheme, tail) = (*rng.pick(&schemes), *rng.pick(&tails));
                Some([scheme, &host, tail].concat())
            }
        };
        let origin = origin.map(|mut o| {
            o.retain(|&c| c != 0);
            o.truncate(127);
            o
        });
        let c_origin = origin.as_ref().map(|o| CString::new(o.clone()).unwrap());
        let c_host = CString::new(hostname.clone()).unwrap();
        let reference = unsafe {
            ref_auth_network_allowed(
                ap,
                ip.to_be(),
                c_origin.as_ref().map_or(std::ptr::null(), |c| c.as_ptr()),
                c_host.as_ptr(),
            )
        };
        let current = http_auth::network_allowed(ap, ip, origin.as_deref(), &hostname);
        if current != reference {
            // only the bypass is closed: the reference allowed, the requester is public
            assert!(
                reference && !current && !http_auth::is_private(ip) && !ap,
                "{ip:08x} {origin:?}"
            );
            changed += 1;
        }
        rec.add(format!(
            "ap {ap} ip {ip:08x} origin {:?} -> {current}",
            origin.as_ref().map(|o| o.escape_ascii().to_string())
        ));
    }
    assert!(changed > 0, "the Origin case was generated");
    rec.finish();
}
