//! Differential tests of HTTP step H2b: influx, alert, OTP and CAN node
//! bodies, CAN node IDs and actions from the URI, history query parameters
//! and allTime. The former code (c-http/bodies_ref.cpp, verbatim, with
//! ArduinoJson and ESP-IDF's httpd_query_key_value) against the Rust parsers.

use std::cell::Cell;
use std::ffi::{c_char, CString};
use std::sync::Mutex;

use c_reference as _;
use rust_core::http_query::HistoryQuery;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

#[repr(C)]
#[derive(Debug, Default, PartialEq, Eq)]
struct RefHistoryQuery {
    start: u64,
    current: u64,
    span_ms: u64,
    limit: u32,
    requested: bool,
}

extern "C" {
    fn ref_influx(body: *const c_char) -> bool;
    fn ref_alert(body: *const c_char) -> bool;
    fn ref_otp(body: *const c_char, enabled: *mut bool) -> i32;
    fn ref_can_node_id(uri: *const c_char) -> i32;
    fn ref_can_body(body: *const c_char, slave: u8) -> bool;
    fn ref_can_action(uri: *const c_char) -> i32;
    fn ref_history_query(query: *const c_char, v2: bool, out: *mut RefHistoryQuery);
    fn ref_all_time(query: *const c_char) -> bool;

    fn new_influx(body: *const c_char) -> bool;
    fn new_alert(body: *const c_char) -> bool;
    fn new_otp(body: *const c_char, enabled: *mut bool) -> i32;
    fn new_can_node_id(uri: *const c_char) -> i32;
    fn new_can_body(body: *const c_char, slave: u8) -> bool;
    fn new_can_action(uri: *const c_char) -> i32;
    fn new_history_query(query: *const c_char, v2: bool, out: *mut HistoryQuery);
    fn new_all_time(query: *const c_char) -> bool;

    fn cfg_log_reset();
    fn cfg_log(len: *mut usize) -> *const c_char;
}

/// The config log is global (shared with tests/http_settings.rs only in
/// its own binary).
static CFG: Mutex<()> = Mutex::new(());

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

fn logged<T>(f: impl FnOnce() -> T) -> (T, String) {
    unsafe { cfg_log_reset() };
    let r = f();
    let mut len = 0;
    let p = unsafe { cfg_log(&mut len) };
    let log = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(p as *const u8, len) })
        .into_owned();
    (r, log)
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
    let _g = CFG.lock().unwrap_or_else(|e| e.into_inner());
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
    for i in 0..20_000 {
        let b = body(&rng, &influx);
        let c = CString::new(b.clone()).unwrap();
        let new = logged(|| unsafe { new_influx(c.as_ptr()) });
        assert_eq!(
            new,
            logged(|| unsafe { ref_influx(c.as_ptr()) }),
            "influx {i}: {b}"
        );
        rec.add(format!("influx {b:?} -> {}\n{}", new.0, new.1));
        let b = body(&rng, &alert);
        let c = CString::new(b.clone()).unwrap();
        let new = logged(|| unsafe { new_alert(c.as_ptr()) });
        assert_eq!(
            new,
            logged(|| unsafe { ref_alert(c.as_ptr()) }),
            "alert {i}: {b}"
        );
        rec.add(format!("alert {b:?} -> {}\n{}", new.0, new.1));
        let b = body(&rng, &["enabled"]);
        let c = CString::new(b.clone()).unwrap();
        let (mut e1, mut e2) = (false, false);
        let new = unsafe { new_otp(c.as_ptr(), &mut e1) };
        let reference = unsafe { ref_otp(c.as_ptr(), &mut e2) };
        assert_eq!(
            (new, new == 0 && e1),
            (reference, reference == 0 && e2),
            "otp {i}: {b}"
        );
        rec.add(format!("otp {b:?} -> {new} {}", new == 0 && e1));
    }
    rec.finish();
}

#[test]
fn can_node_bodies_match_reference() {
    let _g = CFG.lock().unwrap_or_else(|e| e.into_inner());
    let rng = Rng(Cell::new(0xca17));
    let mut rec = Rec::new("http_bodies", "can_node_bodies_match_reference");
    let keys = [
        "frequency",
        "coreVoltage",
        "fans",
        "flipScreen",
        "autoScreenOff",
    ];
    for i in 0..30_000 {
        let b = body(&rng, &keys);
        let c = CString::new(b.clone()).unwrap();
        let slave = 1 + rng.below(31) as u8;
        let new = logged(|| unsafe { new_can_body(c.as_ptr(), slave) });
        assert_eq!(
            new,
            logged(|| unsafe { ref_can_body(c.as_ptr(), slave) }),
            "case {i}: {b}"
        );
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
    for i in 0..50_000 {
        let u = uri(&rng);
        let c = CString::new(u.clone()).unwrap();
        let id = unsafe { new_can_node_id(c.as_ptr()) };
        assert_eq!(id, unsafe { ref_can_node_id(c.as_ptr()) }, "id {i}: {u:?}");
        let action = unsafe { new_can_action(c.as_ptr()) };
        assert_eq!(
            action,
            unsafe { ref_can_action(c.as_ptr()) },
            "action {i}: {u:?}"
        );
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
    for i in 0..100_000 {
        let q = query(&rng);
        let c = CString::new(q.clone()).unwrap();
        let none = rng.below(20) == 0;
        let ptr = if none { std::ptr::null() } else { c.as_ptr() };
        let mut out = if none {
            "none".to_string()
        } else {
            format!("{q:?}")
        };
        for v2 in [false, true] {
            let mut r = RefHistoryQuery::default();
            let mut n = HistoryQuery::default();
            unsafe {
                ref_history_query(ptr, v2, &mut r);
                new_history_query(ptr, v2, &mut n);
            }
            let new = RefHistoryQuery {
                start: n.start,
                current: n.current,
                span_ms: n.span_ms,
                limit: n.limit,
                requested: n.requested,
            };
            assert_eq!(new, r, "case {i} v2 {v2}: {q:?}");
            out += &format!(
                "\nv2 {v2} -> start {} cur {} span {} limit {} requested {}",
                new.start, new.current, new.span_ms, new.limit, new.requested
            );
        }
        let all = unsafe { new_all_time(ptr) };
        assert_eq!(all, unsafe { ref_all_time(ptr) }, "allTime {i}: {q:?}");
        rec.add(format!("{out}\nallTime {all}"));
    }
    rec.finish();
}
