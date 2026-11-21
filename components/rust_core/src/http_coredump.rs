//! The core dump download `GET /api/system/coredump` (port of
//! `main/http_server/handler_system.cpp`; only with core dumps in flash,
//! `sdkconfig.coredump`): the stored dump from the coredump partition,
//! with the crashed application's ELF SHA-256 (from the dump's
//! `ESP_CORE_DUMP_INFO` note) in `X-ESP-App-ELF-SHA256`.

use crate::idf::idf_const;
use core::ffi::{c_void, CStr};

use crate::http_data::Partition;
use crate::idf::httpd::{self, err, Request};
use crate::idf::nvs::err_name;
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"http_system";

idf_const!(ESP_ERR_INVALID_SIZE: EspErr = ESP_ERR_INVALID_SIZE, 0x104);
idf_const!(ESP_ERR_NOT_FOUND: EspErr = ESP_ERR_NOT_FOUND, 0x105);
idf_const!(ESP_ERR_INVALID_RESPONSE: EspErr = ESP_ERR_INVALID_RESPONSE, 0x108);
idf_const!(PARTITION_TYPE_DATA: u32 = esp_partition_type_t_ESP_PARTITION_TYPE_DATA, 1);
idf_const!(PARTITION_SUBTYPE_COREDUMP: u32 = esp_partition_subtype_t_ESP_PARTITION_SUBTYPE_DATA_COREDUMP, 3);

#[cfg(all(target_os = "espidf", coredump_flash))]
mod sys {
    use super::*;
    extern "C" {
        pub fn esp_core_dump_image_check() -> EspErr;
        pub fn esp_core_dump_image_get(addr: *mut usize, size: *mut usize) -> EspErr;
    }
}

/// Without core dumps in flash (the route is not registered then).
#[cfg(not(all(target_os = "espidf", coredump_flash)))]
mod sys {
    use super::*;
    pub unsafe fn esp_core_dump_image_check() -> EspErr {
        ESP_ERR_NOT_FOUND
    }
    pub unsafe fn esp_core_dump_image_get(_: *mut usize, _: *mut usize) -> EspErr {
        ESP_ERR_NOT_FOUND
    }
}

#[cfg(target_os = "espidf")]
mod part {
    pub use crate::idf::raw::{esp_partition_find_first, esp_partition_read};
}

#[cfg(not(target_os = "espidf"))]
mod part {
    use super::*;
    pub unsafe fn esp_partition_find_first(
        _: u32,
        _: u32,
        _: *const core::ffi::c_char,
    ) -> *const Partition {
        core::ptr::null()
    }
    pub unsafe fn esp_partition_read(
        _: *const Partition,
        _: usize,
        _: *mut c_void,
        _: usize,
    ) -> EspErr {
        ESP_FAIL
    }
}

