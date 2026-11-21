//! Authentication gate of the HTTP API (port of the checks in `otp.cpp`,
//! `http_utils.cpp` and `http_cors.cpp`): TOTP codes (RFC 6238, HMAC-SHA1,
//! 30 s, 6 digits, +-1 step, replay mask), session tokens (HMAC-SHA256 over a
//! Base32 payload), the failed-attempt rate limit and the check that a
//! request comes from a private network.
//!
//! Behaviour notes:
//! * the session endpoint counts failed TOTP codes like every other
//!   protected endpoint,
//! * the requester's address must be private even when the `Origin` header
//!   names the device, `localhost` or a `.local` host,
//! * session token signatures are compared in constant time.

use std::net::{IpAddr, Ipv6Addr};
use std::sync::Mutex;

use crate::crypto::{ct_eq, hmac_sha1, hmac_sha256};

const B32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// RFC 4648 Base32, uppercase, without padding.
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 8 + 4) / 5);
    let (mut buf, mut bits) = (0u32, 0);
    for &b in data {
        buf = (buf << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(B32_ALPHABET[((buf >> (bits - 5)) & 0x1f) as usize] as char);
            bits -= 5;
        }
    }
    if bits > 0 {
        out.push(B32_ALPHABET[((buf << (5 - bits)) & 0x1f) as usize] as char);
    }
    out
}

/// Base32 decode into `out` (either case; other characters are skipped);
/// the number of bytes, or 0 if they do not fit.
pub fn base32_decode(input: &[u8], out: &mut [u8]) -> usize {
    let (mut buf, mut bits, mut n) = (0u32, 0, 0);
    for &c in input {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            _ => continue,
        };
        buf = (buf << 5) | v as u32;
        bits += 5;
        if bits >= 8 {
            if n >= out.len() {
                return 0;
            }
            out[n] = (buf >> (bits - 8)) as u8;
            n += 1;
            bits -= 8;
        }
    }
    n
}

/// Exactly six digits; spaces are ignored.
pub fn parse_otp6(code: &[u8]) -> Option<u32> {
    let mut value = 0u32;
    let mut digits = 0;
    for &c in code {
        if c == b' ' {
            continue;
        }
        if !c.is_ascii_digit() {
            return None;
        }
        value = value * 10 + (c - b'0') as u32;
        digits += 1;
        if digits > 6 {
            return None;
        }
    }
    (digits == 6).then_some(value)
}

/// HOTP (RFC 4226) with HMAC-SHA1, 6 digits.
pub fn hotp(key: &[u8], counter: u64) -> u32 {
    let mac = hmac_sha1(key, &counter.to_be_bytes());
    let off = (mac[19] & 0x0f) as usize;
    let bin = u32::from_be_bytes(mac[off..off + 4].try_into().unwrap()) & 0x7fff_ffff;
    bin % 1_000_000
}

/// Replay protection: the base time step and which of base-1, base, base+1
/// were used already (bits 0..2). Kept in NVS by the caller.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReplayState {
    pub base_step: i64,
    pub mask: u8,
}

/// Checks a TOTP code (30 s steps, one step of tolerance either way); a code
/// is accepted once per step.
pub fn totp_verify(key: &[u8], now: i64, code: u32, state: &mut ReplayState) -> bool {
    let step = now / 30;
    if state.base_step == 0 || step > state.base_step + 1 {
        *state = ReplayState {
            base_step: step,
            mask: 0,
        };
    } else if step < state.base_step - 1 {
        return false; // far too old
    }
    for s in step - 1..=step + 1 {
        let idx = s - (state.base_step - 1);
        if !(0..=2).contains(&idx) || state.mask & (1 << idx) != 0 {
            continue;
        }
        if hotp(key, s as u64) == code {
            state.mask |= 1 << idx;
            return true;
        }
    }
    false
}

/// Checks a TOTP code given as text against the Base32 secret.
pub fn verify_totp_code(code: &[u8], secret_b32: &[u8], now: i64, state: &mut ReplayState) -> bool {
    let Some(code) = parse_otp6(code) else {
        return false;
    };
    let mut key = [0u8; 32];
    let len = base32_decode(secret_b32, &mut key);
    len != 0 && totp_verify(&key[..len], now, code, state)
}

