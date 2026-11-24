//! Golden tests of HTTP step H2b: influx, alert, OTP and CAN node bodies,
//! CAN node IDs and actions from the URI, history query parameters and
//! allTime. The Rust parsers against the outputs of the former code
//! (ArduinoJson and ESP-IDF's httpd_query_key_value;
//! tests/golden/http_bodies.txt, recorded by the differential tests).

use std::cell::{Cell, RefCell};

use rust_core::http::query::{self, NodeId};
use rust_core::http::settings::{self, Setting, Write};
use rust_core::settings_apply::{apply, Applied, Store};

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
    fn pick<T: Copy>(&self, items: &[T]) -> T {
        items[self.below(items.len() as u64) as usize]
    }
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

/// As the handlers (settings_apply): invalid JSON applies nothing. The
/// result and the config writes.
fn settings(body: &[u8], parse: impl Fn(&rust_core::ajson::Json) -> Vec<Write>) -> (bool, String) {
    let log = RefCell::new(String::new());
    let mut a = Applied::default();
    let Ok(doc) = rust_core::ajson::deserialize(body) else {
        return (false, log.into_inner());
    };
    for w in parse(&doc) {
        apply(&w, &mut a, true, &mut Log(&log));
    }
    (true, log.into_inner())
}

/// 0: ok (`enabled` set), 1: invalid JSON, 2: no bool `enabled`.
fn otp(body: &[u8]) -> (i32, bool) {
    match settings::otp_body(body) {
        settings::OtpBody::Invalid => (1, false),
        settings::OtpBody::NoEnabled => (2, false),
        settings::OtpBody::Enabled(b) => (0, b),
    }
}

/// The CAN node commands as the CAN sink of handler_can_swarm.cpp logged
/// them ("can=s:...").
fn can_body(body: &[u8], slave: u8) -> (bool, String) {
    let Ok(doc) = rust_core::ajson::deserialize(body) else {
        return (false, String::new());
    };
    let mut log = String::new();
    for w in settings::can_node(&doc) {
        let n = w.num;
        let line = match w.setting {
            Setting::CanFrequency => format!("freq {slave} {}", n as u16),
            Setting::CanVoltage => format!("voltage {slave} {}", n as u16),
            Setting::CanFan => format!(
                "fan {slave} {} {} {} {} {}",
                w.index,
                n & 0xff,
                (n >> 8) & 0xff,
                (n >> 16) & 0xff,
                (n >> 24) & 0xff
            ),
            Setting::CanDisplay => format!("display {slave} {} {}", n & 1, (n >> 1) & 1),
            _ => continue,
        };
        log += &format!("can=s:{line}\n");
    }
    (true, log)
}

/// The node ID, or -1 (missing), -2 (invalid).
fn can_node_id(uri: &[u8]) -> i32 {
    match query::can_node_id(uri) {
        NodeId::Missing => -1,
        NodeId::Invalid => -2,
        NodeId::Id(id) => id as i32,
    }
}

/// -2: no ID, -3: unknown action, else 100 * (action + 1) + ID.
fn can_action(uri: &[u8]) -> i32 {
    match query::can_node_action(uri) {
        (NodeId::Id(id), Some(a)) => 100 * (a as i32 + 1) + id as i32,
        (NodeId::Id(_), None) => -3,
        _ => -2,
    }
}

/// The handlers pass the query only when it fit into their buffer (128
/// bytes for the history, 32 for allTime).
fn fits(query: Option<&[u8]>, buf: usize) -> Option<&[u8]> {
    query.filter(|q| q.len() < buf)
}

const VALUES: &[&str] = &[
    "true",
    "false",
    "null",
    "0",
    "1",
    "-1",
    "2",
    "255",
    "256",
    "65535",
    "65536",
    "1.5",
    "255.9",
    "-0.5",
    "1e2",
    "\"x\"",
    "\"12\"",
    "\"3.7\"",
    "\"\"",
    "\"http://h:8086\"",
    "'s'",
    "[]",
    "{}",
    "[1,2]",
    "{\"mode\":3}",
    "\"a\\u0000b\"",
    "+7",
    ".5",
    "4294967296",
];

fn body(rng: &Rng, keys: &[&str]) -> String {
    let n = rng.below(8);
    let members: Vec<String> = (0..n)
        .map(|_| {
            let key = if rng.below(8) == 0 {
                "x"
            } else {
                rng.pick(keys)
            };
            let value = match key {
                "fans" if rng.below(3) != 0 => {
                    let fans: Vec<String> = (0..rng.below(4))
                        .map(|_| {
                            if rng.below(5) == 0 {
                                rng.pick(VALUES).to_string()
                            } else {
                                let m: Vec<String> = (0..rng.below(5))
                                    .map(|_| {
                                        format!(
                                            "\"{}\":{}",
                                            rng.pick(&[
                                                "mode",
                                                "manualSpeed",
                                                "targetTemp",
                                                "overheatTemp",
                                                "x"
                                            ]),
                                            rng.pick(VALUES)
                                        )
                                    })
                                    .collect();
                                format!("{{{}}}", m.join(","))
                            }
                        })
                        .collect();
                    format!("[{}]", fans.join(","))
                }
                _ => rng.pick(VALUES).to_string(),
            };
            format!("\"{key}\":{value}")
        })
        .collect();
    let mut b = format!("{{{}}}", members.join(","));
    match rng.below(10) {
        0 => b.truncate(rng.below(b.len() as u64 + 1) as usize),
        1 => b = rng.pick(VALUES).to_string(),
        _ => {}
    }
    b
}

