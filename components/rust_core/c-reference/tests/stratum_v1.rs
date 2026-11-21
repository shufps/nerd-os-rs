//! Compares the current StratumApi (C++ glue + Rust codec, as built into the
//! firmware) with the former C++/ArduinoJson implementation, compiled for the
//! host: parsing of random and mutated pool messages, request formatting and
//! line framing.
//!
//! `DIFF_ITERATIONS` (default 100000) and `DIFF_SEED` override the run.

use std::cell::Cell;
use std::ffi::{c_char, c_void, CStr, CString};

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::stratum_v1::{method, Message, MiningNotify, EXTRANONCE2_LEN};
use serde_json::{json, Value};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

// ---- C interface (c-reference/cpp-shim) ----

macro_rules! api {
    ($($prefix:ident),*) => {
        extern "C" {
            $(
                api!(@fns $prefix);
            )*
        }
    };
    (@fns ref_) => {
        fn ref_parse(json: *const c_char, m: *mut Message) -> bool;
        fn ref_api_parse(a: *mut c_void, json: *const c_char, m: *mut Message) -> bool;
        fn ref_api_new() -> *mut c_void;
        fn ref_api_free(a: *mut c_void);
        fn ref_receive(a: *mut c_void, t: *mut c_void) -> *mut c_char;
        fn ref_subscribe(a: *mut c_void, t: *mut c_void, device: *const c_char, asic: *const c_char) -> bool;
        fn ref_extranonce_subscribe(a: *mut c_void, t: *mut c_void) -> bool;
        fn ref_suggest_difficulty(a: *mut c_void, t: *mut c_void, d: u32) -> bool;
        fn ref_authorize(a: *mut c_void, t: *mut c_void, u: *const c_char, p: *const c_char) -> bool;
        fn ref_submit(a: *mut c_void, t: *mut c_void, u: *const c_char, job: *const c_char, en2: *const c_char, ntime: u32, nonce: u32, version: u32) -> bool;
        fn ref_configure(a: *mut c_void, t: *mut c_void) -> bool;
    };
    (@fns new_) => {
        fn new_parse(json: *const c_char, m: *mut Message) -> bool;
        fn new_api_parse(a: *mut c_void, json: *const c_char, m: *mut Message) -> bool;
        fn new_api_new() -> *mut c_void;
        fn new_api_free(a: *mut c_void);
        fn new_receive(a: *mut c_void, t: *mut c_void) -> *mut c_char;
        fn new_subscribe(a: *mut c_void, t: *mut c_void, device: *const c_char, asic: *const c_char) -> bool;
        fn new_extranonce_subscribe(a: *mut c_void, t: *mut c_void) -> bool;
        fn new_suggest_difficulty(a: *mut c_void, t: *mut c_void, d: u32) -> bool;
        fn new_authorize(a: *mut c_void, t: *mut c_void, u: *const c_char, p: *const c_char) -> bool;
        fn new_submit(a: *mut c_void, t: *mut c_void, u: *const c_char, job: *const c_char, en2: *const c_char, ntime: u32, nonce: u32, version: u32) -> bool;
        fn new_configure(a: *mut c_void, t: *mut c_void) -> bool;
    };
}
api!(ref_, new_);

extern "C" {
    fn stub_new() -> *mut c_void;
    fn stub_free(t: *mut c_void);
    fn stub_push(t: *mut c_void, data: *const c_char, len: usize);
    fn stub_sent(t: *mut c_void, len: *mut usize) -> *const c_char;
    fn stub_clear_sent(t: *mut c_void);
    fn free(p: *mut c_void);
}

// ---- helpers ----

struct Rng(Cell<u64>);

impl Rng {
    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn pick<'a, T>(&self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
    fn hex(&self, n: usize) -> String {
        (0..n)
            .map(|_| *self.pick(b"0123456789abcdefABCDEF") as char)
            .collect()
    }
    fn text(&self, max: usize) -> String {
        let n = self.below(max + 1);
        (0..n)
            .map(|_| (b' ' + self.below(95) as u8) as char)
            .filter(|&c| c != '"' && c != '\\')
            .collect()
    }
}

/// Owned parse result; frees what the parser allocated.
struct Parsed {
    ok: bool,
    m: Message,
}