/// Session token: Base32 of the payload (issued, expires, boot ID, each u32
/// little endian), ".", Base32 of the HMAC-SHA256 over the payload text.
pub fn mint_session_token(key: &[u8; 32], boot_id: u32, now: i64, ttl: u32) -> Option<String> {
    if ttl == 0 {
        return None;
    }
    let mut payload = [0u8; 12];
    payload[..4].copy_from_slice(&(now as u32).to_le_bytes());
    payload[4..8].copy_from_slice(&((now + ttl as i64) as u32).to_le_bytes());
    payload[8..].copy_from_slice(&boot_id.to_le_bytes());
    let payload = base32_encode(&payload);
    let mac = hmac_sha256(key, payload.as_bytes());
    Some(format!("{payload}.{}", base32_encode(&mac)))
}

/// Valid signature, not expired, not issued more than 5 minutes in the
/// future, and from this boot ID.
pub fn verify_session_token(key: &[u8; 32], boot_id: u32, now: i64, token: &[u8]) -> bool {
    let Some(dot) = token.iter().position(|&c| c == b'.') else {
        return false;
    };
    let (payload_b32, sig_b32) = (&token[..dot], &token[dot + 1..]);
    if payload_b32.is_empty() || sig_b32.is_empty() {
        return false;
    }
    let mut payload = [0u8; 32];
    if base32_decode(payload_b32, &mut payload) != 12 {
        return false;
    }
    let mut sig = [0u8; 32];
    if base32_decode(sig_b32, &mut sig) != 32 {
        return false;
    }
    // constant time: the comparison does not stop at the first difference
    if !ct_eq(&hmac_sha256(key, payload_b32), &sig) {
        return false;
    }
    let word = |i: usize| u32::from_le_bytes(payload[i..i + 4].try_into().unwrap());
    let (iat, exp, bid) = (word(0) as i64, word(4) as i64, word(8));
    exp > now && iat <= now + 300 && bid == boot_id
}

/// At most 5 failed attempts within 60 s, then 5 minutes blocked.
#[derive(Default)]
pub struct RateLimit {
    /// failure times in ms, newest first
    failures: [u64; 5],
    block_until: u64,
}

impl RateLimit {
    pub const fn new() -> Self {
        RateLimit {
            failures: [0; 5],
            block_until: 0,
        }
    }

    pub fn blocked(&self, now_ms: u64) -> bool {
        self.block_until != 0 && self.block_until > now_ms
    }

    /// Records a failed attempt; false if it starts a block.
    pub fn record_failure(&mut self, now_ms: u64) -> bool {
        if self.block_until != 0 && self.block_until <= now_ms {
            *self = RateLimit::new();
        }
        self.failures.rotate_right(1);
        self.failures[0] = now_ms;
        let oldest = self.failures[4];
        if oldest != 0 && now_ms.wrapping_sub(oldest) < 60_000 {
            self.block_until = now_ms + 300_000;
            self.failures = [0; 5];
            return false;
        }
        true
    }
}

static RATE_LIMIT: Mutex<RateLimit> = Mutex::new(RateLimit::new());

fn rate_limit() -> std::sync::MutexGuard<'static, RateLimit> {
    RATE_LIMIT.lock().unwrap_or_else(|e| e.into_inner())
}

/// lwIP `ip4addr_aton()`: a, a.b, a.b.c or a.b.c.d, each part decimal, octal
/// (leading 0) or hex (0x), optionally followed by whitespace. Host order.
pub fn parse_ipv4(text: &[u8]) -> Option<u32> {
    let mut parts = [0u32; 4];
    let mut n = 0;
    let mut i = 0;
    let at = |i: usize| text.get(i).copied().unwrap_or(0);
    let val = loop {
        if !at(i).is_ascii_digit() {
            return None;
        }
        let mut val: u32 = 0;
        let mut base = 10;
        if at(i) == b'0' {
            i += 1;
            if at(i) == b'x' || at(i) == b'X' {
                base = 16;
                i += 1;
            } else {
                base = 8;
            }
        }
        loop {
            let c = at(i);
            if c.is_ascii_digit() {
                if base == 8 && c - b'0' >= 8 {
                    break;
                }
                val = val.wrapping_mul(base).wrapping_add((c - b'0') as u32);
            } else if base == 16 && c.is_ascii_hexdigit() {
                val = (val << 4) | (c.to_ascii_lowercase() - b'a' + 10) as u32;
            } else {
                break;
            }
            i += 1;
        }
        if at(i) == b'.' {
            if n >= 3 {
                return None;
            }
            parts[n] = val;
            n += 1;
            i += 1;
        } else {
            break val;
        }
    };
    // C isspace(): space, \t, \n, \v, \f, \r; the end of the string is NUL
    if !matches!(at(i), 0 | b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        return None;
    }
    // the C string ends at the first NUL
    let [a, b, c, _] = parts;
    match n {
        0 => Some(val),
        1 if val <= 0xff_ffff && a <= 0xff => Some(val | a << 24),
        2 if val <= 0xffff && a <= 0xff && b <= 0xff => Some(val | a << 24 | b << 16),
        3 if val <= 0xff && a <= 0xff && b <= 0xff && c <= 0xff => {
            Some(val | a << 24 | b << 16 | c << 8)
        }
        _ => None,
    }
}