#[test]
fn influx_alert_otp_bodies_match_reference() {
    let rng = Rng(Cell::new(0xb0d1));
    let mut rec = Rec::new("http_bodies", "influx_alert_otp_bodies_match_reference");
    let influx = ["enabled", "url", "port", "token", "bucket", "org", "prefix"];
    let alert = [
        "webhookUrl",
        "watchdogEnable",
        "blockFoundEnable",
        "bestDiffEnable",
        "coinbaseVerifyEnable",
        "showBlockFoundScreen",
    ];
    for _ in 0..20_000 {
        let b = body(&rng, &influx);
        let new = settings(b.as_bytes(), settings::settings_influx);
        rec.add(format!("influx {b:?} -> {}\n{}", new.0, new.1));
        let b = body(&rng, &alert);
        let new = settings(b.as_bytes(), settings::settings_alert);
        rec.add(format!("alert {b:?} -> {}\n{}", new.0, new.1));
        let b = body(&rng, &["enabled"]);
        let (new, e1) = otp(b.as_bytes());
        rec.add(format!("otp {b:?} -> {new} {}", new == 0 && e1));
    }
    rec.finish();
}

#[test]
fn can_node_bodies_match_reference() {
    let rng = Rng(Cell::new(0xca17));
    let mut rec = Rec::new("http_bodies", "can_node_bodies_match_reference");
    let keys = [
        "frequency",
        "coreVoltage",
        "fans",
        "flipScreen",
        "autoScreenOff",
    ];
    for _ in 0..30_000 {
        let b = body(&rng, &keys);
        let slave = 1 + rng.below(31) as u8;
        let new = can_body(b.as_bytes(), slave);
        rec.add(format!("{b:?} slave {slave} -> {}\n{}", new.0, new.1));
    }
    rec.finish();
}

fn uri(rng: &Rng) -> String {
    let segment = |rng: &Rng| {
        rng.pick(&[
            "0",
            "1",
            "5",
            "31",
            "32",
            "-1",
            "007",
            " 3",
            "+4",
            "3x",
            "99999999999",
            "-99999999999",
            "",
            "a",
            "restart",
            "shutdown",
            "identify",
            "Restart",
            "2?x=1",
            "\t9",
        ])
        .to_string()
    };
    let mut u = rng
        .pick(&[
            "/api/v2/can/nodes/",
            "/api/v2/can/nodes",
            "",
            "/",
            "nodes/",
            "/a/b/",
        ])
        .to_string();
    for j in 0..rng.below(4) {
        if j > 0 {
            u.push('/');
        }
        u.push_str(&segment(rng));
    }
    u
}

#[test]
fn can_node_uris_match_reference() {
    let rng = Rng(Cell::new(0x0ad));
    let mut rec = Rec::new("http_bodies", "can_node_uris_match_reference");
    for _ in 0..50_000 {
        let u = uri(&rng);
        let id = can_node_id(u.as_bytes());
        let action = can_action(u.as_bytes());
        rec.add(format!("{u:?} -> id {id} action {action}"));
    }
    rec.finish();
}

fn query(rng: &Rng) -> String {
    let keys = [
        "ts",
        "TS",
        "limit",
        "historySpan",
        "history_span",
        "History_Span",
        "cur",
        "allTime",
        "alltime",
        "x",
        "",
    ];
    let values = [
        "",
        "0",
        "1",
        "true",
        "TRUE",
        "1000",
        "1001",
        "-1",
        "+5",
        " 7",
        "12x",
        "4294967295",
        "4294967296",
        "18446744073709551615",
        "18446744073709551616",
        "10800000",
        "10800001",
        "1731767000000",
        "1234567890123456789012345678901234567890123456789012345678901234567890",
    ];
    let n = rng.below(6);
    let pairs: Vec<String> = (0..n)
        .map(|_| match rng.below(8) {
            0 => rng.pick(&keys).to_string(),
            1 => format!("{}=={}", rng.pick(&keys), rng.pick(&values)),
            _ => format!("{}={}", rng.pick(&keys), rng.pick(&values)),
        })
        .collect();
    pairs.join(rng.pick(&["&", "&", "&&", ";"]))
}

#[test]
fn query_parameters_match_reference() {
    let rng = Rng(Cell::new(0x9e7));
    let mut rec = Rec::new("http_bodies", "query_parameters_match_reference");
    for _ in 0..100_000 {
        let q = query(&rng);
        let none = rng.below(20) == 0;
        let arg = if none { None } else { Some(q.as_bytes()) };
        let mut out = if none {
            "none".to_string()
        } else {
            format!("{q:?}")
        };
        for v2 in [false, true] {
            let new = query::history_query(fits(arg, 128), v2);
            out += &format!(
                "\nv2 {v2} -> start {} cur {} span {} limit {} requested {}",
                new.start, new.current, new.span_ms, new.limit, new.requested
            );
        }
        let all = query::all_time(fits(arg, 32));
        rec.add(format!("{out}\nallTime {all}"));
    }
    rec.finish();
}
