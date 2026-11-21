//! Firmware and web UI updates over HTTP (port of the input handling of
//! `handler_ota.cpp` and `handler_ota_factory.cpp`): receiving an upload
//! into flash, the body of `POST /api/system/OTA/github` and the check of the
//! factory image URL. Used by `ota_upload.rs` and `ota_factory_task.rs`.
//!
//! Behaviour notes:
//! * an upload gives up after [`MAX_STALLS`] receive timeouts in a row,
//! * the upload size is a `size_t`, so every size goes through the size
//!   check,
//! * the URL check also looks at the percent-decoded URL, so `%2E%2e` and
//!   similar mixes are rejected like `%2e%2e`.

use crate::ajson;
use crate::idf::idf_const;
use crate::log::log;

const TAG: &core::ffi::CStr = c"http_ota";

idf_const!(
    /// `HTTPD_SOCK_ERR_TIMEOUT`: the receive timed out (5 s), try again.
    pub RECV_TIMEOUT: i32 = HTTPD_SOCK_ERR_TIMEOUT, -3
);

/// Receive timeouts in a row before an upload is given up (about a minute).
pub const MAX_STALLS: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UploadError {
    TooLarge = 1,
    Receive = 2,
    Stalled = 3,
    Write = 4,
}

/// The request body and the flash.
pub trait UploadIo {
    /// Bytes received (up to `buf.len()`), [`RECV_TIMEOUT`], or < 0 / 0 on
    /// an error or a closed connection.
    fn recv(&mut self, buf: &mut [u8]) -> i32;
    /// Writes `data` at `offset`; false on an error.
    fn write(&mut self, offset: u32, data: &[u8]) -> bool;
}

/// Receives `content_len` bytes through `buf` and writes them in order.
pub fn receive_upload(
    io: &mut impl UploadIo,
    content_len: usize,
    max_len: usize,
    buf: &mut [u8],
) -> Result<(), UploadError> {
    if content_len > max_len {
        return Err(UploadError::TooLarge);
    }
    let (mut remaining, mut offset, mut stalls) = (content_len, 0u32, 0u32);
    while remaining > 0 {
        let want = remaining.min(buf.len());
        let n = io.recv(&mut buf[..want]);
        if n == RECV_TIMEOUT {
            stalls += 1;
            if stalls >= MAX_STALLS {
                log!(Error, TAG, "upload stalled at {offset:08x}");
                return Err(UploadError::Stalled);
            }
            continue;
        }
        if n <= 0 || n as usize > want {
            return Err(UploadError::Receive);
        }
        stalls = 0;
        // print each 64 KiB
        if offset & 0xffff == 0 {
            log!(Info, TAG, "flashing to {offset:08x}");
        }
        if !io.write(offset, &buf[..n as usize]) {
            return Err(UploadError::Write);
        }
        remaining -= n as usize;
        offset += n as u32;
    }
    Ok(())
}

/// The release downloads of the Rust firmware's repository: switching between
/// the Rust and the C firmware only goes through the web flasher.
const GITHUB_REPO: &[u8] = b"https://github.com/shufps/nerd-os-rs/releases/download/";

