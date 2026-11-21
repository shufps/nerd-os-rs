//! Golden tests of the configuration store (rust_core::config): getters
//! on random documents, random set sequences and the saved document,
//! against the outputs of the accessors of the former nvs_config.cpp on
//! ArduinoJson (tests/golden/config.txt, recorded by the differential
//! tests through the C API config_store.h).

use std::ffi::CString;

use rust_core::ajson::{Json, Num};
use rust_core::config;

mod golden;
use golden::Rec;

/// The store is global: one test at a time.
static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

const VALUES: &[&str] = &[
    "0",
    "1",
    "2",
    "65535",
    "65536",
    "70000",
    "-1",
    "-70000",
    "1.9",
    "-0.5",
    "65535.9",
    "65536.0",
    "1e3",
    "1e19",
    "18446744073709551615",
    "18446744073709551616",
    "9223372036854775807",
    "-9223372036854775808",
    "true",
    "false",
    "null",
    "\"\"",
    "\"42\"",
    "\"x\"",
    "\"1.5\"",
    "\" 7\"",
    "\"0x10\"",
    "\"a\\u0000b\"",
    "[]",
    "{}",
    "[1]",
    "{\"k\":1}",
    "4294967040.0",
    "4294967296.5",
    "1.8446742974197924e19",
];
const KEYS: &[&str] = &["a", "b", "c", "fanspeed", "missing"];

/// The saved document (as `config_flush()` after `config_mark_dirty()`).
fn saved() -> String {
    let mut out = String::new();
    config::mark_dirty();
    config::flush_with(|json| {
        out = String::from_utf8_lossy(json).into_owned();
        true
    });
    out
}

/// The getters (as through the C API); the values, one line per key.
fn getters() -> String {
    let mut out = String::new();
    for k in KEYS {
        let key = k.as_bytes();
        let u16s: Vec<u16> = [0u16, 7, 65535]
            .iter()
            .map(|&def| config::get_u16(key, def))
            .collect();
        let u64s: Vec<u64> = [0u64, 9, u64::MAX]
            .iter()
            .map(|&def| config::get_u64(key, def))
            .collect();
        let bools: Vec<bool> = [false, true]
            .iter()
            .map(|&def| config::get_bool(key, def))
            .collect();
        let has = config::has(key);
        // a C string: up to the first NUL
        let st =
            config::get_str(key).map(|v| v.split(|&b| b == 0).next().unwrap_or_default().to_vec());
        out += &format!("{k}: {u16s:?} {u64s:?} {bools:?} {has} {st:?}\n");
    }
    out
}

#[test]
fn getters_on_random_documents() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xdead_beef_1234_5678);
    let mut rec = Rec::new("config", "getters_on_random_documents");
    for _ in 0..20_000 {
        let mut doc = String::from("{");
        for i in 0..r.next() % 5 {
            if i > 0 {
                doc.push(',');
            }
            doc.push_str(&format!("\"{}\":{}", r.pick(KEYS), r.pick(VALUES)));
        }
        doc.push('}');
        let ok = config::load(doc.as_bytes());
        let got = getters();
        let saved = saved();
        rec.add(format!("{doc} {ok}\n{got}{saved}"));
    }
    rec.finish();
}

#[test]
fn random_set_sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x0123_4567_89ab_cdef);
    let mut rec = Rec::new("config", "random_set_sequences");
    let strings = [
        "",
        "x",
        "with \"quote\"",
        "back\\slash",
        "nl\nand\ttab",
        "äöü",
    ];
    for _ in 0..5_000 {
        let doc = "{\"b\":1,\"fanspeed\":\"old\"}";
        let mut ops = String::new();
        config::load(doc.as_bytes());
        for _ in 0..r.next() % 8 {
            // as C strings (their `{:?}` is part of the golden outputs)
            let key = CString::new(*r.pick(KEYS)).unwrap();
            let k = key.to_bytes();
            match r.next() % 5 {
                0 => {
                    let v = CString::new(*r.pick(&strings)).unwrap();
                    config::set(k, Json::Str(v.to_bytes().to_vec()));
                    ops += &format!("str {key:?} {v:?}\n");
                }
                1 => {
                    config::set(k, Json::Null);
                    ops += &format!("null {key:?}\n");
                }
                2 => {
                    let v = r.next() as u16;
                    config::set(k, Json::Num(Num::U(v as u64)));
                    ops += &format!("u16 {key:?} {v}\n");
                }
                3 => {
                    let v = r.next() % 2 == 0;
                    config::set(k, Json::Bool(v));
                    ops += &format!("bool {key:?} {v}\n");
                }
                _ => {
                    let v = r.next();
                    config::set(k, Json::Num(Num::U(v)));
                    ops += &format!("u64 {key:?} {v}\n");
                }
            }
        }
        let saved = saved();
        let got = getters();
        rec.add(format!("{ops}{saved}\n{got}"));
    }
    rec.finish();
}
