//! Helpers that reproduce C library behaviour and allocate memory that C code
//! frees with `free()`.

use core::ffi::{c_char, c_void};

#[cfg(not(target_os = "espidf"))]
extern "C" {
    fn malloc(size: usize) -> *mut c_void;
}

extern "C" {
    fn free(p: *mut c_void);
}

/// `free()` for memory from `c_malloc()` (or the C side); null is ignored.
///
/// # Safety
/// `p` is null or a live allocation from `malloc()`/`c_malloc()`.
pub unsafe fn c_free(p: *mut c_void) {
    if !p.is_null() {
        free(p);
    }
}

#[cfg(target_os = "espidf")]
mod caps {
    pub use crate::idf::raw::{
        heap_caps_malloc_prefer, MALLOC_CAP_8BIT, MALLOC_CAP_INTERNAL, MALLOC_CAP_SPIRAM,
    };
}

/// `malloc()`, preferring PSRAM on the device like the firmware's `MALLOC()`
/// (which uses PSRAM only). The C side releases it with `free()`.
pub fn c_malloc(size: usize) -> *mut c_void {
    #[cfg(target_os = "espidf")]
    // SAFETY: plain allocation call; the varargs are the caps to try, in order.
    unsafe {
        use caps::*;
        heap_caps_malloc_prefer(
            size.max(1),
            2,
            MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT,
            MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT,
        )
    }
    #[cfg(not(target_os = "espidf"))]
    // SAFETY: plain allocation call.
    unsafe {
        malloc(size.max(1))
    }
}

/// Lowercase hex of `b`.
pub fn to_hex(b: &[u8]) -> String {
    use core::fmt::Write;
    b.iter()
        .fold(String::with_capacity(2 * b.len()), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// The bytes before the first NUL, i.e. what C code sees of a string.
pub fn until_nul(s: &[u8]) -> &[u8] {
    s.iter().position(|&b| b == 0).map_or(s, |end| &s[..end])
}

/// Decodes hex like the C `hex2bin()` in mining_utils.cpp: at most `out.len()`
/// bytes, stops at the end of `hex`, a trailing odd digit becomes the high
/// nibble of a last byte, invalid digits decode as 0. Returns bytes written.
pub fn hex2bin(hex: &[u8], out: &mut [u8]) -> usize {
    fn val(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        }
    }
    let mut written = 0;
    for (dst, pair) in out.iter_mut().zip(hex.chunks(2)) {
        *dst = val(pair[0]) << 4 | pair.get(1).map_or(0, |&c| val(c));
        written += 1;
    }
    written
}

/// `strtoul(s, NULL, 16)` as newlib implements it with a 32-bit `unsigned
/// long` (ESP32): leading whitespace, optional sign, optional `0x`, then hex
/// digits up to the first other character. Overflow saturates to
/// `u32::MAX`; a `-` negates the result modulo 2^32.
pub fn strtoul32_hex(s: &[u8]) -> u32 {
    let s = until_nul(s);
    let mut i = s
        .iter()
        .take_while(|&&c| matches!(c, b' ' | b'\t'..=b'\r'))
        .count();
    let negative = match s.get(i) {
        Some(b'-') => {
            i += 1;
            true
        }
        Some(b'+') => {
            i += 1;
            false
        }
        _ => false,
    };
    if s.get(i) == Some(&b'0') && matches!(s.get(i + 1), Some(b'x' | b'X')) {
        i += 2;
    }
    let mut acc: u32 = 0;
    let mut overflow = false;
    for &c in &s[i.min(s.len())..] {
        let digit = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => break,
        } as u32;
        match acc.checked_mul(16).and_then(|v| v.checked_add(digit)) {
            Some(v) if !overflow => acc = v,
            _ => overflow = true,
        }
    }
    if overflow {
        u32::MAX
    } else if negative {
        acc.wrapping_neg()
    } else {
        acc
    }
}

