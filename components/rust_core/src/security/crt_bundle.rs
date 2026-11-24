//! The certificate bundle for TLS (port of `components/crt_bundle`):
//! ESP-IDF's bundle, plus the acceptance of a cross-signed root. When a
//! server sends a chain that ends in a cross-signed copy of a root in the
//! bundle, mbedTLS finds no parent for it and marks only that topmost
//! certificate "not trusted"; the signatures below it are checked. If it
//! carries a bundled root's subject and public key, the chain ends at that
//! root.

use crate::idf::idf_const;
use core::ffi::{c_int, c_void, CStr};

use crate::idf::{EspErr, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"crt_bundle";

idf_const!(BADCERT_NOT_TRUSTED: u32 = MBEDTLS_X509_BADCERT_NOT_TRUSTED, 0x08);
idf_const!(BADCERT_BAD_MD: u32 = MBEDTLS_X509_BADCERT_BAD_MD, 0x4000);

/// the bundle format (gen_crt_bundle.py): u16 count, then per root u16
/// name length, u16 key length, subject (DER), SubjectPublicKeyInfo (DER)
const BUNDLE_HEADER_LEN: usize = 2;
const CRT_HEADER_LEN: usize = 4;

/// `mbedtls_x509_crt` (the generated type in the firmware)
#[cfg(target_os = "espidf")]
type Crt = crate::idf::raw::mbedtls_x509_crt;
#[cfg(not(target_os = "espidf"))]
type Crt = c_void;

type VerifyFn = unsafe extern "C" fn(*mut c_void, *mut Crt, c_int, *mut u32) -> c_int;

#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    use crate::idf::raw;
    pub use raw::esp_crt_bundle_attach;

    pub unsafe fn mbedtls_ssl_conf_verify(conf: *mut c_void, f: VerifyFn, p: *mut c_void) {
        crate::idf::raw::mbedtls_ssl_conf_verify(conf.cast(), Some(f), p)
    }

    extern "C" {
        #[link_name = "_binary_x509_crt_bundle_start"]
        pub static BUNDLE_START: u8;
        #[link_name = "_binary_x509_crt_bundle_end"]
        pub static BUNDLE_END: u8;
        /// not in the public header of ESP-IDF 5.3, but exported
        pub fn esp_crt_verify_callback(
            buf: *mut c_void,
            crt: *mut Crt,
            depth: c_int,
            flags: *mut u32,
        ) -> c_int;
    }

    unsafe fn buf<'a>(b: &raw::mbedtls_x509_buf) -> &'a [u8] {
        if b.p.is_null() {
            &[]
        } else {
            core::slice::from_raw_parts(b.p, b.len)
        }
    }
    /// The certificate's subject (DER).
    pub unsafe fn subject<'a>(crt: *mut Crt) -> &'a [u8] {
        buf(&(*crt).subject_raw)
    }
    /// The certificate's SubjectPublicKeyInfo (DER).
    pub unsafe fn public_key<'a>(crt: *mut Crt) -> &'a [u8] {
        buf(&(*crt).pk_raw)
    }

    pub fn bundle() -> &'static [u8] {
        // SAFETY: the linker symbols delimit the embedded bundle in flash (start <= end, same
        // object), which is read-only and lives for the whole program.
        unsafe {
            let start = core::ptr::addr_of!(BUNDLE_START);
            let end = core::ptr::addr_of!(BUNDLE_END);
            core::slice::from_raw_parts(start, end.offset_from(start) as usize)
        }
    }
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn esp_crt_bundle_attach(_: *mut c_void) -> EspErr {
        crate::idf::ESP_FAIL
    }
    pub unsafe fn esp_crt_verify_callback(
        _: *mut c_void,
        _: *mut Crt,
        _: c_int,
        _: *mut u32,
    ) -> c_int {
        -1
    }
    pub unsafe fn mbedtls_ssl_conf_verify(_: *mut c_void, _: VerifyFn, _: *mut c_void) {}
    pub fn bundle() -> &'static [u8] {
        &[]
    }
    pub unsafe fn subject<'a>(_: *mut Crt) -> &'a [u8] {
        &[]
    }
    pub unsafe fn public_key<'a>(_: *mut Crt) -> &'a [u8] {
        &[]
    }
}

/// True if a root in `bundle` has this subject and public key.
pub fn is_bundle_root(bundle: &[u8], subject: &[u8], key: &[u8]) -> bool {
    if bundle.len() < BUNDLE_HEADER_LEN {
        return false;
    }
    let count = u16::from_be_bytes([bundle[0], bundle[1]]) as usize;
    let mut p = BUNDLE_HEADER_LEN;
    for _ in 0..count {
        if p + CRT_HEADER_LEN > bundle.len() {
            return false;
        }
        let name_len = u16::from_be_bytes([bundle[p], bundle[p + 1]]) as usize;
        let key_len = u16::from_be_bytes([bundle[p + 2], bundle[p + 3]]) as usize;
        let name = p + CRT_HEADER_LEN;
        let k = name + name_len;
        if k + key_len > bundle.len() {
            return false;
        }
        if &bundle[name..k] == subject && &bundle[k..k + key_len] == key {
            return true;
        }
        p = k + key_len;
    }
    false
}

unsafe extern "C" fn verify(
    buf: *mut c_void,
    crt: *mut Crt,
    depth: c_int,
    flags: *mut u32,
) -> c_int {
    if *flags & !BADCERT_BAD_MD == BADCERT_NOT_TRUSTED
        && is_bundle_root(sys::bundle(), sys::subject(crt), sys::public_key(crt))
    {
        log!(
            Info,
            TAG,
            "certificate is a cross-signed trusted root, validated"
        );
        *flags = 0;
        return 0;
    }
    sys::esp_crt_verify_callback(buf, crt, depth, flags)
}

/// `crt_bundle_attach()`: the bundle and the verify callback on an
/// `mbedtls_ssl_config`.
///
/// # Safety
/// `conf` an `mbedtls_ssl_config` (or NULL).
pub unsafe extern "C" fn attach(conf: *mut c_void) -> EspErr {
    let e = sys::esp_crt_bundle_attach(conf);
    if e == ESP_OK && !conf.is_null() {
        sys::mbedtls_ssl_conf_verify(conf, verify, core::ptr::null_mut());
    }
    e
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(roots: &[(&[u8], &[u8])]) -> Vec<u8> {
        let mut b = (roots.len() as u16).to_be_bytes().to_vec();
        for (name, key) in roots {
            b.extend_from_slice(&(name.len() as u16).to_be_bytes());
            b.extend_from_slice(&(key.len() as u16).to_be_bytes());
            b.extend_from_slice(name);
            b.extend_from_slice(key);
        }
        b
    }

    #[test]
    fn finds_a_root_by_subject_and_key() {
        let b = bundle(&[(b"CN=A", b"keyA"), (b"CN=B", b"keyB2")]);
        assert!(is_bundle_root(&b, b"CN=B", b"keyB2"));
        assert!(is_bundle_root(&b, b"CN=A", b"keyA"));
        // the subject of one with the key of another
        assert!(!is_bundle_root(&b, b"CN=A", b"keyB2"));
        assert!(!is_bundle_root(&b, b"CN=C", b"keyA"));
        // truncated bundles
        assert!(!is_bundle_root(&b[..b.len() - 1], b"CN=B", b"keyB2"));
        assert!(!is_bundle_root(&[0], b"CN=A", b"keyA"));
    }
}
