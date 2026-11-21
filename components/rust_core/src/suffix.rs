//! Numbers with an SI suffix for the display and alerts (port of
//! `suffixString()` of `main/utils.cpp`): k, M, G, T, P, E with three
//! significant digits (`%.3g`) or a given number of them. The C library's
//! `snprintf` formats, so the text stays exactly as before.

use core::ffi::{c_char, c_double, c_int, c_uint};

extern "C" {
    fn snprintf(buf: *mut c_char, size: usize, fmt: *const c_char, ...) -> c_int;
}

/// The text of `val`.
pub fn suffix_string(mut val: u64, sig_digits: i32) -> String {
    const STEPS: [(u64, u64, &str); 6] = [
        (1_000_000_000_000_000_000, 1_000_000_000_000_000, "E"),
        (1_000_000_000_000_000, 1_000_000_000_000, "P"),
        (1_000_000_000_000, 1_000_000_000, "T"),
        (1_000_000_000, 1_000_000, "G"),
        (1_000_000, 1_000, "M"),
        (1_000, 1, "k"),
    ];
    let mut suffix = "";
    let mut decimal = true;
    let dval: f64;
    if let Some(&(_, div, s)) = STEPS.iter().find(|(min, _, _)| val >= *min) {
        val /= div;
        dval = val as f64 / 1000.0;
        suffix = s;
    } else {
        dval = val as f64;
        decimal = false;
    }
    let mut buf = [0u8; 64];
    let p = buf.as_mut_ptr() as *mut c_char;
    let n = buf.len();
    let sfx = std::ffi::CString::new(suffix).unwrap_or_default();
    // SAFETY: `p`/`n` describe `buf` (snprintf truncates and terminates); the
    // formats are static C strings whose conversions match the passed arguments
    unsafe {
        if sig_digits == 0 {
            if decimal {
                snprintf(p, n, c"%.3g%s".as_ptr(), dval as c_double, sfx.as_ptr());
            } else {
                snprintf(p, n, c"%d%s".as_ptr(), dval as c_uint, sfx.as_ptr());
            }
        } else {
            let lg = if dval > 0.0 {
                dval.log10().floor()
            } else {
                0.0
            };
            let digits = ((sig_digits - 1) as f64 - lg) as c_int;
            snprintf(
                p,
                n,
                c"%.*f%s".as_ptr(),
                digits.max(0),
                dval as c_double,
                sfx.as_ptr(),
            );
        }
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(0);
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

mod ffi {
    use super::*;

    /// Into `buf` (`size` bytes, terminated).
    ///
    /// # Safety
    /// `buf` writable for `size` bytes.
    #[no_mangle]
    pub unsafe extern "C" fn suffixString(
        val: u64,
        buf: *mut c_char,
        size: usize,
        sig_digits: c_int,
    ) {
        if buf.is_null() || size == 0 {
            return;
        }
        let s = suffix_string(val, sig_digits);
        let k = s.len().min(size - 1);
        core::ptr::copy_nonoverlapping(s.as_ptr(), buf as *mut u8, k);
        *buf.add(k) = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn examples() {
        assert_eq!(suffix_string(999, 0), "999");
        assert_eq!(suffix_string(1234, 0), "1.23k");
        assert_eq!(suffix_string(12_345_678, 0), "12.3M");
        assert_eq!(suffix_string(123_456_789_012, 3), "123G");
        assert_eq!(suffix_string(1_500_000_000_000, 3), "1.50T");
    }
}