impl Drop for Parsed {
    fn drop(&mut self) {
        // SAFETY: both implementations allocate with malloc on the host.
        unsafe {
            let n = self.m.mining_notification;
            if !n.is_null() {
                for p in [(*n).job_id, (*n).coinbase_1, (*n).coinbase_2] {
                    free(p as _);
                }
                free(n as _);
            }
            free(self.m.extranonce_str as _);
        }
    }
}

fn parse_with(f: unsafe extern "C" fn(*const c_char, *mut Message) -> bool, json: &str) -> Parsed {
    let json = CString::new(json).unwrap();
    let mut m = Message::zeroed();
    // SAFETY: valid string and zeroed message, as the firmware passes them.
    let ok = unsafe { f(json.as_ptr(), &mut m) };
    Parsed { ok, m }
}

fn cstr(p: *const c_char) -> Option<String> {
    // SAFETY: NUL-terminated strings allocated by the parsers.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Everything the firmware reads from a parse result.
fn summary(p: &Parsed) -> String {
    let m = &p.m;
    let mut s = format!(
        "ok={} method={} id={} en2_len={} en={:?} abandon={} diff={} mask={:#x} success={}",
        p.ok,
        m.method,
        m.message_id,
        m.extranonce_2_len,
        cstr(m.extranonce_str),
        m.should_abandon_work,
        m.new_difficulty,
        m.version_mask,
        m.response_success
    );
    if !m.mining_notification.is_null() && p.ok {
        // SAFETY: set by a successful parse.
        let n: &MiningNotify = unsafe { &*m.mining_notification };
        s += &format!(
            " notify: job={:?} prev={:02x?} cb1={:?} cb2={:?} merkle={:02x?} version={:#x} nbits={:#x} ntime={:#x}",
            cstr(n.job_id),
            n.prev_block_hash,
            cstr(n.coinbase_1),
            cstr(n.coinbase_2),
            &n.merkle_branches[..n.n_merkle_branches],
            n.version,
            n.target,
            n.ntime
        );
    }
    s
}

// ---- message generator ----

/// A random JSON value, biased towards the kinds a field could plausibly
/// (or implausibly) hold.
fn any_value(rng: &Rng) -> Value {
    match rng.below(16) {
        0 => Value::Null,
        1 => json!(rng.chance(50)),
        2 => json!(rng.next() as i64),
        3 => json!(rng.below(100) as i64 - 50),
        4 => json!(*rng.pick(&[
            0.5,
            1.5,
            -0.5,
            1e3,
            1e10,
            4294967295.5,
            2147483647.0,
            65536.25
        ])),
        5 => json!(*rng.pick(&[
            0u64,
            1,
            4,
            8,
            16,
            17,
            255,
            2147483647,
            2147483648,
            4294967295,
            4294967296,
            u64::MAX
        ])),
        6 => json!(*rng.pick(&[-1i64, -2147483648, -2147483649, i64::MIN])),
        7 => json!(*rng.pick(&[
            "42",
            "4.7",
            "-3",
            "+5",
            "1e2",
            "0x10",
            " 42",
            "",
            "1.",
            ".5",
            "4294967296",
            "x"
        ])),
        8 => json!(rng.hex(rng.below(12))),
        9 => json!([]),
        10 => json!({"a": 1}),
        11 => json!(rng.text(20)),
        _ => json!(rng.below(1000)),
    }
}

fn hex_field(rng: &Rng, len: usize) -> Value {
    match rng.below(20) {
        0 => any_value(rng),
        1 => json!(format!(
            "{}{}",
            rng.pick(&["", " ", "0x", "-", "+", "  0X"]),
            rng.hex(rng.below(12))
        )),
        2 => json!(rng.text(len)),
        _ => json!(rng.hex(len)),
    }
}

fn notify(rng: &Rng) -> Value {
    let n_merkle = if rng.chance(5) {
        rng.below(40)
    } else {
        rng.below(14)
    };
    let merkle: Vec<Value> = (0..n_merkle)
        .map(|_| {
            if rng.chance(3) {
                any_value(rng)
            } else {
                hex_field(rng, 64 + rng.below(3) - 1)
            }
        })
        .collect();
    let mut params = vec![
        if rng.chance(3) {
            any_value(rng)
        } else {
            json!(rng.hex(rng.below(10)))
        },
        if rng.chance(3) {
            any_value(rng)
        } else {
            json!(rng.hex(62 + rng.below(5)))
        },
        if rng.chance(3) {
            any_value(rng)
        } else {
            json!(rng.hex(rng.below(200)))
        },
        if rng.chance(3) {
            any_value(rng)
        } else {
            json!(rng.hex(rng.below(300)))
        },
        if rng.chance(3) {
            any_value(rng)
        } else {
            json!(merkle)
        },
        hex_field(rng, 8),
        hex_field(rng, 8),
        hex_field(rng, 8),
        if rng.chance(70) {
            json!(rng.chance(50))
        } else {
            any_value(rng)
        },
    ];
    match rng.below(10) {
        0 => params.truncate(rng.below(params.len())),
        1 => params.push(any_value(rng)),
        _ => {}
    }
    json!({"params": params, "id": Value::Null, "method": "mining.notify"})
}

fn message(rng: &Rng) -> Value {
    let id = |rng: &Rng| match rng.below(10) {
        0 => any_value(rng),
        1..=4 => json!(1 + rng.below(5)),
        _ => json!(6 + rng.below(1000)),
    };
    let mut doc = match rng.below(14) {
        0..=3 => notify(rng),
        4 => json!({"id": null, "method": "mining.set_difficulty", "params": [any_value(rng)]}),
        5 => {
            json!({"id": null, "method": "mining.set_version_mask", "params": [hex_field(rng, 8)]})
        }
        6 => {
            let en2 = if rng.chance(50) {
                json!(rng.below(20) as i64 - 2)
            } else {
                any_value(rng)
            };
            let en = if rng.chance(80) {
                json!(rng.hex(8))
            } else {
                any_value(rng)
            };
            let params = if rng.chance(85) {
                json!([en, en2])
            } else {
                json!([en])
            };
            json!({"id": null, "method": "mining.set_extranonce", "params": params})
        }
        7 => {
            let en2 = if rng.chance(60) {
                json!(rng.below(20) as i64 - 2)
            } else {
                any_value(rng)
            };
            let en = if rng.chance(80) {
                json!(rng.hex(8))
            } else {
                any_value(rng)
            };
            let result = match rng.below(8) {
                0 => any_value(rng),
                1 => json!([[], en]),
                _ => json!([[["mining.notify", rng.hex(16)]], en, en2]),
            };
            json!({"id": 1, "result": result, "error": null})
        }
        8 => {
            let result = match rng.below(4) {
                0 => any_value(rng),
                _ => json!({"version-rolling": true, "version-rolling.mask": hex_field(rng, 8)}),
            };
            json!({"id": 2, "result": result, "error": null})
        }
        9 | 10 => {
            let result = if rng.chance(60) {
                json!(rng.chance(70))
            } else {
                any_value(rng)
            };
            let error = if rng.chance(70) {
                Value::Null
            } else {
                json!([23, "Low difficulty share", null])
            };
            json!({"id": id(rng), "result": result, "error": error})
        }
        11 => {
            json!({"id": null, "method": *rng.pick(&["client.reconnect", "mining.ping", "client.show_message", ""]), "params": []})
        }
        12 => json!({"id": id(rng), "method": any_value(rng), "params": [any_value(rng)]}),
        _ => any_value(rng),
    };
    // Occasionally drop or replace a top-level field.
    if let Some(obj) = doc.as_object_mut() {
        if rng.chance(5) {
            let key = rng
                .pick(&["id", "method", "params", "result", "error"])
                .to_string();
            if rng.chance(50) {
                obj.remove(&key);
            } else {
                obj.insert(key, any_value(rng));
            }
        }
    }
    doc
}

/// Inputs with missing or non-string fields, which the reference does not
/// handle, so it is not run on them; the current code must reject them.
fn reference_dereferences_null(doc: &Value) -> bool {
    let is_str = |v: &Value| v.is_string();
    let at = |v: &Value, i: usize| {
        v.as_array()
            .and_then(|a| a.get(i))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let params = &doc["params"];
    match doc["method"].as_str() {
        Some("mining.notify") => {
            if !(0..4).all(|i| is_str(&at(params, i))) {
                return true;
            }
            let merkle = at(params, 4);
            let branches = merkle.as_array().cloned().unwrap_or_default();
            branches.len() <= 32
                && (!branches.iter().all(is_str) || !(5..8).all(|i| is_str(&at(params, i))))
        }
        Some("mining.set_version_mask") => !is_str(&at(params, 0)),
        _ => false,
    }
}

/// `hex2bin()` writes only as many bytes as the hex string has; the current
/// code zeroes the rest. Zero it in the reference result too, so only the
/// written bytes are compared.
fn zero_uninitialized(p: &mut Parsed, doc: &Value) {
    if !p.ok || p.m.mining_notification.is_null() {
        return;
    }
    // SAFETY: set by a successful parse.
    let n = unsafe { &mut *p.m.mining_notification };
    let written = |v: &Value| v.as_str().map_or(0, |s| s.len().div_ceil(2).min(32));
    let params = &doc["params"];
    n.prev_block_hash[written(&params[1])..].fill(0);
    for (i, branch) in n.merkle_branches[..n.n_merkle_branches]
        .iter_mut()
        .enumerate()
    {
        branch[written(&params[4][i])..].fill(0);
    }
}

/// Fixed (the golden file records the outputs for exactly these inputs).
fn iterations() -> (usize, u64) {
    (100_000, 0x5742_a7c0_de00_0001)
}

/// Bytes as recorded: the text, or for long ones length and digest.
fn shown(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    if b.len() <= 200 {
        format!("{:?}", String::from_utf8_lossy(b))
    } else {
        let d: String = Sha256::digest(b)
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        format!("<{} bytes, sha256 {d}..>", b.len())
    }
}

#[test]
fn parse_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let (mut compared, mut ok, mut notifies, mut skipped_null, mut extranonce_rejected) =
        (0, 0, 0, 0, 0);
    let mut rec = Rec::new("stratum_v1", "parse_matches_reference");

    for i in 0..n {
        let doc = message(&rng);
        let json = serde_json::to_string(&doc).unwrap();
        let new = parse_with(new_parse, &json);

        if reference_dereferences_null(&doc) {
            assert!(!new.ok, "case {i}: must be rejected: {json}");
            rec.add(format!("{i} rejected (undefined in C)"));
            skipped_null += 1;
            continue;
        }
        let mut reference = parse_with(ref_parse, &json);
        zero_uninitialized(&mut reference, &doc);

        // Deliberate difference: extranonce2_size outside 1..=16 is rejected.
        let extranonce_result = [
            method::STRATUM_RESULT_SUBSCRIBE,
            method::MINING_SET_EXTRANONCE,
        ]
        .contains(&reference.m.method);
        if reference.ok
            && extranonce_result
            && !EXTRANONCE2_LEN.contains(&reference.m.extranonce_2_len)
        {
            assert!(
                !new.ok,
                "case {i}: extranonce2_size must be rejected: {json}"
            );
            assert_eq!(new.m.method, reference.m.method);
            // recorded as the other cases (the golden test cannot tell them
            // apart); only the rejection and the method are compared with C
            rec.add(format!("{i} {}", summary(&new)));
            extranonce_rejected += 1;
            continue;
        }

        assert_eq!(summary(&new), summary(&reference), "case {i}: {json}");
        rec.add(format!("{i} {}", summary(&new)));
        compared += 1;
        ok += new.ok as usize;
        notifies += (new.ok && new.m.method == method::MINING_NOTIFY) as usize;
    }
    eprintln!(
        "{compared} identical ({ok} ok, {notifies} notifies), {skipped_null} skipped (reference dereferences NULL), \
         {extranonce_rejected} rejected extranonce2_size"
    );
    assert!(
        ok > compared / 5 && notifies > compared / 50,
        "generator coverage too low"
    );
    rec.finish();
}

/// Runs `request` against a fresh API object of each implementation and
/// returns what each sent.
fn sent_by(request: impl Fn(bool, *mut c_void, *mut c_void) -> bool) -> [(bool, Vec<u8>); 2] {
    [false, true].map(|current| {
        // SAFETY: objects created and freed here; stub_sent points into the stub.
        unsafe {
            let api = if current {
                new_api_new()
            } else {
                ref_api_new()
            };
            let t = stub_new();
            let ok = request(current, api, t);
            let mut len = 0;
            let p = stub_sent(t, &mut len);
            let sent = std::slice::from_raw_parts(p as *const u8, len).to_vec();
            stub_clear_sent(t);
            stub_free(t);
            if current {
                new_api_free(api)
            } else {
                ref_api_free(api)
            }
            (ok, sent)
        }
    })
}

#[test]
fn requests_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0xabcd));
    let mut rec = Rec::new("stratum_v1", "requests_match_reference");
    for i in 0..n / 10 {
        let c = |s: String| CString::new(s).unwrap();
        let (user, pass, job, en2) = (
            c(rng.text(80)),
            c(rng.text(40)),
            c(rng.hex(rng.below(8))),
            c(rng.hex(8)),
        );
        let (device, asic) = (c(rng.text(20)), c(rng.text(10)));
        let (a, b, d) = (
            rng.next() as u32,
            rng.next() as u32,
            rng.below(1 << 31) as u32,
        );
        let kind = rng.below(6);
        // Calls happen in sequence per object, so message IDs advance too.
        let [reference, current] = sent_by(|current, api, t| unsafe {
            let calls = 1 + (i % 3);
            let mut ok = true;
            for _ in 0..calls {
                ok &= match (kind, current) {
                    (0, false) => ref_subscribe(api, t, device.as_ptr(), asic.as_ptr()),
                    (0, true) => new_subscribe(api, t, device.as_ptr(), asic.as_ptr()),
                    (1, false) => ref_extranonce_subscribe(api, t),
                    (1, true) => new_extranonce_subscribe(api, t),
                    (2, false) => ref_suggest_difficulty(api, t, d),
                    (2, true) => new_suggest_difficulty(api, t, d),
                    (3, false) => ref_authorize(api, t, user.as_ptr(), pass.as_ptr()),
                    (3, true) => new_authorize(api, t, user.as_ptr(), pass.as_ptr()),
                    (4, false) => {
                        ref_submit(api, t, user.as_ptr(), job.as_ptr(), en2.as_ptr(), a, b, d)
                    }
                    (4, true) => {
                        new_submit(api, t, user.as_ptr(), job.as_ptr(), en2.as_ptr(), a, b, d)
                    }
                    (_, false) => ref_configure(api, t),
                    (_, true) => new_configure(api, t),
                };
            }
            ok
        });
        assert_eq!(
            current,
            reference,
            "case {i} kind {kind}: {:?}",
            String::from_utf8_lossy(&reference.1)
        );
        rec.add(format!("{i} {kind} {} {}", current.0, shown(&current.1)));
    }
    rec.finish();
}

