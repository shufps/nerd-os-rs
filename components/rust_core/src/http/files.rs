//! The web UI files (port of `main/http_server/handler_file.cpp`): mounts
//! the SPIFFS partition (`init_fs()`; a failure means recovery mode) and
//! serves its gzipped files with the type and cache policy of
//! `http/file.rs`; a missing page redirects to the portal, a missing asset
//! is a 404.

use crate::idf::idf_const;
use core::ffi::CStr;
use std::ffi::CString;
use std::io::Read;

use crate::idf::httpd::{err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"http_file";
idf_const!(
    /// `FILE_PATH_MAX`: `ESP_VFS_PATH_MAX + 128`
    FILE_PATH_MAX: usize = ESP_VFS_PATH_MAX + 128, 15 + 128
);
idf_const!(ESP_ERR_NOT_FOUND: EspErr = ESP_ERR_NOT_FOUND, 0x105);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_vfs_spiffs_conf_t as SpiffsConf;
#[cfg(not(target_os = "espidf"))]
/// `esp_vfs_spiffs_conf_t` (host model)
#[repr(C)]
pub struct SpiffsConf {
    pub base_path: *const core::ffi::c_char,
    pub partition_label: *const core::ffi::c_char,
    pub max_files: usize,
    pub format_if_mount_failed: bool,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{esp_spiffs_info, esp_vfs_spiffs_register};
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn esp_vfs_spiffs_register(_: *const SpiffsConf) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_spiffs_info(
        _: *const core::ffi::c_char,
        _: *mut usize,
        _: *mut usize,
    ) -> EspErr {
        ESP_FAIL
    }
}

/// Mounts the web UI partition at "".
pub fn init_fs() -> EspErr {
    let conf = SpiffsConf {
        base_path: c"".as_ptr(),
        partition_label: core::ptr::null(),
        max_files: 5,
        format_if_mount_failed: false,
    };
    // SAFETY: `conf` is a live config with a static base path; it is only read
    // (the strings are copied)
    let ret = unsafe { sys::esp_vfs_spiffs_register(&conf) };
    if ret != ESP_OK {
        match ret {
            ESP_FAIL => log!(Error, TAG, "Failed to mount or format filesystem"),
            ESP_ERR_NOT_FOUND => log!(Error, TAG, "Failed to find SPIFFS partition"),
            e => log!(
                Error,
                TAG,
                "Failed to initialize SPIFFS ({})",
                crate::idf::nvs::err_name(e)
            ),
        }
        return ESP_FAIL;
    }
    let (mut total, mut used) = (0usize, 0usize);
    // SAFETY: NULL label means the default partition; both outputs are writable usizes
    let ret = unsafe { sys::esp_spiffs_info(core::ptr::null(), &mut total, &mut used) };
    if ret != ESP_OK {
        log!(
            Error,
            TAG,
            "Failed to get SPIFFS partition information ({})",
            crate::idf::nvs::err_name(ret)
        );
    } else {
        log!(Info, TAG, "Partition size: total: {total}, used: {used}");
    }
    ESP_OK
}

fn redirect_portal(req: &mut Request) -> EspErr {
    log!(Info, TAG, "Redirecting to portal");
    req.set_status(c"302 Found");
    req.set_type(c"text/plain");
    req.set_hdr(c"Location", c"/");
    req.send(b"Redirect to the captive portal");
    ESP_OK
}

fn open(path: &[u8]) -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;
    std::fs::File::open(std::ffi::OsStr::from_bytes(path))
}

/// `rest_common_get_handler()`: the file of the request.
pub fn file_handler(req: &mut Request) -> EspErr {
    // the base path of the server context is "" (it stays zeroed)
    let uri: CString = req.uri().into();
    let Some(f) =
        crate::http::file::resolve(b"", uri.to_bytes()).filter(|f| f.path.len() < FILE_PATH_MAX)
    else {
        return req.send_err(err::E500_INTERNAL_SERVER_ERROR, Some(c"File path too long"));
    };
    let path = String::from_utf8_lossy(&f.path).into_owned();

    // do not MIME-sniff
    req.set_hdr(c"X-Content-Type-Options", c"nosniff");

    let mut file = match open(&f.path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if f.asset {
                // a missing asset: 404, no portal redirect
                let errno = e.raw_os_error().unwrap_or(0);
                log!(Error, TAG, "Failed to open file: {path}, errno: {errno}");
                req.send_err(err::E404_NOT_FOUND, Some(c"Not found"));
                return ESP_OK;
            }
            log!(Error, TAG, "page not found: {}", uri.to_string_lossy());
            return redirect_portal(req);
        }
        Err(_) => {
            return req.send_err(
                err::E500_INTERNAL_SERVER_ERROR,
                Some(c"Failed to open file"),
            )
        }
    };

    log!(Info, TAG, "Sending {path}");

    // the file's headers (type, cache policy, gzip) only go with the file,
    // not with an error response
    let start = |req: &mut Request| {
        req.set_type(f.content_type);
        req.set_hdr(c"Cache-Control", f.cache_control);
        req.set_hdr(c"Content-Encoding", c"gzip");
    };
    crate::http::server::with_scratch(req, |req, chunk| {
        send_file(req, &mut file, chunk, &path, start)
    })
    .unwrap_or(ESP_FAIL)
}

/// Sends the file in chunks through `chunk`; `start` sets the file's
/// headers before the first chunk.
fn send_file(
    req: &mut Request,
    file: &mut std::fs::File,
    chunk: &mut [u8],
    path: &str,
    start: impl Fn(&mut Request),
) -> EspErr {
    let mut started = false;
    loop {
        let n = match file.read(chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => {
                log!(Error, TAG, "Read error while sending {path}");
                if !started {
                    return req.send_err(err::E500_INTERNAL_SERVER_ERROR, Some(c"Read error"));
                }
                // headers and part of the body are out: no final chunk, so
                // the client sees a truncated transfer
                return ESP_FAIL;
            }
        };
        if !started {
            start(req);
            started = true;
        }
        if req.send_chunk(&chunk[..n]) != ESP_OK {
            log!(Error, TAG, "Failed to send file chunk");
            return ESP_FAIL;
        }
    }
    if !started {
        // an empty file
        start(req);
    }
    log!(Info, TAG, "File sending complete");
    req.end_chunks();
    ESP_OK
}
