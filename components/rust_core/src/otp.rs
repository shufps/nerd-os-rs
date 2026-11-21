//! The OTP state (port of `OTP` in `main/otp/otp.cpp`): the TOTP secret,
//! the session key and boot ID for session tokens, enabled and enrollment
//! state, persisted in the configuration; checking a TOTP code with replay
//! protection, minting and checking session tokens, and the `otpauth://`
//! URI of a new enrollment (its QR code: `otp_device.rs`).
//! The algorithms are in `http_auth.rs`.
//!
//! All state is behind one lock, including the secret, the enabled flag
//! and loading/saving the settings.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::ajson::{Json, Num};
use crate::config;
use crate::cutil::url_encode;
use crate::http_auth::{
    base32_decode, base32_encode, mint_session_token, verify_session_token, verify_totp_code,
    ReplayState,
};
use crate::log::log;

const TAG: &CStr = c"OTP";

// the setting names of nvs_config.h
const KEY_SECRET: &[u8] = b"otp_secret";
const KEY_ENABLED: &[u8] = b"otp_enabled";
const KEY_LAST_STEP: &[u8] = b"otp_last_step";
const KEY_USED_MASK: &[u8] = b"otp_used_mask";
const KEY_SESSION_KEY: &[u8] = b"otp_sess_key";
const KEY_BOOT_ID: &[u8] = b"otp_boot_id";

struct Otp {
    enabled: bool,
    secret: String,
    boot_id: u32,
    session_key: Option<[u8; 32]>,
    enrollment: bool,
}

static OTP: Mutex<Otp> = Mutex::new(Otp {
    enabled: false,
    secret: String::new(),
    boot_id: 0,
    session_key: None,
    enrollment: false,
});

