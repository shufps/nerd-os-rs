//! Golden tests of the settings bodies: the Rust parsers
//! (rust_core::http::settings) and settings_apply writing into a recording
//! config layer, against the NVS writes of the former ArduinoJson handling
//! (tests/golden/http_settings.txt, recorded by the differential tests).

use std::cell::{Cell, RefCell};

use rust_core::config::settings_apply::{apply, Applied, Store};
use rust_core::http::settings::Write;
use rust_core::http::v1::settings as v1;
use rust_core::http::v2::settings as v2;

mod golden;
use golden::Rec;

struct Rng(Cell<u64>);
impl Rng {
    fn next(&self) -> u64 {
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: u64) -> u64 {
        self.next() % n
    }
    fn pick<'a, T>(&self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

const V1_KEYS: &[&str] = &[
    "ssid",
    "wifiPass",
    "hostname",
    "coreVoltage",
    "frequency",
    "jobInterval",
    "stratumDifficulty",
    "flipscreen",
    "overheat_temp",
    "invertscreen",
    "invertfanpolarity",
    "autofanspeed",
    "manualFanSpeed",
    "autoscreenoff",
    "stratum_keep",
    "pidTargetTemp",
    "pidP",
    "pidI",
    "pidD",
    "vrFrequency",
    "fans",
    "poolMode",
    "poolBalance",
    "stratumURL",
    "stratumUser",
    "stratumPassword",
    "stratumPort",
    "stratumEnonceSubscribe",
    "stratumTLS",
    "fallbackStratumURL",
    "fallbackStratumUser",
    "fallbackStratumPassword",
    "fallbackStratumPort",
    "fallbackStratumEnonceSubscribe",
    "fallbackStratumTLS",
    "stratumProtocol",
    "fallbackStratumProtocol",
    "sv2AuthorityPubkey",
    "fallbackSv2AuthorityPubkey",
    "sv2ChannelType",
    "fallbackSv2ChannelType",
];
const V2_KEYS: &[&str] = &[
    "ssid",
    "wifiPass",
    "hostname",
    "coreVoltage",
    "frequency",
    "jobInterval",
    "stratumDifficulty",
    "vrFrequency",
    "flipScreen",
    "invertScreen",
    "autoScreenOff",
    "mempoolCustom",
    "mempoolUrl",
    "invertFanPolarity",
    "pidUseMax",
    "stratumKeep",
    "canMaster",
    "fans",
    "poolMode",
    "poolBalance",
    "pools",
];
const FAN_KEYS: &[&str] = &["mode", "manualSpeed", "overheatTemp", "pid"];
const PID_KEYS: &[&str] = &["targetTemp", "p", "i", "d"];
const POOL_KEYS: &[&str] = &[
    "url",
    "port",
    "user",
    "password",
    "enonceSubscribe",
    "tls",
    "protocol",
    "sv2AuthorityPubkey",
    "sv2ChannelType",
    "sv2RequireAuth",
    "coinbaseVerifyMode",
    "coinbaseMaxFee",
    "coinbaseVerifyForce",
];
/// Keys whose float values are cast to uint16_t: only values in range here
/// (the former code did not define the others; Rust saturates).
const SCALED: &[&str] = &["pidP", "pidI", "pidD", "p", "i", "d", "coinbaseMaxFee"];

fn json_string(rng: &Rng) -> String {
    let parts = [
        "pool.example.com",
        "",
        "a\\\"b",
        "\\u0000cut",
        "x\\u0000",
        "caf\\u00e9",
        "\\u00ff",
        "\\n\\t",
        "123",
        "true",
        "bc1qxyz.worker",
        "\\ud83d\\ude00",
        "1e2",
        "-5",
        "0x10",
    ];
    let mut s = String::from("\"");
    for _ in 0..1 + rng.below(3) {
        s.push_str(parts[rng.below(parts.len() as u64) as usize]);
    }
    s.push('"');
    s
}

fn number(rng: &Rng, scaled: bool) -> String {
    if scaled {
        // in range for x100 / x10 as uint16_t
        return rng
            .pick(&[
                "0", "1", "0.5", "1.25", "2", "0.01", "0.29", "3.5", "100", "655.35", "0.0",
                "-0.0", "1e1", "12.345", "2.25", "0.05",
            ])
            .to_string();
    }
    rng.pick(&[
        "0",
        "1",
        "-1",
        "-0",
        "100",
        "255",
        "65535",
        "65536",
        "4294967295",
        "4294967296",
        "2147483647",
        "2147483648",
        "-2147483648",
        "-2147483649",
        "18446744073709551615",
        "18446744073709551616",
        "1.0",
        "1.5",
        "-0.5",
        "1e2",
        "1E-2",
        "655.35",
        "3e9",
        "1e20",
        "0.000001",
        "12345678901234567890123",
    ])
    .to_string()
}

fn value(rng: &Rng, key: &str, depth: usize) -> String {
    let scaled = SCALED.contains(&key);
    match rng.below(if depth > 3 { 7 } else { 10 }) {
        0 | 1 => json_string(rng),
        2 => rng.pick(&["true", "false"]).to_string(),
        3..=5 => number(rng, scaled),
        6 => "null".to_string(),
        7 => object(rng, &[], depth + 1),
        _ => {
            let n = rng.below(4);
            let items: Vec<String> = (0..n).map(|_| value(rng, "", depth + 1)).collect();
            format!("[{}]", items.join(","))
        }
    }
}

fn object(rng: &Rng, keys: &[&str], depth: usize) -> String {
    let n = rng.below(if keys.is_empty() { 3 } else { 12 });
    let mut members = Vec::new();
    for _ in 0..n {
        let key = if !keys.is_empty() && rng.below(8) != 0 {
            *rng.pick(keys)
        } else {
            *rng.pick(&["x", "Ssid", "fan", "pool", ""][..])
        };
        let v = match key {
            "fans" => array(rng, FAN_KEYS, depth),
            "pools" => array(rng, POOL_KEYS, depth),
            "pid" => {
                if rng.below(4) == 0 {
                    value(rng, key, depth)
                } else {
                    object(rng, PID_KEYS, depth + 1)
                }
            }
            _ => value(rng, key, depth),
        };
        members.push(format!("\"{key}\":{v}"));
    }
    format!("{{{}}}", members.join(","))
}

/// An array of objects (sometimes something else in between).
fn array(rng: &Rng, keys: &[&str], depth: usize) -> String {
    if rng.below(6) == 0 {
        return value(rng, "", depth);
    }
    let n = rng.below(4);
    let items: Vec<String> = (0..n)
        .map(|_| {
            if rng.below(5) == 0 {
                value(rng, "", depth + 1)
            } else {
                object(rng, keys, depth + 2)
            }
        })
        .collect();
    format!("[{}]", items.join(","))
}

/// A body: mostly valid, sometimes with trailing data, cut off, or not an
/// object.
fn body(rng: &Rng, keys: &[&str]) -> String {
    let mut b = object(rng, keys, 1);
    match rng.below(12) {
        0 => {
            let tails = [" ", "x", "}", " {\"ssid\":\"late\"}", "\n,"];
            b.push_str(tails[rng.below(tails.len() as u64) as usize]);
        }
        1 => {
            let cut = rng.below(b.len() as u64) as usize;
            b.truncate(cut);
            while !b.is_char_boundary(b.len()) {
                b.pop();
            }
        }
        2 => b = value(rng, "", 1),
        3 => b = format!(" \t\r\n{b}"),
        _ => {}
    }
    b
}

/// The config layer as the former test stub logs it ("key=type:value").
struct Log<'a>(&'a RefCell<String>);

