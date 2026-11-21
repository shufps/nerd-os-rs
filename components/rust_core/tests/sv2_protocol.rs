//! Golden tests of the Stratum v2 codec (`stratum_v2::protocol`, behind
//! the `sv2_*` C API): builders on random arguments and buffer sizes,
//! parsers on random, well-formed and mutated payloads, against the outputs
//! of the former `sv2_protocol.c` (tests/golden/sv2_protocol.txt, recorded
//! by the differential tests).

use std::cell::Cell;
use std::ffi::CString;

use rust_core::stratum_v2::protocol::*;

mod golden;
use golden::Rec;

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
    fn bytes(&self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn u32(&self) -> u32 {
        match self.below(4) {
            0 => self.below(4) as u32,
            1 => u32::MAX - self.below(4) as u32,
            _ => self.next() as u32,
        }
    }
    /// A C string (no NUL), sometimes longer than STR0_255 allows.
    fn cstring(&self) -> CString {
        let n = if self.chance(10) {
            200 + self.below(150)
        } else {
            self.below(40)
        };
        CString::new(
            (0..n)
                .map(|_| 1 + self.below(255) as u8)
                .collect::<Vec<_>>(),
        )
        .unwrap()
    }
}

/// Fixed (the golden file records the outputs for exactly these inputs).
fn iterations() -> (usize, u64) {
    (100_000, 0x05f2_c0de_c0de_0004)
}

fn hex(b: &[u8]) -> String {
    rust_core::cutil::to_hex(b)
}

/// The fields of a parsed extended job as compared.
type ExtFields = (
    (u32, u32, bool, [u8; 32], u32, u32, bool),
    Vec<[u8; 32]>,
    bool,
    (bool, Vec<u8>),
    (bool, Vec<u8>),
    ([u8; 32], u8),
);

fn ext_fields_text(f: &ExtFields) -> String {
    let merkle: Vec<String> = f.1.iter().map(|h| hex(h)).collect();
    format!(
        "{:?} merkle=[{}] rest_zero={} prefix={}:{} suffix={}:{} en={}:{}",
        f.0,
        merkle.join(","),
        f.2,
        f.3 .0,
        hex(&f.3 .1),
        f.4 .0,
        hex(&f.4 .1),
        hex(&f.5 .0),
        f.5 .1
    )
}

// ---- the C API's output conventions (src/stratum_v2/mod.rs, ffi) ----

/// A B0_32 prefix copied into a 32-byte buffer plus its length.
fn prefix_buf(src: &[u8]) -> ([u8; 32], u8) {
    let mut out = [0u8; 32];
    out[..src.len()].copy_from_slice(src);
    (out, src.len() as u8)
}

/// The frame copied into a buffer of `buf_len` bytes: its length, or -1 if
/// it does not fit.
fn emit(buf: &mut [u8], buf_len: usize, frame: Option<Vec<u8>>) -> i32 {
    match frame {
        Some(f) if f.len() <= buf_len => {
            buf[..f.len()].copy_from_slice(&f);
            f.len() as i32
        }
        _ => -1,
    }
}

// ---- payload generators: well-formed, then mutated ----

fn le32(v: u32) -> [u8; 4] {
    v.to_le_bytes()
}

fn b0_32(rng: &Rng, out: &mut Vec<u8>) {
    let n = if rng.chance(10) {
        33 + rng.below(10)
    } else {
        rng.below(33)
    };
    out.push(n as u8);
    out.extend(rng.bytes(n));
}

