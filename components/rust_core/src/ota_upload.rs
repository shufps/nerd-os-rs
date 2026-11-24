//! The upload handlers (port of `main/http_server/handler_ota.cpp`): a
//! firmware image into the next OTA partition (`POST /api/system/OTA`,
//! then the restart into it) and a web UI image into the `www` partition
//! (`POST /api/system/OTAWWW`). The body is received and written by
//! `http/ota.rs`; both write under the hardware lock, the firmware upload
//! with the board shut down (I2C traffic during flash writes upset the
//! PID), and a failed firmware upload restarts the device to mine again.

use crate::idf::idf_const;
use core::ffi::{c_int, c_void, CStr};

use crate::http::data::Partition;
use crate::http::ota::{receive_upload, UploadError, UploadIo};
use crate::idf::httpd::{self, err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::power_mgmt;

const TAG: &CStr = c"http_ota";
/// the receive buffer (internal RAM: flash writes)
const UPLOAD_CHUNK: usize = 2048;
idf_const!(PARTITION_TYPE_DATA: u32 = esp_partition_type_t_ESP_PARTITION_TYPE_DATA, 1);
idf_const!(PARTITION_SUBTYPE_SPIFFS: u32 = esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_SPIFFS, 0x82);
idf_const!(OTA_SIZE_UNKNOWN: usize = OTA_SIZE_UNKNOWN, 0xffff_ffff);
idf_const!(E408_REQ_TIMEOUT: httpd::ErrCode = httpd_err_code_t_HTTPD_408_REQ_TIMEOUT, 8);
idf_const!(CAP_INTERNAL_8BIT: u32 = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT, (1 << 11) | (1 << 2));

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_ota_abort, esp_ota_begin, esp_ota_end, esp_ota_get_next_update_partition,
        esp_ota_set_boot_partition, esp_ota_write, esp_partition_erase_range,
        esp_partition_find_first, esp_partition_write, heap_caps_free, heap_caps_malloc,
        vPortYield,
    };
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_partition_find_first(
        _: u32,
        _: u32,
        _: *const core::ffi::c_char,
    ) -> *const Partition {
        core::ptr::null()
    }
    pub unsafe fn esp_partition_erase_range(_: *const Partition, _: usize, _: usize) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_partition_write(
        _: *const Partition,
        _: usize,
        _: *const c_void,
        _: usize,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_get_next_update_partition(_: *const Partition) -> *const Partition {
        core::ptr::null()
    }
    pub unsafe fn esp_ota_begin(_: *const Partition, _: usize, _: *mut u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_write(_: u32, _: *const c_void, _: usize) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_abort(_: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_end(_: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn esp_ota_set_boot_partition(_: *const Partition) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn heap_caps_malloc(_: usize, _: u32) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn heap_caps_free(_: *mut c_void) {}
    pub unsafe fn vPortYield() {}
}

/// A buffer in internal RAM.
struct Buf(*mut u8);

impl Buf {
    fn new() -> Option<Buf> {
        // SAFETY: heap_caps_malloc has no preconditions; a NULL result yields None
        let p = unsafe { sys::heap_caps_malloc(UPLOAD_CHUNK, CAP_INTERNAL_8BIT) } as *mut u8;
        if p.is_null() {
            return None;
        }
        // SAFETY: a fresh UPLOAD_CHUNK-byte allocation; zeroed so that slice() never
        // exposes uninitialised bytes
        unsafe { core::ptr::write_bytes(p, 0, UPLOAD_CHUNK) };
        Some(Buf(p))
    }
    fn slice(&mut self) -> &mut [u8] {
        // SAFETY: self.0 is the non-null UPLOAD_CHUNK-byte allocation owned by self, and &mut self
        // keeps the slice unique
        unsafe { core::slice::from_raw_parts_mut(self.0, UPLOAD_CHUNK) }
    }
}

impl Drop for Buf {
    fn drop(&mut self) {
        // SAFETY: self.0 came from heap_caps_malloc in Buf::new and is freed only here
        unsafe { sys::heap_caps_free(self.0 as *mut c_void) }
    }
}

/// The hardware lock for a scope.
struct PmLock;

impl PmLock {
    fn new() -> PmLock {
        power_mgmt::lock();
        PmLock
    }
}

impl Drop for PmLock {
    fn drop(&mut self) {
        power_mgmt::unlock();
    }
}

/// The request body into the www partition or an OTA handle.
struct Upload<'a> {
    req: &'a mut Request,
    target: Target,
}

enum Target {
    Www(&'static Partition),
    Ota(u32),
}

impl UploadIo for Upload<'_> {
    fn recv(&mut self, buf: &mut [u8]) -> i32 {
        self.req.recv(buf)
    }
    fn write(&mut self, offset: u32, data: &[u8]) -> bool {
        let ok = match self.target {
            Target::Www(p) => {
                let _g = PmLock::new();
                // SAFETY: a partition entry; `data` for the call
                unsafe {
                    sys::esp_partition_write(
                        p,
                        offset as usize,
                        data.as_ptr() as *const c_void,
                        data.len(),
                    ) == ESP_OK
                }
            }
            // SAFETY: an open OTA handle; `data` for the call
            Target::Ota(h) => unsafe {
                sys::esp_ota_write(h, data.as_ptr() as *const c_void, data.len()) == ESP_OK
            },
        };
        // let the PID and the other tasks run
        // SAFETY: a plain FreeRTOS call from a task
        unsafe { sys::vPortYield() };
        ok
    }
}

fn send_err(req: &mut Request, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

fn upload_error(req: &mut Request, e: UploadError) -> EspErr {
    match e {
        UploadError::TooLarge => send_err(
            req,
            err::E400_BAD_REQUEST,
            c"File provided is too large for device",
        ),
        UploadError::Stalled => send_err(req, E408_REQ_TIMEOUT, c"Upload stalled"),
        UploadError::Write => send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Write Error"),
        UploadError::Receive => send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Protocol Error"),
    }
}

/// The restart after a firmware upload (with or without success).
fn restart_soon() -> ! {
    crate::idf::task::delay_ms(1000);
    power_mgmt::restart()
}

/// `POST /api/system/OTAWWW`
pub fn www_update(req: &mut Request) -> EspErr {
    // SAFETY: a static entry of the partition table, or NULL
    let p: Option<&'static Partition> = unsafe {
        sys::esp_partition_find_first(
            PARTITION_TYPE_DATA,
            PARTITION_SUBTYPE_SPIFFS,
            c"www".as_ptr(),
        )
        .as_ref()
    };
    let Some(p) = p else {
        send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"WWW partition not found",
        );
        return ESP_FAIL;
    };
    let size = p.size as usize;
    // the whole image must fit (content_len is a size_t)
    let content_len = req.content_len();
    if content_len > size {
        send_err(
            req,
            err::E400_BAD_REQUEST,
            c"File provided is too large for device",
        );
        return ESP_FAIL;
    }
    let Some(mut buf) = Buf::new() else {
        return send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Out of memory");
    };
    {
        let _g = PmLock::new();
        log!(Info, TAG, "erasing www partition ...");
        // SAFETY: a partition entry and its size
        if unsafe { sys::esp_partition_erase_range(p, 0, size) } != ESP_OK {
            return send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Erase Error");
        }
        log!(Info, TAG, "erasing done");
    }
    let mut up = Upload {
        req,
        target: Target::Www(p),
    };
    if let Err(e) = receive_upload(&mut up, content_len, size, buf.slice()) {
        upload_error(req, e);
        return ESP_FAIL;
    }
    req.send(b"WWW update complete\n");
    ESP_OK
}

/// `POST /api/system/OTA`
pub fn firmware_update(req: &mut Request) -> EspErr {
    // SAFETY: a static entry of the partition table, or NULL
    let p: Option<&'static Partition> =
        unsafe { sys::esp_ota_get_next_update_partition(core::ptr::null()).as_ref() };
    let Some(p) = p else {
        return send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"OTA partition not found",
        );
    };
    let size = p.size as usize;
    // before the board is shut down
    let content_len = req.content_len();
    if content_len > size {
        return send_err(
            req,
            err::E400_BAD_REQUEST,
            c"File provided is too large for device",
        );
    }
    let Some(mut buf) = Buf::new() else {
        return send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Out of memory");
    };

    // held until the restart
    let _g = PmLock::new();
    // the regulator off: the PID cannot run during the flash writes (I2C),
    // and the device restarts afterwards anyway
    power_mgmt::shutdown();

    let mut handle = 0u32;
    // SAFETY: a partition entry; `handle` writable
    let result = if unsafe { sys::esp_ota_begin(p, OTA_SIZE_UNKNOWN, &mut handle) } == ESP_OK {
        let mut up = Upload {
            req,
            target: Target::Ota(handle),
        };
        let r = receive_upload(&mut up, content_len, size, buf.slice());
        if r.is_err() {
            // SAFETY: the open handle
            unsafe { sys::esp_ota_abort(handle) };
        }
        r
    } else {
        Err(UploadError::Write)
    };
    drop(buf);

    if let Err(e) = result {
        // restart to mine again with the current firmware
        upload_error(req, e);
        log!(
            Error,
            TAG,
            "Firmware upload failed ({}), restarting",
            e as c_int
        );
        restart_soon();
    }
    // validate and activate the new image
    // SAFETY: the open handle; the partition it was written to
    if unsafe { sys::esp_ota_end(handle) != ESP_OK || sys::esp_ota_set_boot_partition(p) != ESP_OK }
    {
        send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"Validation / Activation Error",
        );
        restart_soon();
    }
    req.send(b"Firmware update complete, rebooting now!\n");
    log!(
        Info,
        TAG,
        "Restarting System because of Firmware update complete"
    );
    restart_soon()
}
