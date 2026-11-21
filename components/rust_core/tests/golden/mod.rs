//! Golden files: the outputs of the former C/C++ code for the inputs of a
//! test, recorded once by the differential tests (when the C reference
//! still existed) and compared against the Rust outputs since.
//!
//! The tests run thousands of rounds, so a fixture keeps per test the
//! number of outputs, a SHA-256 digest over all of them and the first few
//! in full (to see what changed). `GOLDEN_BLESS=1` rewrites the fixtures;
//! only for an intended change of behaviour, and the diff of the fixture
//! belongs to the review.
//!
//! Fixture: `tests/golden/<suite>.txt`, one section per test.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::Mutex;

use sha2::{Digest, Sha256};

/// outputs kept in full
const SAMPLES: usize = 8;

/// one fixture file at a time (the tests of a suite run in parallel)
static FILE: Mutex<()> = Mutex::new(());

fn dir() -> PathBuf {
    let m = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // included by the differential tests (c-reference/) and by rust_core's
    if m.ends_with("c-reference") {
        m.join("../tests/golden")
    } else {
        m.join("tests/golden")
    }
}

/// The outputs of one test.
pub struct Rec {
    suite: &'static str,
    test: String,
    n: u64,
    hash: Sha256,
    samples: Vec<String>,
}

impl Rec {
    pub fn new(suite: &'static str, test: impl Into<String>) -> Rec {
        Rec {
            suite,
            test: test.into(),
            n: 0,
            hash: Sha256::new(),
            samples: Vec::new(),
        }
    }

    /// One output (a trace, a value as text, ...).
    pub fn add(&mut self, out: impl AsRef<str>) {
        let out = out.as_ref();
        self.n += 1;
        self.hash.update((out.len() as u64).to_le_bytes());
        self.hash.update(out.as_bytes());
        if self.samples.len() < SAMPLES {
            self.samples.push(out.to_string());
        }
    }

    /// Several values of one round, by `{:?}`.
    pub fn add_dbg(&mut self, out: impl std::fmt::Debug) {
        self.add(format!("{out:?}"));
    }

    fn section(&self, digest: &str) -> String {
        let mut s = format!("count {}\nsha256 {digest}\n", self.n);
        for (i, sample) in self.samples.iter().enumerate() {
            let _ = writeln!(s, "--- sample {i}");
            for line in sample.split('\n') {
                let _ = writeln!(s, "| {line}");
            }
        }
        s
    }

    /// Compares with the fixture (or writes it with `GOLDEN_BLESS=1`).
    pub fn finish(self) {
        let digest = self
            .hash
            .clone()
            .finalize()
            .iter()
            .fold(String::new(), |mut s, b| {
                let _ = write!(s, "{b:02x}");
                s
            });
        let section = self.section(&digest);
        let _g = FILE.lock().unwrap_or_else(|e| e.into_inner());
        let path = dir().join(format!("{}.txt", self.suite));
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        let mut sections = parse(&text);
        if std::env::var_os("GOLDEN_BLESS").is_some() {
            sections.insert(self.test.clone(), section);
            std::fs::write(&path, render(self.suite, &sections)).expect("write the fixture");
            return;
        }
        let Some(want) = sections.get(&self.test) else {
            panic!(
                "no golden output for {}/{} in {} (GOLDEN_BLESS=1 records it)",
                self.suite,
                self.test,
                path.display()
            );
        };
        if *want != section {
            panic!(
                "{}/{}: the outputs differ from the golden file {}\n\
                 --- golden\n{want}--- now\n{section}",
                self.suite,
                self.test,
                path.display()
            );
        }
    }
}

const HEADER: &str = "# Golden outputs (the former C/C++ code), see tests/golden/mod.rs\n";

fn parse(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    let mut cur: Option<(String, String)> = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("=== ") {
            if let Some((k, v)) = cur.take() {
                map.insert(k, v);
            }
            cur = Some((name.to_string(), String::new()));
        } else if let Some((_, v)) = cur.as_mut() {
            v.push_str(line);
            v.push('\n');
        }
    }
    if let Some((k, v)) = cur {
        map.insert(k, v);
    }
    map
}

fn render(_suite: &str, sections: &BTreeMap<String, String>) -> String {
    let mut s = String::from(HEADER);
    for (k, v) in sections {
        let _ = write!(s, "=== {k}\n{v}");
    }
    s
}
