//! Which file a web UI request is served from (port of the path handling of
//! `rest_common_get_handler()` in `handler_file.cpp`): the gzipped file in
//! SPIFFS, its content type, the cache policy, and whether a missing file
//! is an asset (404) or a page (portal redirect).
//!
//! The extension checks (`CHECK_FILE_EXTENSION`) compare only when the path
//! is at least as long as the extension, so short paths such as `/a` match
//! none.

use crate::idf::idf_const;
use core::ffi::CStr;

idf_const!(
    /// `FILE_PATH_MAX`: `ESP_VFS_PATH_MAX` (15) + 128
    pub PATH_MAX: usize = ESP_VFS_PATH_MAX + 128, 15 + 128
);

#[derive(Debug, PartialEq, Eq)]
pub struct File {
    /// the file to open (with `.gz`)
    pub path: Vec<u8>,
    pub content_type: &'static CStr,
    pub cache_control: &'static CStr,
    /// a missing file is an asset (404), not a page (portal redirect)
    pub asset: bool,
}

/// `strcasecmp()` of the path's end with `ext`.
fn has_ext(path: &[u8], ext: &[u8]) -> bool {
    path.len() >= ext.len() && path[path.len() - ext.len()..].eq_ignore_ascii_case(ext)
}

fn content_type(path: &[u8]) -> &'static CStr {
    for (ext, t) in [
        (&b".html"[..], c"text/html"),
        (b".js", c"application/javascript"),
        (b".css", c"text/css"),
        (b".png", c"image/png"),
        (b".ico", c"image/x-icon"),
        (b".svg", c"image/svg+xml"),
        (b".woff2", c"font/woff2"),
        (b".json", c"application/json"),
    ] {
        if has_ext(path, ext) {
            return t;
        }
    }
    c"text/plain"
}

fn cache_control(path: &[u8]) -> &'static CStr {
    if has_ext(path, b".woff2") || has_ext(path, b".png") {
        // can be cached "forever"
        c"public, max-age=31536000, immutable"
    } else if has_ext(path, b".html") {
        // never cache the index.html
        c"no-cache"
    } else {
        // ~30 days
        c"max-age=2592000"
    }
}

/// An asset by the extension after the last `.` (case-sensitive, as the
/// C++ `strcmp()`); no extension is a page.
fn is_asset(uri: &[u8]) -> bool {
    let Some(dot) = uri.iter().rposition(|&c| c == b'.') else {
        return false;
    };
    matches!(
        &uri[dot..],
        b".js" | b".css" | b".png" | b".jpg" | b".jpeg" | b".svg" | b".ico" | b".woff2" | b".json"
    )
}

/// The file for `uri` under `base`; None if the path does not fit
/// `PATH_MAX` (the C++ "File path too long").
pub fn resolve(base: &[u8], uri: &[u8]) -> Option<File> {
    // the URI as the C++ buffer held it, without the query string
    let uri = &uri[..uri.len().min(PATH_MAX - 1)];
    let uri = &uri[..uri.iter().position(|&c| c == b'?').unwrap_or(uri.len())];
    let base = &base[..base.len().min(PATH_MAX - 1)];

    let mut path = base.to_vec();
    if uri.last() == Some(&b'/') {
        // any "/dir/" serves the root index.html
        path.extend_from_slice(b"/index.html");
    } else {
        path.extend_from_slice(uri);
    }
    // the precompressed file, with its NUL
    if path.len() + 3 + 1 > PATH_MAX {
        return None;
    }
    let content_type = content_type(&path);
    let cache_control = cache_control(&path);
    path.extend_from_slice(b".gz");
    Some(File {
        path,
        content_type,
        cache_control,
        asset: is_asset(uri),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(uri: &str) -> File {
        resolve(b"", uri.as_bytes()).unwrap()
    }

    #[test]
    fn paths_and_types() {
        let f = r("/");
        assert_eq!(f.path, b"/index.html.gz");
        assert_eq!(
            (f.content_type, f.cache_control, f.asset),
            (c"text/html", c"no-cache", false)
        );
        // any directory: the root index
        assert_eq!(r("/settings/").path, b"/index.html.gz");
        let f = r("/main.abc123.js?v=1");
        assert_eq!(f.path, b"/main.abc123.js.gz");
        assert_eq!(
            (f.content_type, f.cache_control, f.asset),
            (c"application/javascript", c"max-age=2592000", true)
        );
        let f = r("/assets/font.WOFF2");
        assert_eq!(
            (f.content_type, f.cache_control),
            (c"font/woff2", c"public, max-age=31536000, immutable")
        );
        // case-sensitive asset check, as before
        assert!(!r("/x.JS").asset);
        assert_eq!(r("/x.JS").content_type, c"application/javascript");
        assert_eq!(r("/dashboard").content_type, c"text/plain");
        assert!(!r("/dashboard").asset);
        assert!(r("/img.jpg").asset);
        assert_eq!(r("/img.jpg").content_type, c"text/plain");
    }

    #[test]
    fn short_paths_and_limits() {
        // shorter than the extension
        assert_eq!(r("/a").content_type, c"text/plain");
        assert_eq!(r("").path, b".gz");
        // 142 bytes is the most a path with ".gz" and NUL can have
        let long = format!("/{}", "a".repeat(138));
        assert_eq!(r(&long).path.len(), 142);
        assert!(resolve(b"", format!("/{}", "a".repeat(139)).as_bytes()).is_none());
        // the query string does not count
        assert!(resolve(b"", format!("/x.js?{}", "q".repeat(300)).as_bytes()).is_some());
    }
}