/// Percent-decoding (either case); other bytes as they are.
fn percent_decode(s: &[u8]) -> Vec<u8> {
    let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i] == b'%' && i + 2 < s.len() {
            if let (Some(h), Some(l)) = (hex(s[i + 1]), hex(s[i + 2])) {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(s[i]);
        i += 1;
    }
    out
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

/// The factory image URL must be under [`GITHUB_REPO`] (prefix compared
/// without case, as before), without `..` path steps (also
/// percent-encoded, in any case), `%2e%2e` anywhere, backslashes or control
/// characters.
pub fn factory_url_ok(url: &[u8]) -> bool {
    if url.len() < GITHUB_REPO.len() || !url[..GITHUB_REPO.len()].eq_ignore_ascii_case(GITHUB_REPO)
    {
        return false;
    }
    if url.iter().any(|&c| c < 0x20 || c == 0x7f || c == b'\\') {
        return false;
    }
    // as before: an encoded ".." anywhere, now in any case
    if contains(&url.to_ascii_lowercase(), b"%2e%2e") {
        return false;
    }
    let decoded = percent_decode(url);
    for s in [url, &decoded] {
        if contains(s, b"../")
            || contains(s, b"/..")
            || s.iter().any(|&c| c == b'\\' || c < 0x20 || c == 0x7f)
        {
            return false;
        }
    }
    true
}

#[derive(Debug, PartialEq, Eq)]
pub enum FactoryBody {
    Invalid,
    NoUrl,
    Ok { url: Vec<u8>, keep_config: bool },
}

/// `{"url": "...", "keep_config": bool}`: `url` must be a string;
/// `keep_config` is true unless it is the bool false (`doc[...] | true`).
pub fn factory_body(body: &[u8]) -> FactoryBody {
    let Ok(doc) = ajson::deserialize(body) else {
        return FactoryBody::Invalid;
    };
    let Some(url) = doc.get("url").as_cstr() else {
        return FactoryBody::NoUrl;
    };
    let keep_config = doc.get("keep_config").as_bool().unwrap_or(true);
    FactoryBody::Ok {
        url: url.to_vec(),
        keep_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        script: VecDeque<i32>,
        data: Vec<u8>,
        writes: Vec<(u32, usize)>,
        fail_write: bool,
    }

    impl UploadIo for Fake {
        fn recv(&mut self, buf: &mut [u8]) -> i32 {
            match self.script.pop_front() {
                Some(n) if n > 0 => {
                    let n = (n as usize).min(buf.len());
                    for (i, b) in buf[..n].iter_mut().enumerate() {
                        *b = (self.data.len() + i) as u8;
                    }
                    self.data.extend_from_slice(&buf[..n]);
                    n as i32
                }
                Some(n) => n,
                None => 0,
            }
        }
        fn write(&mut self, offset: u32, data: &[u8]) -> bool {
            self.writes.push((offset, data.len()));
            !self.fail_write
        }
    }

    fn run(script: &[i32], len: usize, max: usize) -> (Result<(), UploadError>, Fake) {
        let mut f = Fake {
            script: script.iter().copied().collect(),
            ..Default::default()
        };
        let mut buf = [0u8; 2048];
        (receive_upload(&mut f, len, max, &mut buf), f)
    }

    #[test]
    fn uploads_in_order_through_timeouts() {
        let (r, f) = run(
            &[2048, RECV_TIMEOUT, 100, RECV_TIMEOUT, 2048],
            4196,
            1 << 20,
        );
        assert_eq!(r, Ok(()));
        assert_eq!(f.writes, [(0, 2048), (2048, 100), (2148, 2048)]);
    }

    #[test]
    fn errors() {
        assert_eq!(run(&[], 10, 5).0, Err(UploadError::TooLarge));
        assert_eq!(run(&[100, 0], 200, 1000).0, Err(UploadError::Receive));
        assert_eq!(run(&[100, -1], 200, 1000).0, Err(UploadError::Receive));
        let stalls = vec![RECV_TIMEOUT; MAX_STALLS as usize];
        assert_eq!(run(&stalls, 200, 1000).0, Err(UploadError::Stalled));
        // a received byte resets the count
        let mut s = vec![RECV_TIMEOUT; MAX_STALLS as usize - 1];
        s.push(100);
        s.extend(vec![RECV_TIMEOUT; MAX_STALLS as usize - 1]);
        s.push(100);
        assert_eq!(run(&s, 200, 1000).0, Ok(()));
        let mut f = Fake {
            script: [100].into(),
            fail_write: true,
            ..Default::default()
        };
        assert_eq!(
            receive_upload(&mut f, 100, 100, &mut [0; 64]),
            Err(UploadError::Write)
        );
        // sizes beyond i32 are not negative
        assert_eq!(run(&[], 3 << 30, 4 << 20).0, Err(UploadError::TooLarge));
    }

    #[test]
    fn factory_urls() {
        const R: &str = "https://github.com/shufps/nerd-os-rs/releases/download";
        let ok = format!("{R}/v1.0/factory.bin");
        assert!(factory_url_ok(ok.as_bytes()));
        assert!(factory_url_ok(&ok.to_ascii_uppercase().into_bytes()));
        for bad in [
            // the C firmware's releases: only via the web flasher
            "https://github.com/shufps/ESP-Miner-NerdQAxePlus/releases/download/v1.0/factory.bin"
                .to_string(),
            "https://github.com/shufps/nerd-os-rsx/releases/download/v1/f.bin".to_string(),
            "https://github.com/shufps/x".to_string(),
            "https://github.com/shufpsx/y".to_string(),
            format!("{R}/../../../ESP-Miner-NerdQAxePlus/releases/download/v1/f.bin"),
            format!("{R}/%2e%2e/other"),
            format!("{R}/%2E%2e/other"),
            format!("{R}/.%2E/other"),
            format!("{R}/x%5c.."),
            format!("{R}/a\\b"),
            format!("{R}/a\nb"),
            format!("http{}", &R[5..]),
        ] {
            let bad = bad.as_bytes();
            assert!(!factory_url_ok(bad), "{}", String::from_utf8_lossy(bad));
        }
    }

    #[test]
    fn factory_bodies() {
        assert_eq!(factory_body(b"{"), FactoryBody::Invalid);
        assert_eq!(factory_body(b"{\"url\":1}"), FactoryBody::NoUrl);
        assert_eq!(
            factory_body(b"{\"url\":\"u\",\"keep_config\":\"no\"}"),
            FactoryBody::Ok {
                url: b"u".to_vec(),
                keep_config: true
            }
        );
        assert_eq!(
            factory_body(b"{\"url\":\"u\",\"keep_config\":false}"),
            FactoryBody::Ok {
                url: b"u".to_vec(),
                keep_config: false
            }
        );
    }
}