#[test]
fn requests_escape_strings_and_reject_oversized() {
    let [_, (ok, sent)] = sent_by(|current, api, t| unsafe {
        let (user, pass) = (c"we\"ird\\user\n".as_ptr(), c"pa\"ss".as_ptr());
        if current {
            new_authorize(api, t, user, pass)
        } else {
            ref_authorize(api, t, user, pass)
        }
    });
    assert!(ok);
    let mut rec = Rec::new("stratum_v1", "requests_escape_strings_and_reject_oversized");
    rec.add(format!("{ok} {}", shown(&sent)));
    let v: Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(v["params"], json!(["we\"ird\\user\n", "pa\"ss"]));

    let long = CString::new("x".repeat(2000)).unwrap();
    let [(ref_ok, ref_sent), (ok, sent)] = sent_by(|current, api, t| unsafe {
        if current {
            new_authorize(api, t, long.as_ptr(), c"x".as_ptr())
        } else {
            ref_authorize(api, t, long.as_ptr(), c"x".as_ptr())
        }
    });
    // The reference sends it truncated to 1023 bytes; Rust refuses it.
    assert!(ref_ok && ref_sent.len() == 1023 && !ref_sent.ends_with(b"\n"));
    assert!(!ok && sent.is_empty());
    rec.add(format!("{ok} {}", shown(&sent)));
    rec.finish();
}

