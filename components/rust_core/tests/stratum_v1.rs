//! Golden tests of the Stratum v1 codec (`session::Session`, `lines`,
//! `connection`): parsing of random and mutated pool messages, request
//! formatting and line framing, against the outputs of the former C++/
//! ArduinoJson `StratumApi` (tests/golden/stratum_v1.txt, recorded by the
//! differential tests).

use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::Mutex;

use rust_core::stratum_v1::connection::{receive_line, send_tracked, Io, Transfer};
use rust_core::stratum_v1::lines::LineBuffer;
use rust_core::stratum_v1::session::Session;
use rust_core::stratum_v1::{method, Message};
use serde_json::{json, Value};

mod golden;
use golden::Rec;

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

/// A parse result.
struct Parsed {
    ok: bool,
    m: Message,
}

/// Parsing without requests sent: responses fall back to the fixed ID rule.
fn parse_new(json: &str) -> Parsed {
    let mut m = Message::default();
    let ok = Session::default().parse(json.as_bytes(), &mut m);
    Parsed { ok, m }
}

fn text(s: &[u8]) -> String {
    String::from_utf8_lossy(s).into_owned()
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
        m.extranonce_str.as_deref().map(text),
        m.should_abandon_work as i32,
        m.new_difficulty,
        m.version_mask,
        m.response_success
    );
    if let Some(n) = m.mining_notification.as_ref().filter(|_| p.ok) {
        s += &format!(
            " notify: job={:?} prev={:02x?} cb1={:?} cb2={:?} merkle={:02x?} version={:#x} nbits={:#x} ntime={:#x}",
            Some(text(&n.job_id)),
            n.prev_block_hash,
            Some(text(&n.coinbase_1)),
            Some(text(&n.coinbase_2)),
            &n.merkle_branches[..],
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
        let d = rust_core::cutil::to_hex(&Sha256::digest(b)[..8]);
        format!("<{} bytes, sha256 {d}..>", b.len())
    }
}

#[test]
fn parse_matches_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let (mut compared, mut ok, mut notifies, mut skipped_null) = (0, 0, 0, 0);
    let mut rec = Rec::new("stratum_v1", "parse_matches_reference");

    for i in 0..n {
        let doc = message(&rng);
        let json = serde_json::to_string(&doc).unwrap();
        let new = parse_new(&json);

        if reference_dereferences_null(&doc) {
            assert!(!new.ok, "case {i}: must be rejected: {json}");
            rec.add(format!("{i} rejected (undefined in C)"));
            skipped_null += 1;
            continue;
        }
        rec.add(format!("{i} {}", summary(&new)));
        compared += 1;
        ok += new.ok as usize;
        notifies += (new.ok && new.m.method == method::MINING_NOTIFY) as usize;
    }
    eprintln!("{compared} compared ({ok} ok, {notifies} notifies), {skipped_null} rejected");
    assert!(
        ok > compared / 5 && notifies > compared / 50,
        "generator coverage too low"
    );
    rec.finish();
}

/// The transport stub: recv returns the queued chunks, then 0 (connection
/// closed); send records the bytes.
#[derive(Default)]
struct Stub {
    chunks: VecDeque<Vec<u8>>,
    sent: Vec<u8>,
}

impl Io for Stub {
    fn send(&mut self, data: &[u8]) -> Transfer {
        self.sent.extend_from_slice(data);
        Transfer::Bytes(data.len())
    }
    fn recv(&mut self, buf: &mut [u8]) -> Transfer {
        let Some(c) = self.chunks.front_mut() else {
            return Transfer::Zero;
        };
        let n = c.len().min(buf.len());
        buf[..n].copy_from_slice(&c[..n]);
        c.drain(..n);
        if c.is_empty() {
            self.chunks.pop_front();
        }
        Transfer::Bytes(n)
    }
    fn is_connected(&mut self) -> bool {
        true
    }
    fn delay_ms(&mut self, _ms: u32) {}
}

/// `esp_app_get_description()->version` of the host build
const VERSION: &[u8] = b"v-test";

/// One API object as the firmware uses it (requests formatted into a 1024
/// byte buffer, so at most 1023 bytes are sent).
#[derive(Default)]
struct Api {
    session: Mutex<Session>,
    lines: LineBuffer,
}

impl Api {
    /// A request that does not fit the 1024 byte request buffer (with its
    /// NUL) is neither sent nor left pending.
    fn send(&self, t: &mut Stub, f: impl FnOnce(&mut Session) -> Vec<u8>) -> bool {
        send_tracked(t, &self.session, f)
    }
    fn subscribe(&self, t: &mut Stub, device: &[u8], asic: &[u8]) -> bool {
        self.send(t, |s| s.subscribe(device, asic, VERSION))
    }
    fn extranonce_subscribe(&self, t: &mut Stub) -> bool {
        self.send(t, Session::extranonce_subscribe)
    }
    fn suggest_difficulty(&self, t: &mut Stub, d: u32) -> bool {
        self.send(t, |s| s.suggest_difficulty(d))
    }
    fn authorize(&self, t: &mut Stub, user: &[u8], pass: &[u8]) -> bool {
        self.send(t, |s| s.authorize(user, pass))
    }
    #[allow(clippy::too_many_arguments)]
    fn submit(
        &self,
        t: &mut Stub,
        user: &[u8],
        job: &[u8],
        en2: &[u8],
        ntime: u32,
        nonce: u32,
        version: u32,
    ) -> bool {
        self.send(t, |s| s.submit(user, job, en2, ntime, nonce, version))
    }
    fn configure(&self, t: &mut Stub) -> bool {
        self.send(t, Session::configure_version_rolling)
    }
    fn parse(&self, json: &str, m: &mut Message) -> bool {
        self.session.lock().unwrap().parse(json.as_bytes(), m)
    }
}