/// RFC 1918 (host order).
pub fn is_private(ip: u32) -> bool {
    (0x0a00_0000..=0x0aff_ffff).contains(&ip)
        || (0xac10_0000..=0xac1f_ffff).contains(&ip)
        || (0xc0a8_0000..=0xc0a8_ffff).contains(&ip)
}

/// An IPv6 LAN address: link-local (fe80::/10) or unique local (fc00::/7).
pub fn is_private_v6(ip: &Ipv6Addr) -> bool {
    let s = ip.segments()[0];
    s & 0xffc0 == 0xfe80 || s & 0xfe00 == 0xfc00
}

/// Whether a requester is on the LAN; an IPv4-mapped IPv6 address is
/// judged by its IPv4 address.
pub fn is_private_addr(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private(v4.into()),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_private(v4.into()),
            None => is_private_v6(&v6),
        },
    }
}

/// The host of an `Origin` header `http://host[:port][/...]`; `None` for any
/// other scheme (the device serves http only).
pub fn origin_host(origin: &[u8]) -> Option<&[u8]> {
    let rest = origin.strip_prefix(b"http://")?;
    if rest.starts_with(b"[") {
        // an IPv6 literal, with its brackets
        let end = rest.iter().position(|&c| c == b']')?;
        return Some(&rest[..=end]);
    }
    let end = rest
        .iter()
        .position(|&c| c == b'/' || c == b':' || c == 0)
        .unwrap_or(rest.len());
    Some(&rest[..end])
}

/// Whether a request may use the API: in AP mode always; otherwise the
/// requester must have a private address (also when the `Origin` names the
/// device, `localhost` or `*.local`), and an `Origin`, if sent, must name
/// the device, `localhost`, a `.local` host or a private IPv4 or IPv6 address.
pub fn network_allowed(
    ap_mode: bool,
    request_ip: IpAddr,
    origin: Option<&[u8]>,
    hostname: &[u8],
) -> bool {
    if ap_mode {
        return true;
    }
    if !is_private_addr(request_ip) {
        return false;
    }
    let Some(origin) = origin else {
        return true; // direct access, not from a browser page
    };
    let Some(host) = origin_host(origin) else {
        return false;
    };
    host == hostname
        || host == b"localhost"
        || host.ends_with(b".local")
        || parse_ipv4(host).is_some_and(|ip| ip != u32::MAX && is_private(ip))
        || host
            .strip_prefix(b"[")
            .and_then(|h| h.strip_suffix(b"]"))
            .and_then(|h| core::str::from_utf8(h).ok()?.parse::<Ipv6Addr>().ok())
            .is_some_and(|ip| is_private_v6(&ip))
}

pub(crate) mod api {
    use super::*;

    pub fn auth_rate_limit_blocked(now_ms: u64) -> bool {
        rate_limit().blocked(now_ms)
    }