fn with<T>(f: impl FnOnce(&mut Otp) -> T) -> T {
    f(&mut OTP.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Random bytes (the hardware RNG on the device).
fn fill_random(buf: &mut [u8]) {
    #[cfg(target_os = "espidf")]
    {
        extern "C" {
            fn esp_fill_random(buf: *mut core::ffi::c_void, len: usize);
        }
        // SAFETY: buf is writable for its length
        unsafe { esp_fill_random(buf.as_mut_ptr().cast(), buf.len()) }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
        for b in buf {
            let mut x = STATE.load(Ordering::Relaxed);
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            STATE.store(x, Ordering::Relaxed);
            *b = x as u8;
        }
    }
}

/// Asks the configuration to be written (`Config::flush()`).
fn flush() {
    #[cfg(target_os = "espidf")]
    crate::config_nvs::request_flush();
}

impl Otp {
    fn create_session_key(&mut self) {
        let mut id = [0u8; 4];
        // the boot ID is never 0
        while u32::from_le_bytes(id) == 0 {
            fill_random(&mut id);
        }
        self.boot_id = u32::from_le_bytes(id);
        let mut key = [0u8; 32];
        fill_random(&mut key);
        self.session_key = Some(key);
    }

    fn save(&self) {
        config::set(KEY_BOOT_ID, Json::Num(Num::U(self.boot_id as u64)));
        let key = base32_encode(&self.session_key.unwrap_or([0; 32]));
        config::set(KEY_SESSION_KEY, Json::Str(key.into_bytes()));
        config::set(KEY_SECRET, Json::Str(self.secret.clone().into_bytes()));
        config::set(KEY_ENABLED, Json::Num(Num::U(self.enabled as u64)));
        set_replay_state(ReplayState {
            base_step: 0,
            mask: 0,
        });
        flush();
    }

    fn load(&mut self) {
        self.enabled = config::get_u16(KEY_ENABLED, 0) != 0;
        self.secret =
            String::from_utf8_lossy(&config::get_str(KEY_SECRET).unwrap_or_default()).into_owned();
        self.boot_id = config::get_u64(KEY_BOOT_ID, 0) as u32;
        self.session_key = None;
        let stored = config::get_str(KEY_SESSION_KEY).unwrap_or_default();
        if stored.is_empty() {
            // OTP enabled with a secret but no session key yet
            log!(Warn, TAG, "session key empty, initializing ...");
            self.create_session_key();
            self.save();
            return;
        }
        let mut key = [0u8; 32];
        if base32_decode(&stored, &mut key) == 32 {
            self.session_key = Some(key);
        } else {
            log!(Error, TAG, "Invalid session key in NVS");
        }
    }
}

fn replay_state() -> ReplayState {
    ReplayState {
        base_step: config::get_u64(KEY_LAST_STEP, 0) as i64,
        mask: config::get_u16(KEY_USED_MASK, 0) as u8,
    }
}

fn set_replay_state(s: ReplayState) {
    config::set(KEY_LAST_STEP, Json::Num(Num::U(s.base_step as u64)));
    config::set(KEY_USED_MASK, Json::Num(Num::U((s.mask & 0x07) as u64)));
}

/// The `otpauth://` URI of a secret (SHA1, 6 digits, 30 s are the defaults
/// and left out); the label is the hostname and 4 characters of the MAC.
pub fn otpauth_uri(hostname: &str, mac: &str, model: &str, secret: &str) -> String {
    let m = mac.as_bytes();
    let pick = |i: usize| m.get(i).map_or(String::new(), |&c| (c as char).to_string());
    let label = format!("{hostname}-{}{}{}{}", pick(0), pick(1), pick(3), pick(4));
    let issuer = url_encode(model);
    format!(
        "otpauth://totp/{issuer}:{}?secret={secret}&issuer={issuer}",
        url_encode(&label)
    )
}

/// The result of starting an enrollment.
#[derive(Debug, PartialEq, Eq)]
pub enum Enrollment {
    /// the URI for the QR code
    Started(String),
    AlreadyActive,
    /// OTP is enabled: no new secret
    Enabled,
}

/// Loads the saved state (secret, session key, enabled, replay state).
pub fn load() {
    with(Otp::load);
}

/// A new secret and session key; the enrollment stays active until it is
/// disabled (as before, also when OTP was enabled).
pub fn start_enrollment(hostname: &str, mac: &str, model: &str) -> Enrollment {
    with(|o| {
        if o.enrollment {
            log!(Warn, TAG, "enrollment already active");
            return Enrollment::AlreadyActive;
        }
        o.enrollment = true;
        if o.enabled {
            log!(Error, TAG, "OTP enabled, can't create new QR");
            return Enrollment::Enabled;
        }
        o.create_session_key();
        let mut secret = [0u8; 20];
        fill_random(&mut secret);
        o.secret = base32_encode(&secret);
        // a fresh secret has no used codes
        set_replay_state(ReplayState {
            base_step: 0,
            mask: 0,
        });
        flush();
        Enrollment::Started(otpauth_uri(hostname, mac, model, &o.secret))
    })
}

/// Checks a TOTP code (one step tolerance, each code once).
pub fn validate(token: &[u8], now: i64, time_synced: bool) -> bool {
    with(|o| {
        if o.secret.is_empty() {
            log!(Error, TAG, "No TOTP secret");
            return false;
        }
        let mut state = replay_state();
        if !time_synced {
            log!(Error, TAG, "System time not set");
            return false;
        }
        if !verify_totp_code(token, o.secret.as_bytes(), now, &mut state) {
            log!(Error, TAG, "invalid otp");
            return false;
        }
        log!(Info, TAG, "otp verified!");
        set_replay_state(state);
        flush();
        true
    })
}

pub fn mint(ttl: u32, now: i64) -> Option<String> {
    with(|o| mint_session_token(o.session_key.as_ref()?, o.boot_id, now, ttl))
}

pub fn verify(token: &[u8], now: i64) -> bool {
    with(|o| {
        o.session_key
            .as_ref()
            .is_some_and(|k| !token.is_empty() && verify_session_token(k, o.boot_id, now, token))
    })
}

/// The outcome of a request's OTP check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Auth {
    Ok = 0,
    /// too many failures: blocked for 5 minutes
    Blocked = 1,
    /// no valid session token or TOTP (the failure is counted)
    Failed = 2,
}

/// The check of a protected request (the former `validateOTP()`): without
/// OTP (and without `force`) everything passes; a valid session token
/// passes (not with `force`, which insists on a TOTP); else a valid TOTP.
/// `session`/`totp` are the `X-OTP-Session`/`X-TOTP` headers (None: absent
/// or too long).
pub fn check(
    enabled: bool,
    force: bool,
    session: Option<&[u8]>,
    totp: Option<&[u8]>,
    verify_session: impl FnOnce(&[u8]) -> bool,
    validate_totp: impl FnOnce(&[u8]) -> bool,
) -> bool {
    if !enabled && !force {
        return true;
    }
    if !force && session.is_some_and(verify_session) {
        return true;
    }
    totp.is_some_and(validate_totp)
}

/// `check()` behind the rate limit; a failure is counted.
pub fn request(
    force: bool,
    session: Option<&[u8]>,
    totp: Option<&[u8]>,
    now: i64,
    time_synced: bool,
    now_ms: u64,
) -> Auth {
    use crate::http_auth::ffi::{auth_rate_limit_blocked, auth_rate_limit_failure};
    if auth_rate_limit_blocked(now_ms) {
        return Auth::Blocked;
    }
    let enabled = with(|o| o.enabled);
    if check(
        enabled,
        force,
        session,
        totp,
        |t| verify(t, now),
        |t| validate(t, now, time_synced),
    ) {
        return Auth::Ok;
    }
    log!(Error, TAG, "totp validation failed");
    if !auth_rate_limit_failure(now_ms) {
        log!(Error, TAG, "too many OTP failures. Blocking ...");
    }
    Auth::Failed
}

/// The TOTP login of `/api/v2/otp/session` behind the rate limit (the TOTP
/// header must be present); a wrong code is counted.
pub fn login(totp: &[u8], now: i64, time_synced: bool, now_ms: u64) -> Auth {
    use crate::http_auth::ffi::{auth_rate_limit_blocked, auth_rate_limit_failure};
    if auth_rate_limit_blocked(now_ms) {
        return Auth::Blocked;
    }
    if validate(totp, now, time_synced) {
        return Auth::Ok;
    }
    if !auth_rate_limit_failure(now_ms) {
        log!(Error, TAG, "too many OTP failures. Blocking ...");
    }
    Auth::Failed
}

/// An enrollment (QR code) is active.
pub fn enrollment_active() -> bool {
    with(|o| o.enrollment)
}

/// Ends an active enrollment.
pub fn disable_enrollment() {
    with(|o| o.enrollment = false);
}

pub(crate) mod ffi {
    use super::*;

    pub extern "C" fn otp_save() {
        with(|o| o.save());
    }

    pub extern "C" fn otp_is_enabled() -> bool {
        with(|o| o.enabled)
    }

    pub extern "C" fn otp_set_enabled(enabled: bool) {
        with(|o| o.enabled = enabled);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    /// The decision of the former `check_otp_or_session()`, all inputs.
    #[test]
    fn request_check_decisions() {
        for enabled in [false, true] {
            for force in [false, true] {
                for session in [None, Some(&b"good"[..]), Some(&b"bad"[..])] {
                    for totp in [None, Some(&b"123456"[..]), Some(&b"000000"[..])] {
                        let (sv, tv) = (Cell::new(false), Cell::new(false));
                        let ok = check(
                            enabled,
                            force,
                            session,
                            totp,
                            |t| {
                                sv.set(true);
                                t == b"good"
                            },
                            |t| {
                                tv.set(true);
                                t == b"123456"
                            },
                        );
                        // without force: OTP off or a good session; else a good TOTP
                        let expected = (!force && (!enabled || session == Some(b"good")))
                            || totp == Some(b"123456");
                        assert_eq!(ok, expected, "{enabled} {force} {session:?} {totp:?}");
                        // force never looks at the session token
                        if force {
                            assert!(!sv.get());
                        }
                        // the TOTP only after the session token failed
                        if !force && enabled && session == Some(b"good") {
                            assert!(!tv.get());
                        }
                    }
                }
            }
        }
    }

    use super::*;
    use crate::http_auth::hotp;

    #[test]
    fn uri_like_the_cpp_code() {
        assert_eq!(
            otpauth_uri("nerd qaxe", "AC:A7:04:F5", "NerdQAxe++", "ABCDEF"),
            "otpauth://totp/NerdQAxe%2B%2B:nerd%20qaxe-ACA7?secret=ABCDEF&issuer=NerdQAxe%2B%2B"
        );
        // a short MAC does not read past it
        assert!(otpauth_uri("h", "A", "m", "S").contains(":h-A?"));
    }

    #[test]
    fn enrollment_validate_and_sessions() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::config::load(b"{}");
        with(|o| {
            o.enabled = false;
            o.enrollment = false;
        });
        let Enrollment::Started(uri) = start_enrollment("h", "AA:BB:CC", "m") else {
            panic!("not started");
        };
        assert_eq!(
            start_enrollment("h", "AA:BB:CC", "m"),
            Enrollment::AlreadyActive
        );
        let secret = uri
            .split("secret=")
            .nth(1)
            .unwrap()
            .split('&')
            .next()
            .unwrap()
            .to_string();
        assert_eq!(secret.len(), 32);

        // a valid code once, not twice
        let now = 1_794_900_000i64;
        let mut key = [0u8; 32];
        let n = base32_decode(secret.as_bytes(), &mut key);
        let code = format!("{:06}", hotp(&key[..n], (now / 30) as u64));
        assert!(!validate(code.as_bytes(), now, false));
        assert!(validate(code.as_bytes(), now, true));
        assert!(!validate(code.as_bytes(), now, true));
        assert!(!validate(b"000000x", now, true));

        // session tokens
        let token = mint(3600, now).unwrap();
        assert!(verify(token.as_bytes(), now + 10));
        assert!(!verify(token.as_bytes(), now + 7200));
        assert!(!verify(b"", now));

        // saved and loaded again
        with(|o| o.enabled = true);
        with(|o| o.save());
        with(|o| {
            o.secret.clear();
            o.session_key = None;
        });
        with(Otp::load);
        assert!(with(|o| o.enabled && o.secret == secret));
        assert!(verify(token.as_bytes(), now + 10));
        with(|o| o.enrollment = false);
        assert_eq!(start_enrollment("h", "m", "x"), Enrollment::Enabled);
    }
}