/// Feeds the same chunked byte stream to both and collects the lines until
/// each returns NULL (connection closed or buffer full).
fn receive_all(current: bool, chunks: &[Vec<u8>]) -> Vec<Option<Vec<u8>>> {
    // SAFETY: objects created and freed here; returned lines are malloc()ed.
    unsafe {
        let api = if current {
            new_api_new()
        } else {
            ref_api_new()
        };
        let t = stub_new();
        for c in chunks {
            stub_push(t, c.as_ptr() as _, c.len());
        }
        let mut lines = Vec::new();
        loop {
            let line = if current {
                new_receive(api, t)
            } else {
                ref_receive(api, t)
            };
            if line.is_null() {
                lines.push(None);
                break;
            }
            lines.push(Some(CStr::from_ptr(line).to_bytes().to_vec()));
            free(line as _);
        }
        stub_free(t);
        if current {
            new_api_free(api)
        } else {
            ref_api_free(api)
        }
        lines
    }
}

#[test]
fn line_framing_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x1234));
    let mut rec = Rec::new("stratum_v1", "line_framing_matches_reference");
    for i in 0..n / 100 {
        // Lines of JSON-ish text (no NUL bytes), sometimes very long.
        let mut stream = Vec::new();
        for _ in 0..rng.below(8) {
            let len = if rng.chance(5) {
                8000 + rng.below(12000)
            } else {
                rng.below(300)
            };
            stream.extend((0..len).map(|_| b' ' + rng.below(95) as u8));
            stream.extend(*rng.pick(&[&b"\n"[..], b"\r\n", b"\r\n", b"\n\n", b""]));
        }
        let mut chunks = Vec::new();
        let mut rest = &stream[..];
        while !rest.is_empty() {
            let n = (1 + rng.below(if rng.chance(20) { 20000 } else { 64 })).min(rest.len());
            chunks.push(rest[..n].to_vec());
            rest = &rest[n..];
        }
        let lines = receive_all(true, &chunks);
        assert_eq!(lines, receive_all(false, &chunks), "case {i}");
        let lines: Vec<String> = lines
            .iter()
            .map(|l| l.as_deref().map_or("<end>".into(), shown))
            .collect();
        rec.add(format!("{i}\n{}", lines.join("\n")));
    }
    rec.finish();
}