    pub fn auth_rate_limit_failure(now_ms: u64) -> bool {
        rate_limit().record_failure(now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4226_hotp() {
        let key = b"12345678901234567890";
        let expected = [
            755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489,
        ];
        for (i, &code) in expected.iter().enumerate() {
            assert_eq!(hotp(key, i as u64), code);
        }
    }

    #[test]
    fn rfc6238_totp_sha1() {
        // the 8-digit vectors, last 6 digits
        let key = b"12345678901234567890";
        for (t, code) in [
            (59, 287082),
            (1111111109, 81804),
            (1111111111, 50471),
            (1234567890, 5924),
            (2000000000, 279037),
        ] {
            let mut state = ReplayState::default();
            assert!(totp_verify(key, t, code, &mut state), "T={t}");
            // the same code again is a replay
            assert!(!totp_verify(key, t, code, &mut state), "replay T={t}");
        }
    }

    #[test]
    fn session_tokens() {
        let key = [7u8; 32];
        let t = mint_session_token(&key, 42, 1_000_000, 3600).unwrap();
        assert_eq!(t.len(), 20 + 1 + 52);
        assert!(verify_session_token(&key, 42, 1_000_000, t.as_bytes()));
        assert!(!verify_session_token(&key, 43, 1_000_000, t.as_bytes()));
        assert!(!verify_session_token(&key, 42, 1_003_600, t.as_bytes()));
        assert!(!verify_session_token(&[8; 32], 42, 1_000_000, t.as_bytes()));
        let mut bad = t.clone().into_bytes();
        bad[3] = if bad[3] == b'A' { b'B' } else { b'A' };
        assert!(!verify_session_token(&key, 42, 1_000_000, &bad));
    }

    #[test]
    fn rate_limit_blocks_after_five_failures_in_a_minute() {
        let mut rl = RateLimit::new();
        for i in 0..4 {
            assert!(rl.record_failure(1000 + i * 1000));
        }
        assert!(!rl.record_failure(5000));
        assert!(rl.blocked(6000) && !rl.blocked(305_000));
    }

    #[test]
    fn origin_cannot_bypass_the_private_address_check() {
        let public = IpAddr::V4(0x0808_0808.into());
        let lan = IpAddr::V4(0xc0a8_0105.into());
        for origin in [
            &b"http://localhost"[..],
            b"http://miner.local",
            b"http://nerdqaxe",
        ] {
            assert!(!network_allowed(false, public, Some(origin), b"nerdqaxe"));
            assert!(network_allowed(false, lan, Some(origin), b"nerdqaxe"));
        }
        assert!(network_allowed(false, lan, None, b"x"));
        assert!(!network_allowed(
            false,
            lan,
            Some(b"https://192.168.1.5"),
            b"x"
        ));
        assert!(network_allowed(
            false,
            lan,
            Some(b"http://192.168.1.5:8080/x"),
            b"x"
        ));
        assert!(!network_allowed(false, lan, Some(b"http://8.8.8.8"), b"x"));
        assert!(network_allowed(true, public, Some(b"http://8.8.8.8"), b"x"));
    }

    #[test]
    fn ipv6_requesters_are_judged_by_their_whole_address() {
        let v6 = |s: &str| IpAddr::V6(s.parse().unwrap());
        // IPv4-mapped: the IPv4 rules
        assert!(network_allowed(false, v6("::ffff:192.168.1.5"), None, b"x"));
        assert!(!network_allowed(false, v6("::ffff:8.8.8.8"), None, b"x"));
        // LAN IPv6
        assert!(network_allowed(false, v6("fe80::1"), None, b"x"));
        assert!(network_allowed(
            false,
            v6("fd5b:7a1c:2e90:1::55"),
            None,
            b"x"
        ));
        // a global address ending in a private IPv4 address is not on the LAN
        assert!(!network_allowed(
            false,
            v6("2001:db8::c0a8:105"),
            None,
            b"x"
        ));
        assert!(!network_allowed(
            false,
            v6("2001:db8::ffff:c0a8:105"),
            None,
            b"x"
        ));
        assert!(!network_allowed(false, v6("::1:c0a8:105"), None, b"x"));
        // the Origin of a page opened by its IPv6 address
        let lan = v6("fd00::2");
        assert!(network_allowed(false, lan, Some(b"http://[fd00::1]"), b"x"));
        assert!(network_allowed(
            false,
            lan,
            Some(b"http://[fe80::1]:80/a"),
            b"x"
        ));
        assert!(!network_allowed(
            false,
            lan,
            Some(b"http://[2001:db8::1]"),
            b"x"
        ));
        assert!(!network_allowed(false, lan, Some(b"http://[fd00::1"), b"x"));
        assert_eq!(
            origin_host(b"http://[fd00::1]:80/x"),
            Some(&b"[fd00::1]"[..])
        );
    }
}
