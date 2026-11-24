//! Requests to the pool. The layout matches the former C++ `snprintf()`
//! formats byte for byte; strings are additionally JSON-escaped.
//!
//! Requests are bytes, not `String`s: config strings are passed on unchanged
//! even if they are not valid UTF-8, as before.

use std::io::Write;

/// `"s"` with `"`, `\` and control characters escaped; other bytes unchanged.
fn quoted(out: &mut Vec<u8>, s: &[u8]) {
    out.push(b'"');
    for &b in crate::util::cutil::until_nul(s) {
        match b {
            b'"' => out.extend(b"\\\""),
            b'\\' => out.extend(b"\\\\"),
            0x00..=0x1f => {
                let _ = write!(out, "\\u{b:04x}");
            }
            _ => out.push(b),
        }
    }
    out.push(b'"');
}

fn request(id: i32, method: &str, params: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let mut out = Vec::with_capacity(128);
    let _ = write!(out, r#"{{"id": {id}, "method": "{method}", "params": ["#);
    params(&mut out);
    out.extend(b"]}\n");
    out
}

pub fn subscribe(id: i32, device: &[u8], asic: &[u8], version: &[u8]) -> Vec<u8> {
    let agent = [device, asic, version].join(&b'/');
    request(id, "mining.subscribe", |out| quoted(out, &agent))
}

pub fn extranonce_subscribe(id: i32) -> Vec<u8> {
    request(id, "mining.extranonce.subscribe", |_| {})
}

pub fn suggest_difficulty(id: i32, difficulty: u32) -> Vec<u8> {
    request(id, "mining.suggest_difficulty", |out| {
        let _ = write!(out, "{difficulty}");
    })
}

pub fn authorize(id: i32, user: &[u8], password: &[u8]) -> Vec<u8> {
    request(id, "mining.authorize", |out| {
        quoted(out, user);
        out.extend(b", ");
        quoted(out, password);
    })
}

pub fn submit(
    id: i32,
    user: &[u8],
    job_id: &[u8],
    extranonce_2: &[u8],
    ntime: u32,
    nonce: u32,
    version: u32,
) -> Vec<u8> {
    request(id, "mining.submit", |out| {
        for s in [user, job_id, extranonce_2] {
            quoted(out, s);
            out.extend(b", ");
        }
        let _ = write!(out, r#""{ntime:08x}", "{nonce:08x}", "{version:08x}""#);
    })
}

pub fn configure_version_rolling(id: i32) -> Vec<u8> {
    request(id, "mining.configure", |out| {
        out.extend(br#"["version-rolling"], {"version-rolling.mask": "1fffe000"}"#);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: Vec<u8>) -> String {
        String::from_utf8(v).unwrap()
    }

    #[test]
    fn formats_match_the_cpp_code() {
        assert_eq!(
            s(subscribe(1, b"NerdQAxe+", b"BM1370", b"v1.0.36")),
            "{\"id\": 1, \"method\": \"mining.subscribe\", \"params\": [\"NerdQAxe+/BM1370/v1.0.36\"]}\n"
        );
        assert_eq!(
            s(submit(9, b"bc1q.worker", b"67a1", b"00000001", 0x66b9a2f0, 0xdeadbeef, 0x1fffe000)),
            "{\"id\": 9, \"method\": \"mining.submit\", \"params\": [\"bc1q.worker\", \"67a1\", \"00000001\", \"66b9a2f0\", \"deadbeef\", \"1fffe000\"]}\n"
        );
        assert_eq!(
            s(suggest_difficulty(4, 3_000_000_000)),
            "{\"id\": 4, \"method\": \"mining.suggest_difficulty\", \"params\": [3000000000]}\n"
        );
        assert_eq!(
            s(extranonce_subscribe(5)),
            "{\"id\": 5, \"method\": \"mining.extranonce.subscribe\", \"params\": []}\n"
        );
    }

    #[test]
    fn strings_are_escaped() {
        let r = authorize(3, b"user\"x", b"p\\w\n");
        let v: serde_json::Value = serde_json::from_slice(&r).unwrap();
        assert_eq!(v["params"][0], "user\"x");
        assert_eq!(v["params"][1], "p\\w\n");
    }
}
