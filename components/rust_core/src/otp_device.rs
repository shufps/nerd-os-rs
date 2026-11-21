//! OTP glue of the device (port of `main/otp/otp.cpp`): the device identity
//! for the otpauth URI, the QR code of an enrollment (the `qrcodegen`
//! crate, version 1..10, ECC medium, for the 170 px display), and the checks with the
//! wall clock. The OTP state itself is in `otp.rs`. C API: `otp_device.h`.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};

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
/// QR versions (the smallest, the display's limit)
const QR_MIN_VERSION: u8 = 1;
const QR_MAX_VERSION: u8 = 10;

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

/// The QR code of the enrollment.
static QR: Mutex<Option<QrCode>> = Mutex::new(None);

/// The QR code of `uri` (medium error correction, raised if it still fits,
/// automatic mask); None if it does not fit version 10.
fn encode_qr(uri: &str) -> Option<QrCode> {
    QrCode::encode_segments_advanced(
        &QrSegment::make_segments(uri),
        QrCodeEcc::Medium,
        Version::new(QR_MIN_VERSION),
        Version::new(QR_MAX_VERSION),
        None,
        true,
    )
    .ok()
}

/// The QR code of the active enrollment.
pub fn qr() -> Option<QrCode> {
    QR.lock().unwrap_or_else(|e| e.into_inner()).clone()
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
        Some(qr) => {
            log!(Info, TAG, "QR ready: size={} modules", qr.size());
            *QR.lock().unwrap_or_else(|e| e.into_inner()) = Some(qr);
            ESP_OK
        }
        None => {
            log!(
                Error,
                TAG,
                "QR encoding failed (URI too long for version {QR_MAX_VERSION}?)"
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_otpauth_uri_fits_the_display() {
        let uri = "otpauth://totp/NerdQAxe%2B%2B:nerdqaxe-AABBCCDDEEFF?secret=JBSWY3DPEHPK3PXPJBSWY3DPEHPK3PXP&issuer=NerdQAxe%2B%2B&algorithm=SHA1&digits=6&period=30";
        let qr = encode_qr(uri).expect("fits version 10");
        assert!(qr.size() >= 21 && qr.size() <= 57);
        assert!(encode_qr(&"x".repeat(400)).is_none());
    }
}