fn payload(rng: &Rng, kind: usize) -> Vec<u8> {
    let mut p = Vec::new();
    match kind {
        // OpenStandard/ExtendedMiningChannel.Success
        0 | 1 => {
            p.extend(le32(rng.u32()));
            p.extend(le32(rng.u32()));
            p.extend(rng.bytes(32));
            if kind == 1 {
                p.extend((rng.below(40) as u16).to_le_bytes());
            }
            b0_32(rng, &mut p);
            p.extend(le32(rng.u32()));
        }
        // NewMiningJob
        2 => {
            p.extend(le32(rng.u32()));
            p.extend(le32(rng.u32()));
            let flag = *[0u8, 1, 1, 2].get(rng.below(4)).unwrap();
            p.push(flag);
            if flag == 1 || rng.chance(10) {
                p.extend(le32(rng.u32()));
            }
            p.extend(le32(rng.u32()));
            p.extend(rng.bytes(32));
        }
        // NewExtendedMiningJob
        3 => {
            p.extend(le32(rng.u32()));
            p.extend(le32(rng.u32()));
            let flag = *[0u8, 1, 1, 7].get(rng.below(4)).unwrap();
            p.push(flag);
            if flag == 1 {
                p.extend(le32(rng.u32()));
            }
            p.extend(le32(rng.u32()));
            p.push(rng.below(3) as u8);
            let count = if rng.chance(10) {
                21 + rng.below(5)
            } else {
                rng.below(15)
            };
            p.push(count as u8);
            p.extend(rng.bytes(32 * count));
            for _ in 0..2 {
                let n = rng.below(if rng.chance(5) { 2000 } else { 200 });
                p.extend((n as u16).to_le_bytes());
                p.extend(rng.bytes(n));
            }
        }
        // SubmitShares.Error
        4 => {
            p.extend(le32(rng.u32()));
            p.extend(le32(rng.u32()));
            let n = rng.below(120);
            p.push(n as u8);
            p.extend(rng.bytes(n));
        }
        // SetExtranoncePrefix
        5 => {
            p.extend(le32(rng.u32()));
            b0_32(rng, &mut p);
        }
        // fixed-size messages: just random bytes of plausible length
        _ => p.extend(rng.bytes(rng.below(60))),
    }
    // mutations
    match rng.below(10) {
        0 => {
            let n = rng.below(p.len() + 1);
            p.truncate(n);
        }
        1 if !p.is_empty() => {
            let i = rng.below(p.len());
            p[i] = rng.next() as u8;
        }
        2 => p.extend(rng.bytes(rng.below(8))),
        _ => {}
    }
    p
}