/// Session bookkeeping: without `mining.extranonce.subscribe`, the first share
/// gets ID 5. The reference treats its result as a setup response; the
/// current code matches it to the submit.
#[test]
fn share_result_with_setup_id() {
    let results = [false, true].map(|current| unsafe {
        let api = if current {
            new_api_new()
        } else {
            ref_api_new()
        };
        let t = stub_new();
        let (agent, user, job, en2) = (c"NerdQAxe+", c"bc1q.worker", c"6a1", c"00000001");
        if current {
            new_subscribe(api, t, agent.as_ptr(), c"BM1370".as_ptr());
            new_configure(api, t);
            new_authorize(api, t, user.as_ptr(), c"x".as_ptr());
            new_suggest_difficulty(api, t, 1000);
            new_submit(api, t, user.as_ptr(), job.as_ptr(), en2.as_ptr(), 1, 2, 3);
        } else {
            ref_subscribe(api, t, agent.as_ptr(), c"BM1370".as_ptr());
            ref_configure(api, t);
            ref_authorize(api, t, user.as_ptr(), c"x".as_ptr());
            ref_suggest_difficulty(api, t, 1000);
            ref_submit(api, t, user.as_ptr(), job.as_ptr(), en2.as_ptr(), 1, 2, 3);
        }
        let mut len = 0;
        let sent = std::slice::from_raw_parts(stub_sent(t, &mut len) as *const u8, len).to_vec();
        let sent = String::from_utf8(sent).unwrap();
        assert!(sent.contains("{\"id\": 5, \"method\": \"mining.submit\""));

        let json = c"{\"id\":5,\"result\":true,\"error\":null}";
        let mut m = Message::zeroed();
        let ok = if current {
            new_api_parse(api, json.as_ptr(), &mut m)
        } else {
            ref_api_parse(api, json.as_ptr(), &mut m)
        };
        stub_free(t);
        if current {
            new_api_free(api)
        } else {
            ref_api_free(api)
        }
        (ok, m.method, m.response_success, sent)
    });
    let results = results.map(|(ok, method, success, sent)| ((ok, method, success), sent));
    assert_eq!(
        results[0].0,
        (true, method::STRATUM_RESULT_SETUP, true),
        "former code"
    );
    assert_eq!(
        results[1].0,
        (true, method::STRATUM_RESULT, true),
        "current code"
    );
    // the requests the same
    assert_eq!(results[1].1, results[0].1);
    let mut rec = Rec::new("stratum_v1", "share_result_with_setup_id");
    rec.add(&results[1].1);
    rec.add(format!("{:?}", results[1].0));
    rec.finish();
}