/// The ELF SHA-256 (hex, at most `max` digits) from the dump's
/// `ESP_CORE_DUMP_INFO` note: `read(offset, buf)` reads from the dump of
/// `dump_size` bytes, `scratch` is the search window.
pub fn elf_sha256(
    mut read: impl FnMut(usize, &mut [u8]) -> Result<(), EspErr>,
    dump_size: usize,
    scratch: &mut [u8],
    max: usize,
) -> Result<String, EspErr> {
    const NOTE_NAME: &[u8] = b"ESP_CORE_DUMP_INFO";
    const NOTE_TYPE: u32 = 8266;
    /// name size, description size, type
    const HEADER: usize = 12;

    // the first occurrence of the name, searched in overlapping windows
    let m = NOTE_NAME.len();
    let mut read_offset = 0;
    let mut marker = None;
    while read_offset < dump_size {
        let n = (dump_size - read_offset).min(scratch.len());
        read(read_offset, &mut scratch[..n])?;
        if let Some(i) = scratch[..n].windows(m).position(|w| w == NOTE_NAME) {
            marker = Some(read_offset + i);
            break;
        }
        if n < m {
            break;
        }
        read_offset += n - (m - 1);
    }
    let marker = marker.filter(|&o| o >= HEADER).ok_or(ESP_ERR_NOT_FOUND)?;

    let note_offset = marker - HEADER;
    let mut h = [0u8; HEADER];
    read(note_offset, &mut h)?;
    let word = |i: usize| u32::from_le_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]) as usize;
    let (name_size, desc_size, ty) = (word(0), word(4), word(8) as u32);
    if ty != NOTE_TYPE || name_size < m + 1 || name_size > 32 || desc_size < 4 + 9 {
        return Err(ESP_ERR_INVALID_RESPONSE);
    }
    let desc_offset = note_offset + HEADER + ((name_size + 3) & !3);
    if desc_offset > dump_size || desc_size > dump_size - desc_offset {
        return Err(ESP_ERR_INVALID_SIZE);
    }
    // after the version word, NUL-terminated
    let mut sha = vec![0u8; (desc_size - 4).min(max)];
    read(desc_offset + 4, &mut sha)?;
    let len = sha.iter().position(|&c| c == 0).unwrap_or(sha.len());
    sha.truncate(len);
    if !sha.iter().all(u8::is_ascii_hexdigit) || len < 8 {
        return Err(ESP_ERR_INVALID_RESPONSE);
    }
    Ok(String::from_utf8_lossy(&sha).into_owned())
}

fn send_err(req: &mut Request, code: httpd::ErrCode, msg: &CStr) -> EspErr {
    req.send_err(code, Some(msg))
}

/// `GET /api/system/coredump`
pub fn handler(req: &mut Request) -> EspErr {
    // SAFETY: reads the core dump partition
    let e = unsafe { sys::esp_core_dump_image_check() };
    if e == ESP_ERR_NOT_FOUND || e == ESP_ERR_INVALID_SIZE {
        return send_err(req, err::E404_NOT_FOUND, c"No valid core dump stored");
    }
    if e != ESP_OK {
        log!(
            Error,
            TAG,
            "Core dump integrity check failed: {}",
            err_name(e)
        );
        req.set_status(c"409 Conflict");
        return req.send(b"Core dump integrity check failed");
    }

    let (mut address, mut size) = (0usize, 0usize);
    // SAFETY: both outputs writable
    let e = unsafe { sys::esp_core_dump_image_get(&mut address, &mut size) };
    if e != ESP_OK {
        log!(Error, TAG, "Failed to locate core dump: {}", err_name(e));
        return send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"Failed to locate core dump",
        );
    }
    // SAFETY: the partition table lives as long as the firmware (static
    // entries, or NULL)
    let p: Option<&'static Partition> = unsafe {
        part::esp_partition_find_first(
            PARTITION_TYPE_DATA,
            PARTITION_SUBTYPE_COREDUMP,
            core::ptr::null(),
        )
        .as_ref()
    };
    let Some(p) = p.filter(|p| address == p.address as usize && size <= p.size as usize) else {
        log!(
            Error,
            TAG,
            "Core dump location is outside the coredump partition"
        );
        return send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"Invalid core dump location",
        );
    };

    crate::http_server::with_scratch(req, |req, chunk| send_dump(req, chunk, p, size))
        .unwrap_or(ESP_FAIL)
}