impl Store for Log<'_> {
    fn set_u16(&mut self, key: &[u8], v: u16) {
        let line = format!("{}=u16:{v}\n", String::from_utf8_lossy(key));
        self.0.borrow_mut().push_str(&line);
    }
    fn set_u64(&mut self, key: &[u8], v: u64) {
        let line = format!("{}=u64:{v}\n", String::from_utf8_lossy(key));
        self.0.borrow_mut().push_str(&line);
    }
    fn set_str(&mut self, key: &[u8], v: Option<&[u8]>) {
        let v = String::from_utf8_lossy(v.unwrap_or_default());
        let line = format!("{}=s:{v}\n", String::from_utf8_lossy(key));
        self.0.borrow_mut().push_str(&line);
    }
    fn get_u16(&mut self, _: &[u8], def: u16) -> u16 {
        def
    }
}

/// As the handlers: invalid JSON applies nothing. The settings build is a
/// NerdAxe (fanPidUseMax applies). The result and the config writes.
fn run(
    body: &[u8],
    parse: impl Fn(&rust_core::ajson::Json) -> Vec<Write>,
) -> ((bool, Applied), String) {
    let log = RefCell::new(String::new());
    let mut a = Applied::default();
    let Ok(doc) = rust_core::ajson::deserialize(body) else {
        return ((false, a), log.into_inner());
    };
    for w in parse(&doc) {
        apply(&w, &mut a, true, &mut Log(&log));
    }
    ((true, a), log.into_inner())
}

fn settings_v1(body: &[u8], dual_pool: bool) -> (bool, String) {
    let ((ok, _), log) = run(body, |d| v1::settings_v1(d, dual_pool));
    (ok, log)
}

fn settings_v2(body: &[u8]) -> ((bool, [bool; 2]), String) {
    let ((ok, a), log) = run(body, v2::settings_v2);
    ((ok, a.verify_changed), log)
}