/// `doc` in the looser syntax ArduinoJson reads as the same document:
/// unquoted or single-quoted keys, single-quoted strings, `+5` and `.5`,
/// tabs between tokens, text after the object.
fn lenient(v: &Value, rng: &Rng) -> String {
    let space = |rng: &Rng| ["", "", " ", "\t", " \t "][rng.below(5)].to_string();
    match v {
        Value::Object(map) => {
            let members: Vec<String> = map
                .iter()
                .map(|(k, v)| {
                    let plain =
                        !k.is_empty() && k.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_');
                    let key = match rng.below(3) {
                        0 if plain => k.clone(),
                        1 if !k.contains('\'') && !k.contains('\\') => format!("'{k}'"),
                        _ => serde_json::to_string(k).unwrap(),
                    };
                    format!(
                        "{}{key}{}:{}{}",
                        space(rng),
                        space(rng),
                        space(rng),
                        lenient(v, rng)
                    )
                })
                .collect();
            format!("{{{}{}}}", members.join(","), space(rng))
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(|v| lenient(v, rng)).collect();
            format!(
                "[{}{}]",
                items.join(&format!("{},", space(rng))),
                space(rng)
            )
        }
        Value::String(s) => {
            let quoted = serde_json::to_string(s).unwrap();
            let inner = &quoted[1..quoted.len() - 1];
            if rng.below(2) == 0 && !inner.contains('\'') {
                format!("'{inner}'")
            } else {
                quoted
            }
        }
        Value::Number(_) => {
            let n = v.to_string();
            match rng.below(3) {
                0 if n.starts_with("0.") => n[1..].to_string(),
                1 if n.starts_with(|c: char| c.is_ascii_digit()) => format!("+{n}"),
                _ => n,
            }
        }
        _ => v.to_string(),
    }
}

