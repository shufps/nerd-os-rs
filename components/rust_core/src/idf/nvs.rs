//! NVS access (`nvs.h`): open/close a namespace, blobs, strings and
//! integers.

use core::ffi::{c_char, c_void, CStr};

use super::{idf_const, EspErr, ESP_OK};

/// `nvs_handle_t`
#[cfg(target_os = "espidf")]
pub type Handle = super::raw::nvs_handle_t;
#[cfg(not(target_os = "espidf"))]
pub type Handle = u32;
idf_const!(pub READONLY: u32 = nvs_open_mode_t_NVS_READONLY, 0);
idf_const!(pub READWRITE: u32 = nvs_open_mode_t_NVS_READWRITE, 1);

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_err_to_name, nvs_close, nvs_commit, nvs_get_blob, nvs_get_str, nvs_get_u16,
        nvs_get_u64, nvs_open, nvs_set_blob,
    };
}

/// On the host there is no NVS.
#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    use crate::idf::ESP_FAIL;
    pub unsafe fn nvs_open(_: *const c_char, _: u32, _: *mut Handle) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_close(_: Handle) {}
    pub unsafe fn nvs_commit(_: Handle) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_get_blob(
        _: Handle,
        _: *const c_char,
        _: *mut c_void,
        _: *mut usize,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_set_blob(_: Handle, _: *const c_char, _: *const c_void, _: usize) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_get_str(
        _: Handle,
        _: *const c_char,
        _: *mut c_char,
        _: *mut usize,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_get_u16(_: Handle, _: *const c_char, _: *mut u16) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn nvs_get_u64(_: Handle, _: *const c_char, _: *mut u64) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_err_to_name(_: EspErr) -> *const c_char {
        c"ESP_FAIL".as_ptr()
    }
}

/// The name of an error code.
pub fn err_name(err: EspErr) -> String {
    // SAFETY: `esp_err_to_name` always returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(sys::esp_err_to_name(err)) }
        .to_string_lossy()
        .into_owned()
}

/// An open namespace, closed on drop.
pub struct Nvs(Handle);

impl Nvs {
    pub fn open(namespace: &CStr, mode: u32) -> Result<Nvs, EspErr> {
        let mut h = 0;
        // SAFETY: `namespace` is NUL-terminated and `h` a valid local for the out value.
        match unsafe { sys::nvs_open(namespace.as_ptr(), mode, &mut h) } {
            ESP_OK => Ok(Nvs(h)),
            e => Err(e),
        }
    }

    pub fn commit(&self) -> EspErr {
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`.
        unsafe { sys::nvs_commit(self.0) }
    }

    pub fn get_blob(&self, key: &CStr) -> Result<Vec<u8>, EspErr> {
        let mut len = 0usize;
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`; `key` is NUL-terminated, a null buffer only queries the length.
        let e = unsafe { sys::nvs_get_blob(self.0, key.as_ptr(), core::ptr::null_mut(), &mut len) };
        if e != ESP_OK {
            return Err(e);
        }
        let mut buf = vec![0u8; len];
        // SAFETY: as above; `buf` is writable for `len` bytes, the length passed in.
        match unsafe {
            sys::nvs_get_blob(
                self.0,
                key.as_ptr(),
                buf.as_mut_ptr() as *mut c_void,
                &mut len,
            )
        } {
            ESP_OK => {
                buf.truncate(len);
                Ok(buf)
            }
            e => Err(e),
        }
    }

    pub fn set_blob(&self, key: &CStr, data: &[u8]) -> EspErr {
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`; `key` is NUL-terminated and `data` readable for `data.len()` bytes.
        unsafe {
            sys::nvs_set_blob(
                self.0,
                key.as_ptr(),
                data.as_ptr() as *const c_void,
                data.len(),
            )
        }
    }

    /// A string (without the terminator); None if missing or empty.
    pub fn get_str(&self, key: &CStr) -> Option<Vec<u8>> {
        let mut len = 0usize;
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`; `key` is NUL-terminated, a null buffer only queries the length.
        if unsafe { sys::nvs_get_str(self.0, key.as_ptr(), core::ptr::null_mut(), &mut len) }
            != ESP_OK
            || len == 0
        {
            return None;
        }
        let mut buf = vec![0u8; len];
        // SAFETY: as above; `buf` is writable for `len` bytes, the length passed in.
        if unsafe {
            sys::nvs_get_str(
                self.0,
                key.as_ptr(),
                buf.as_mut_ptr() as *mut c_char,
                &mut len,
            )
        } != ESP_OK
        {
            return None;
        }
        let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        buf.truncate(end);
        Some(buf)
    }

    pub fn get_u16(&self, key: &CStr) -> Option<u16> {
        let mut v = 0;
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`; `key` is NUL-terminated and `v` a valid local.
        (unsafe { sys::nvs_get_u16(self.0, key.as_ptr(), &mut v) } == ESP_OK).then_some(v)
    }

    pub fn get_u64(&self, key: &CStr) -> Option<u64> {
        let mut v = 0;
        // SAFETY: the handle comes from a successful `nvs_open` and is only closed in `drop`; `key` is NUL-terminated and `v` a valid local.
        (unsafe { sys::nvs_get_u64(self.0, key.as_ptr(), &mut v) } == ESP_OK).then_some(v)
    }
}

impl Drop for Nvs {
    fn drop(&mut self) {
        // SAFETY: the handle is open and owned by this `Nvs`; it is not used after drop.
        unsafe { sys::nvs_close(self.0) }
    }
}