#[test]
fn parsers_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let (mut ok, mut total) = (0usize, 0usize);
    let mut rec = Rec::new("sv2_protocol", "parsers_match_reference");
    for i in 0..n {
        let kind = rng.below(11);
        let p = payload(&rng, kind);
        total += 1;
        match kind {
            0 | 1 => {
                let parsed = open_channel_success(&p, kind == 1);
                let rb = if parsed.is_some() { 0 } else { -1 };
                rec.add(if let Some(s) = parsed {
                    let (prefix, prefix_len) = prefix_buf(s.extranonce_prefix);
                    let b = (
                        s.request_id,
                        s.channel_id,
                        s.target,
                        s.extranonce_size,
                        prefix,
                        prefix_len,
                        s.group_channel_id,
                    );
                    ok += 1;
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} {rb}")
                });
            }
            2 => {
                rec.add(if let Some(j) = new_mining_job(&p) {
                    let b = (
                        j.channel_id,
                        j.job_id,
                        j.min_ntime.is_some(),
                        j.min_ntime.unwrap_or(0),
                        j.version,
                        j.merkle_root,
                    );
                    ok += 1;
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            3 => {
                let parsed = new_extended_mining_job(&p);
                let cb = match &parsed {
                    Ok(j) => j.channel_id,
                    Err(id) => id.unwrap_or(0xdead),
                };
                match parsed {
                    Ok(j) => {
                        let fields: ExtFields = (
                            (
                                j.job_id,
                                j.version,
                                j.version_rolling_allowed,
                                [0; 32],
                                j.min_ntime.unwrap_or(0),
                                0,
                                j.min_ntime.is_some(),
                            ),
                            j.merkle_path.clone(),
                            true,
                            (j.coinbase_prefix.is_empty(), j.coinbase_prefix.to_vec()),
                            (j.coinbase_suffix.is_empty(), j.coinbase_suffix.to_vec()),
                            ([0; 32], 0),
                        );
                        rec.add(format!("{i} 3 {cb:#x} {}", ext_fields_text(&fields)));
                        ok += 1;
                    }
                    Err(_) => rec.add(format!("{i} 3 {cb:#x} null")),
                }
            }
            4 => {
                let size = 1 + rng.below(80);
                rec.add(if let Some((ch, seq, code)) = submit_shares_error(&p) {
                    // copied NUL-terminated, truncated to size - 1, into a
                    // buffer of 81 bytes 0x55
                    let mut buf = vec![0x55u8; 81];
                    let n = code.len().min(size - 1);
                    buf[..n].copy_from_slice(&code[..n]);
                    buf[n] = 0;
                    ok += 1;
                    format!("{i} {kind} 0 {ch} {seq} {size} {}", hex(&buf))
                } else {
                    format!("{i} {kind} -1")
                });
            }
            5 => {
                rec.add(if let Some((ch, prefix)) = set_extranonce_prefix(&p) {
                    let (prefix, len) = prefix_buf(prefix);
                    let b = (ch, prefix, len);
                    ok += 1;
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            6 => {
                rec.add(if let Some(b) = setup_connection_success(&p) {
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            7 => {
                rec.add(if let Some(b) = set_new_prev_hash(&p) {
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            8 => {
                rec.add(if let Some(b) = set_target(&p) {
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            9 => {
                rec.add(if let Some(b) = submit_shares_success(&p) {
                    format!("{i} {kind} 0 {b:?}")
                } else {
                    format!("{i} {kind} -1")
                });
            }
            _ => {
                let mut t = [0u8; 32];
                let lead = rng.below(33);
                for (j, b) in t.iter_mut().enumerate() {
                    *b = if j >= 32 - lead { 0 } else { rng.next() as u8 };
                }
                let pdiff = target_to_pdiff(&t);
                let h = rng.bytes(6);
                let hb = parse_frame_header(&h.clone().try_into().unwrap());
                rec.add(format!("{i} {kind} {pdiff} {hb:?}"));
            }
        }
    }
    eprintln!("{total} payloads, {ok} parsed successfully");
    assert!(ok > total / 5, "generator coverage too low");
    rec.finish();
}

#[test]
fn builders_match_reference() {
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 0x77));
    let mut rec = Rec::new("sv2_protocol", "builders_match_reference");
    for i in 0..n / 2 {
        let buf_len = if rng.chance(20) { rng.below(80) } else { 1200 };
        let mut b = vec![0xaau8; 1200];
        let (u1, u2, u3, u4, u5, u6) = (
            rng.u32(),
            rng.u32(),
            rng.u32(),
            rng.u32(),
            rng.u32(),
            rng.u32(),
        );
        let rate = f32::from_bits(rng.next() as u32);
        let kind = rng.below(6);
        let rb = match kind {
            0 => {
                let s: Vec<CString> = (0..5).map(|_| rng.cstring()).collect();
                let port = rng.next() as u16;
                let frame = setup_connection(
                    s[0].as_bytes(),
                    port,
                    s[1].as_bytes(),
                    s[2].as_bytes(),
                    s[3].as_bytes(),
                    s[4].as_bytes(),
                    u1,
                );
                emit(&mut b, buf_len, frame)
            }
            1 => {
                let user = rng.cstring();
                let frame = open_standard_mining_channel(u1, user.as_bytes(), rate);
                emit(&mut b, buf_len, Some(frame))
            }
            2 => {
                let user = rng.cstring();
                let min_en = rng.next() as u16;
                let frame = open_extended_mining_channel(u1, user.as_bytes(), rate, min_en);
                emit(&mut b, buf_len, Some(frame))
            }
            3 => {
                let frame = submit_shares_standard(u1, u2, u3, u4, u5, u6);
                emit(&mut b, buf_len, Some(frame))
            }
            4 => {
                let en = rng.bytes(rng.below(256));
                // the length goes through a u8 in the C API
                let en = &en[..en.len() as u8 as usize];
                let frame = submit_shares_extended(u1, u2, u3, u4, u5, u6, en);
                emit(&mut b, buf_len, Some(frame))
            }
            _ => {
                let (ext, ty, len) = (rng.next() as u16, rng.next() as u8, rng.next() as u32);
                b[..FRAME_HEADER_SIZE].copy_from_slice(&encode_frame_header(ext, ty, len));
                FRAME_HEADER_SIZE as i32
            }
        };
        rec.add(format!("{i} {kind} {rb} {}", hex(&b[..rb.max(0) as usize])));
    }
    rec.finish();
}