/// ArduinoJson read pool lines before step 1 and reads them again since the
/// switch to rust_core::ajson; serde_json (steps 1 to H2) rejected these.
#[test]
fn lenient_syntax_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x1e41));
    let mut compared = 0;
    let mut rec = Rec::new("stratum_v1", "lenient_syntax_matches_reference");
    for i in 0..n / 2 {
        let doc = message(&rng);
        if reference_dereferences_null(&doc) {
            continue;
        }
        let strict = serde_json::to_string(&doc).unwrap();
        let mut loose = lenient(&doc, &rng);
        // text after the value is ignored, except after a top-level number
        if rng.below(4) == 0 && !doc.is_number() {
            loose.push_str(["", " ", " x", "}", " {\"id\":9}"][rng.below(5)]);
        }
        let new = parse_with(new_parse, &loose);
        let mut reference = parse_with(ref_parse, &loose);
        zero_uninitialized(&mut reference, &doc);
        let extranonce_result = [
            method::STRATUM_RESULT_SUBSCRIBE,
            method::MINING_SET_EXTRANONCE,
        ]
        .contains(&reference.m.method);
        if reference.ok
            && extranonce_result
            && !EXTRANONCE2_LEN.contains(&reference.m.extranonce_2_len)
        {
            // deliberate difference, see parse_matches_reference; recorded
            // as the other cases (the golden test cannot tell them apart)
            assert!(!new.ok, "case {i}: {loose}");
            assert_eq!(
                summary(&new),
                summary(&parse_with(new_parse, &strict)),
                "case {i}: {loose}"
            );
            rec.add(format!("{i} {}", summary(&new)));
            continue;
        }
        assert_eq!(summary(&new), summary(&reference), "case {i}: {loose}");
        // the same document as the strict text
        assert_eq!(
            summary(&new),
            summary(&parse_with(new_parse, &strict)),
            "case {i}: {loose}"
        );
        rec.add(format!("{i} {}", summary(&new)));
        compared += 1;
    }
    assert!(compared > n / 4, "coverage too low: {compared}");
    rec.finish();
}