#[test]
fn v1_bodies_match_reference() {
    let rng = Rng(Cell::new(0x0001));
    let mut rec = Rec::new("http_settings", "v1_bodies_match_reference");
    for _ in 0..30_000 {
        let b = body(&rng, V1_KEYS);
        let dual = rng.below(2) == 0;
        let current = settings_v1(b.as_bytes(), dual);
        rec.add(format!("{b:?} dual {dual} -> {}\n{}", current.0, current.1));
    }
    rec.finish();
}

#[test]
fn v2_bodies_match_reference() {
    let rng = Rng(Cell::new(0x0002));
    let mut rec = Rec::new("http_settings", "v2_bodies_match_reference");
    for _ in 0..30_000 {
        let b = body(&rng, V2_KEYS);
        let current = settings_v2(b.as_bytes());
        rec.add(format!("{b:?} -> {:?}\n{}", current.0, current.1));
    }
    rec.finish();
}

#[test]
fn nesting_limit_matches_reference() {
    let mut rec = Rec::new("http_settings", "nesting_limit_matches_reference");
    for depth in 0..14 {
        let inner = format!("{}1{}", "[".repeat(depth), "]".repeat(depth));
        for b in [
            format!("{{\"ssid\":\"x\",\"fans\":{inner}}}"),
            format!("{{\"fans\":[{{\"pid\":{{\"p\":{inner}}}}}]}}"),
            inner.clone(),
        ] {
            let current = settings_v1(b.as_bytes(), false);
            rec.add(format!("{b:?} -> {}\n{}", current.0, current.1));
        }
    }
    rec.finish();
}

/// Token soup with what ArduinoJson accepts beyond JSON (single quotes,
/// unquoted keys, odd numbers, raw bytes, broken escapes), around the keys
/// the handlers read. No top-level PID factor: those values go out of range
/// here (the Rust version saturates them).
#[test]
fn unusual_syntax_matches_reference() {
    let rng = Rng(Cell::new(0x0003));
    let mut rec = Rec::new("http_settings", "unusual_syntax_matches_reference");
    let keys = [
        "ssid",
        "'ssid'",
        "\"ssid\"",
        "pidTargetTemp",
        "frequency",
        "'fans'",
        "fans",
        "pools",
        "stratumPort",
        "x_1",
        "`k`",
        "\"fr\\u0065quency\"",
    ];
    let values: &[&[u8]] = &[
        b"+1",
        b".5",
        b"01",
        b"1.",
        b"-.5",
        b"1e",
        b"1e+",
        b"1E-2",
        b"--1",
        b"1.2.3",
        b"0x10",
        b"1e400",
        b"-1e400",
        b"1e-400",
        b"123456789012345678901234567890123456789012345678901234567890123456789",
        b"0.05",
        b"0.29",
        b"'single'",
        b"\"dq\"",
        b"'it\\'s'",
        b"\"\\x\"",
        b"\"\\u12\"",
        b"\"\\uZZZZ\"",
        b"\"\\ud800\\u0041\"",
        b"\"\\udc00\"",
        b"\"\\ud83d\\ude00\"",
        b"\"\xc3\xa9\xff\x80\"",
        b"\"a\\/b\"",
        b"tru",
        b"nul",
        b"truex",
        b"[1,]",
        b"{}",
        b"[]",
        b"{'pid':{'p':0.07}}",
        b"[{\"mode\":2},{'mode':+3}]",
        b"\t\r\n 7",
        b"null",
        b"true",
        b"false",
        b"\"\"",
        b"5",
        b"65535",
        b"\x01",
    ];
    for _ in 0..50_000 {
        let mut b: Vec<u8> = Vec::new();
        b.extend(
            rng.pick(&[&b"{"[..], b"{", b"{ ", b" {\n", b"", b"["])
                .iter(),
        );
        for j in 0..rng.below(6) {
            if j > 0 {
                b.extend(rng.pick(&[&b","[..], b",", b" , ", b";", b",,"]).iter());
            }
            b.extend(rng.pick(&keys).as_bytes());
            b.extend(rng.pick(&[&b":"[..], b":", b" : ", b"="]).iter());
            b.extend(*rng.pick(values));
        }
        b.extend(
            rng.pick(&[&b"}"[..], b"}", b"} ", b"}x", b"", b"]", b"}}"])
                .iter(),
        );
        b.retain(|&c| c != 0);
        let dual = rng.below(2) == 0;
        let v1 = settings_v1(&b, dual);
        let current = settings_v2(&b);
        rec.add(format!(
            "\"{}\" dual {dual} -> {}\n{}v2 -> {:?}\n{}",
            b.escape_ascii(),
            v1.0,
            v1.1,
            current.0,
            current.1
        ));
    }
    rec.finish();
}