/// `x` with `decimals` decimals like `printf("%.Nf")`, without Rust's float
/// formatting (which costs about 10 KB of flash). Rounds half up; "inf" /
/// "nan" as printf; magnitudes from 1e18 on are printed without decimals.
pub fn fixed(x: f64, decimals: u32) -> String {
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x < 0.0 { "-inf" } else { "inf" }.into();
    }
    let sign = if x < 0.0 { "-" } else { "" };
    let a = x.abs();
    let scale = 10u64.pow(decimals);
    if a >= 1e18 / scale as f64 {
        return format!("{sign}{}", a as u64);
    }
    let v = (a * scale as f64 + 0.5) as u64;
    let sign = if v == 0 { "" } else { sign };
    if decimals == 0 {
        format!("{sign}{v}")
    } else {
        format!(
            "{sign}{}.{:0w$}",
            v / scale,
            v % scale,
            w = decimals as usize
        )
    }
}

/// `s` as the contents of a JSON string (quotes, backslashes and control
/// characters escaped).
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// `s` percent-encoded for a URL query value (all but the unreserved
/// characters of RFC 3986).
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Microseconds of a monotonic clock (`esp_timer_get_time()` on the device).
pub fn now_us() -> i64 {
    #[cfg(target_os = "espidf")]
    {
        extern "C" {
            fn esp_timer_get_time() -> i64;
        }
        // SAFETY: plain ESP-IDF call without arguments.
        unsafe { esp_timer_get_time() }
    }
    #[cfg(not(target_os = "espidf"))]
    {
        use std::sync::OnceLock;
        use std::time::Instant;
        static START: OnceLock<Instant> = OnceLock::new();
        START.get_or_init(Instant::now).elapsed().as_micros() as i64 + 1
    }
}

/// `bytes` with a terminating NUL in a `c_malloc()` buffer, the length (without
/// the NUL) in `*len`; null if out of memory.
///
/// # Safety
/// `len` is writable or null.
pub unsafe fn malloc_copy(bytes: &[u8], len: *mut usize) -> *mut c_char {
    let p = c_malloc(bytes.len() + 1) as *mut u8;
    if p.is_null() {
        return core::ptr::null_mut();
    }
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    *p.add(bytes.len()) = 0;
    if !len.is_null() {
        *len = bytes.len();
    }
    p as *mut c_char
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaping() {
        assert_eq!(json_escape("a\"b\\c\nd\u{1}"), "a\\\"b\\\\c\\nd\\u0001");
        assert_eq!(url_encode("my org/1&x=ä"), "my%20org%2F1%26x%3D%C3%A4");
        assert_eq!(url_encode("nerdqaxeplus-1_2.3~"), "nerdqaxeplus-1_2.3~");
    }

    #[test]
    fn fixed_matches_printf() {
        assert_eq!(fixed(1320.24, 1), "1320.2");
        assert_eq!(fixed(0.06, 1), "0.1");
        assert_eq!(fixed(60.3042, 3), "60.304");
        assert_eq!(fixed(-0.5, 3), "-0.500");
        assert_eq!(fixed(-0.0001, 2), "0.00");
        assert_eq!(fixed(2905.0, 2), "2905.00");
        assert_eq!(fixed(7.0, 0), "7");
        assert_eq!(fixed(f64::INFINITY, 2), "inf");
        assert_eq!(fixed(f64::NAN, 2), "nan");
        assert_eq!(fixed(3e18, 2), "3000000000000000000");
    }

    #[test]
    fn hex2bin_matches_c_semantics() {
        let mut out = [0xee; 4];
        assert_eq!(hex2bin(b"0aFz1", &mut out), 3);
        assert_eq!(out, [0x0a, 0xf0, 0x10, 0xee]);
    }

    #[test]
    fn strtoul32_matches_newlib() {
        for (s, v) in [
            (&b"20000000"[..], 0x2000_0000),
            (b"  0x1fffe000", 0x1fff_e000),
            (b"0X10", 0x10),
            (b"ffffffff", u32::MAX),
            (b"100000000", u32::MAX),
            (b"-1", u32::MAX),
            (b"-10", 0xffff_fff0),
            (b"+ff", 0xff),
            (b"12zz", 0x12),
            (b"", 0),
            (b"0x", 0),
            (b"zz", 0),
            (b"1\x002", 1),
        ] {
            assert_eq!(strtoul32_hex(s), v, "{:?}", String::from_utf8_lossy(s));
        }
    }
}
