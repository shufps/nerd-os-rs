//! OTP glue of the device (port of `main/otp/otp.cpp`): the device identity
//! for the otpauth URI, the QR code of an enrollment (qrcodegen, version
//! 1..10, ECC medium, for the 170 px display), and the checks with the
//! wall clock. The OTP state itself is in `otp.rs`. C API: `otp_device.h`.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use crate::log::log;
use crate::otp::{self, Enrollment};

const TAG: &CStr = c"OTP";

idf_const!(
    /// `esp_err_t` results of `otp_dev_start_enrollment()`
    pub ESP_OK: c_int = ESP_OK, 0
);
idf_const!(pub ESP_FAIL: c_int = ESP_FAIL, -1);
idf_const!(
    /// `ESP_ERR_NOT_FINISHED`: an enrollment is already active
    pub ESP_ERR_NOT_FINISHED: c_int = ESP_ERR_NOT_FINISHED, 0x10C
);

/// The longest otpauth URI (the former 256-byte buffer).
const URI_CAP: usize = 256;
/// QR versions (qrcodegen_VERSION_MIN, the display's limit)
const QR_MIN_VERSION: c_int = 1;
const QR_MAX_VERSION: c_int = 10;
/// `qrcodegen_Ecc_MEDIUM`, `qrcodegen_Mask_AUTO`
const QR_ECC_MEDIUM: c_int = 1;
const QR_MASK_AUTO: c_int = -1;

/// `qrcodegen_BUFFER_LEN_FOR_VERSION(n)`
pub const fn qr_buffer_len(version: usize) -> usize {
    let side = version * 4 + 17;
    (side * side).div_ceil(8) + 1
}

#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    extern "C" {
        #[allow(clippy::too_many_arguments)]
        pub fn qrcodegen_encodeText(
            text: *const c_char,
            temp: *mut u8,
            qrcode: *mut u8,
            ecl: c_int,
            min_version: c_int,
            max_version: c_int,
            mask: c_int,
            boost_ecl: bool,
        ) -> bool;
        pub fn qrcodegen_getSize(qrcode: *const u8) -> c_int;
    }
}

/// On the host there is no QR encoder.
#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case, clippy::too_many_arguments)]
mod sys {
    use super::*;
    pub unsafe fn qrcodegen_encodeText(
        _: *const c_char,
        _: *mut u8,
        _: *mut u8,
        _: c_int,
        _: c_int,
        _: c_int,
        _: c_int,
        _: bool,
    ) -> bool {
        false
    }
    pub unsafe fn qrcodegen_getSize(_: *const u8) -> c_int {
        0
    }
}

struct Device {
    hostname: String,
    mac: String,
    model: String,
}

static DEVICE: Mutex<Device> = Mutex::new(Device {
    hostname: String::new(),
    mac: String::new(),
    model: String::new(),
});

/// The QR code of the enrollment and its modules per side.
static QR: Mutex<Option<(Vec<u8>, c_int)>> = Mutex::new(None);

/// The QR code of `uri`; None if it does not fit version 10.
fn encode_qr(uri: &str) -> Option<(Vec<u8>, c_int)> {
    let need = qr_buffer_len(QR_MAX_VERSION as usize);
    let mut tmp = vec![0u8; need];
    let mut qr = vec![0u8; need];
    let Ok(text) = std::ffi::CString::new(uri) else {
        return None;
    };
    // SAFETY: `text` is NUL-terminated; `tmp` and `qr` both hold
    // qr_buffer_len(QR_MAX_VERSION) bytes, the most a version-10 code needs
    let size = unsafe {
        if !sys::qrcodegen_encodeText(
            text.as_ptr(),
            tmp.as_mut_ptr(),
            qr.as_mut_ptr(),
            QR_ECC_MEDIUM,
            QR_MIN_VERSION,
            QR_MAX_VERSION,
            QR_MASK_AUTO,
            true,
        ) {
            return None;
        }
        sys::qrcodegen_getSize(qr.as_ptr())
    };
    Some((qr, size))
}

/// The device identity (and the saved OTP state).
pub fn init(hostname: &str, mac: &str, model: &str) {
    *DEVICE.lock().unwrap_or_else(|e| e.into_inner()) = Device {
        hostname: hostname.into(),
        mac: mac.into(),
        model: model.into(),
    };
    otp::load();
}

/// A new secret and its QR code: `ESP_OK`, `ESP_ERR_NOT_FINISHED` (already
/// active) or `ESP_FAIL`.
pub fn start_enrollment() -> c_int {
    let (host, mac, model) = {
        let d = DEVICE.lock().unwrap_or_else(|e| e.into_inner());
        (d.hostname.clone(), d.mac.clone(), d.model.clone())
    };
    let uri = match otp::start_enrollment(&host, &mac, &model) {
        Enrollment::Started(u) if u.len() < URI_CAP => u,
        Enrollment::AlreadyActive => return ESP_ERR_NOT_FINISHED,
        _ => {
            log!(Error, TAG, "qr code couldn't be generated");
            return ESP_FAIL;
        }
    };
    match encode_qr(&uri) {
        Some((qr, size)) => {
            log!(Info, TAG, "QR ready: size={size} modules");
            *QR.lock().unwrap_or_else(|e| e.into_inner()) = Some((qr, size));
            ESP_OK
        }
        None => {
            log!(
                Error,
                TAG,
                "qrcodegen_encodeText failed (URI too long for maxVer={QR_MAX_VERSION}?)"
            );
            log!(Error, TAG, "qr code couldn't be generated");
            ESP_FAIL
        }
    }
}

/// The bytes of `p` (None for NULL).
///
/// # Safety
/// `p` is NULL or NUL-terminated and stays valid for `'a`.
unsafe fn opt<'a>(p: *const c_char) -> Option<&'a [u8]> {
    // SAFETY: NUL-terminated and alive (caller contract); NULL is checked
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_bytes())
}

pub(crate) mod ffi {
    use super::*;

    /// # Safety
    /// Strings NUL-terminated or NULL.
    pub unsafe extern "C" fn otp_dev_init(
        hostname: *const c_char,
        mac: *const c_char,
        model: *const c_char,
    ) {
        let s = |p| {
            // SAFETY: the strings of this call (its contract); copied at once
            unsafe { opt(p) }
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .unwrap_or_default()
        };
        init(&s(hostname), &s(mac), &s(model));
    }

    pub extern "C" fn otp_dev_start_enrollment() -> c_int {
        start_enrollment()
    }

    /// The QR code (`qrcodegen` format) and its size; NULL without one.
    /// Valid until the next enrollment or `otp_dev_destroy_qr()`.
    ///
    /// # Safety
    /// `size` writable.
    #[no_mangle]
    pub unsafe extern "C" fn otp_dev_qr(size: *mut c_int) -> *const u8 {
        match &*QR.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((qr, n)) => {
                *size = *n;
                qr.as_ptr()
            }
            None => {
                *size = 0;
                core::ptr::null()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_len_matches_qrcodegen() {
        // ((n*4+17)^2 + 7) / 8 + 1
        assert_eq!(qr_buffer_len(1), (21 * 21 + 7) / 8 + 1);
        assert_eq!(qr_buffer_len(10), 408);
    }
}