/// Runs `request` against a fresh API object and returns what it sent.
fn sent_by(request: impl Fn(&Api, &mut Stub) -> bool) -> (bool, Vec<u8>) {
    let api = Api::default();
    let mut t = Stub::default();
    let ok = request(&api, &mut t);
    (ok, t.sent)
}

#[test]
fn requests_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0xabcd));
    let mut rec = Rec::new("stratum_v1", "requests_match_reference");
    for i in 0..n / 10 {
        let (user, pass, job, en2) = (
            rng.text(80),
            rng.text(40),
            rng.hex(rng.below(8)),
            rng.hex(8),
        );
        let (device, asic) = (rng.text(20), rng.text(10));
        let (a, b, d) = (
            rng.next() as u32,
            rng.next() as u32,
            rng.below(1 << 31) as u32,
        );
        let kind = rng.below(6);
        // Calls happen in sequence per object, so message IDs advance too.
        let current = sent_by(|api, t| {
            let calls = 1 + (i % 3);
            let mut ok = true;
            for _ in 0..calls {
                ok &= match kind {
                    0 => api.subscribe(t, device.as_bytes(), asic.as_bytes()),
                    1 => api.extranonce_subscribe(t),
                    2 => api.suggest_difficulty(t, d),
                    3 => api.authorize(t, user.as_bytes(), pass.as_bytes()),
                    4 => api.submit(t, user.as_bytes(), job.as_bytes(), en2.as_bytes(), a, b, d),
                    _ => api.configure(t),
                };
            }
            ok
        });
        rec.add(format!("{i} {kind} {} {}", current.0, shown(&current.1)));
    }
    rec.finish();
}

#[test]
fn requests_escape_strings_and_reject_oversized() {
    let (ok, sent) = sent_by(|api, t| api.authorize(t, b"we\"ird\\user\n", b"pa\"ss"));
    assert!(ok);
    let mut rec = Rec::new("stratum_v1", "requests_escape_strings_and_reject_oversized");
    rec.add(format!("{ok} {}", shown(&sent)));
    let v: Value = serde_json::from_slice(&sent).unwrap();
    assert_eq!(v["params"], json!(["we\"ird\\user\n", "pa\"ss"]));

    let long = "x".repeat(2000);
    let (ok, sent) = sent_by(|api, t| api.authorize(t, long.as_bytes(), b"x"));
    // the C++ code sent it truncated to 1023 bytes; Rust refuses it
    assert!(!ok && sent.is_empty());
    rec.add(format!("{ok} {}", shown(&sent)));
    rec.finish();
}

/// Feeds the chunked byte stream and collects the lines until the receive
/// fails (connection closed or buffer full).
fn receive_all(chunks: &[Vec<u8>]) -> Vec<Option<Vec<u8>>> {
    let mut api = Api::default();
    let mut t = Stub {
        chunks: chunks.iter().cloned().collect(),
        ..Stub::default()
    };
    let mut lines = Vec::new();
    loop {
        match receive_line(&mut t, &mut api.lines, || false) {
            Ok(line) => {
                // as the C string the firmware gets
                let end = line.iter().position(|&b| b == 0).unwrap_or(line.len());
                lines.push(Some(line[..end].to_vec()));
            }
            Err(_) => {
                lines.push(None);
                break;
            }
        }
    }
    lines
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
        let lines = receive_all(&chunks);
        let lines: Vec<String> = lines
            .iter()
            .map(|l| l.as_deref().map_or("<end>".into(), shown))
            .collect();
        rec.add(format!("{i}\n{}", lines.join("\n")));
    }
    rec.finish();
}

/// Session bookkeeping: without `mining.extranonce.subscribe`, the first share
/// gets ID 5. The former code treated its result as a setup response; the
/// current code matches it to the submit.
#[test]
fn share_result_with_setup_id() {
    let api = Api::default();
    let mut t = Stub::default();
    let (user, job, en2) = (&b"bc1q.worker"[..], &b"6a1"[..], &b"00000001"[..]);
    api.subscribe(&mut t, b"NerdQAxe+", b"BM1370");
    api.configure(&mut t);
    api.authorize(&mut t, user, b"x");
    api.suggest_difficulty(&mut t, 1000);
    api.submit(&mut t, user, job, en2, 1, 2, 3);
    let sent = String::from_utf8(t.sent.clone()).unwrap();
    assert!(sent.contains("{\"id\": 5, \"method\": \"mining.submit\""));

    let mut m = Message::default();
    let ok = api.parse("{\"id\":5,\"result\":true,\"error\":null}", &mut m);
    let result = (ok, m.method, m.response_success);
    assert_eq!(result, (true, method::STRATUM_RESULT, true));
    let mut rec = Rec::new("stratum_v1", "share_result_with_setup_id");
    rec.add(&sent);
    rec.add(format!("{result:?}"));
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
        let new = parse_new(&loose);
        // the same document as the strict text
        assert_eq!(
            summary(&new),
            summary(&parse_new(&strict)),
            "case {i}: {loose}"
        );
        rec.add(format!("{i} {}", summary(&new)));
        compared += 1;
    }
    assert!(compared > n / 4, "coverage too low: {compared}");
    rec.finish();
}