/// The ELF identity header and the dump in chunks through `chunk`.
fn send_dump(req: &mut Request, chunk: &mut [u8], p: &'static Partition, size: usize) -> EspErr {
    let read = |offset: usize, buf: &mut [u8]| {
        // SAFETY: a partition entry; at most `buf.len()` bytes into `buf`
        let e = unsafe {
            part::esp_partition_read(p, offset, buf.as_mut_ptr() as *mut c_void, buf.len())
        };
        if e == ESP_OK {
            Ok(())
        } else {
            Err(e)
        }
    };
    let sha = match elf_sha256(read, size, chunk, 64) {
        Ok(s) => s,
        Err(e) => {
            log!(
                Error,
                TAG,
                "Failed to read core dump ELF identity: {}",
                err_name(e)
            );
            return send_err(
                req,
                err::E500_INTERNAL_SERVER_ERROR,
                c"Failed to identify core dump ELF",
            );
        }
    };

    req.set_type(c"application/octet-stream");
    req.set_hdr(c"Cache-Control", c"no-store");
    req.set_hdr(c"X-Content-Type-Options", c"nosniff");
    let sha = std::ffi::CString::new(sha).unwrap_or_default();
    if req.set_hdr(c"X-ESP-App-ELF-SHA256", &sha) != ESP_OK {
        log!(Error, TAG, "Failed to set core dump ELF identity header");
        return send_err(
            req,
            err::E500_INTERNAL_SERVER_ERROR,
            c"Failed to identify core dump ELF",
        );
    }

    let mut offset = 0;
    while offset < size {
        let n = (size - offset).min(chunk.len());
        if read(offset, &mut chunk[..n]).is_err() {
            log!(Error, TAG, "Failed to read core dump at offset {offset}");
            req.end_chunks();
            return ESP_FAIL;
        }
        if req.send_chunk(&chunk[..n]) != ESP_OK {
            log!(Error, TAG, "Failed to send core dump at offset {offset}");
            req.end_chunks();
            return ESP_FAIL;
        }
        offset += n;
    }
    log!(Info, TAG, "Sent {size}-byte core dump");
    req.end_chunks()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dump with the note at `at`.
    fn dump(at: usize, sha: &[u8]) -> Vec<u8> {
        let mut d = vec![0xAAu8; at];
        let name = b"ESP_CORE_DUMP_INFO\0";
        let desc_len = 4 + sha.len() + 1;
        d.extend_from_slice(&(name.len() as u32).to_le_bytes());
        d.extend_from_slice(&(desc_len as u32).to_le_bytes());
        d.extend_from_slice(&8266u32.to_le_bytes());
        // the name padded to 4 bytes (relative to the note)
        d.extend_from_slice(name);
        d.push(0);
        d.extend_from_slice(&1u32.to_le_bytes());
        d.extend_from_slice(sha);
        d.push(0);
        d.extend_from_slice(&[0x55; 100]);
        d
    }

    fn sha_of(d: &[u8], window: usize) -> Result<String, EspErr> {
        let mut scratch = vec![0u8; window];
        let read = |o: usize, b: &mut [u8]| {
            b.copy_from_slice(&d[o..o + b.len()]);
            Ok(())
        };
        elf_sha256(read, d.len(), &mut scratch, 64)
    }

    #[test]
    fn finds_the_note_across_windows() {
        let sha = b"c3af30756754f8e5bd24955943d81b4f01d4d73d5ddd3bd18a6504666c31447d";
        for at in [12, 100, 1000, 1017, 1030] {
            let d = dump(at, sha);
            for window in [64, 256, 16384] {
                assert_eq!(
                    sha_of(&d, window).unwrap().as_bytes(),
                    sha,
                    "at {at} window {window}"
                );
            }
        }
    }

    #[test]
    fn rejects_bad_notes() {
        assert_eq!(sha_of(&[0u8; 500], 64), Err(ESP_ERR_NOT_FOUND));
        // no room for the header before the name
        let d = dump(0, b"0123456789abcdef");
        assert_eq!(sha_of(&d[12..], 64), Err(ESP_ERR_NOT_FOUND));
        // not hex
        assert_eq!(
            sha_of(&dump(40, b"0123456789abcdeg"), 64),
            Err(ESP_ERR_INVALID_RESPONSE)
        );
        // too short
        assert_eq!(
            sha_of(&dump(40, b"0123456"), 64),
            Err(ESP_ERR_INVALID_RESPONSE)
        );
        // a different type
        let mut d = dump(40, b"0123456789abcdef");
        d[40 + 8] = 1;
        assert_eq!(sha_of(&d, 64), Err(ESP_ERR_INVALID_RESPONSE));
    }
}
