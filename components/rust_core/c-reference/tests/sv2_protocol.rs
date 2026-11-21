//! Compares the Rust Stratum v2 codec with the former `sv2_protocol.c`
//! (compiled for the host with every function renamed to `ref_sv2_*`), through
//! the same C API: builders on random arguments and buffer sizes, parsers on
//! random, well-formed and mutated payloads.
//!
//! `DIFF_ITERATIONS` (default 100000) and `DIFF_SEED` override the run.

use std::cell::Cell;
use std::ffi::{c_char, c_int, CString};

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::stratum_v2::protocol::FrameHeader;
use rust_core::stratum_v2::ExtJob;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

// The reference symbols carry a ref_ prefix; declare them separately.
extern "C" {
    fn ref_sv2_parse_frame_header(data: *const u8, h: *mut FrameHeader) -> c_int;
    fn ref_sv2_encode_frame_header(dest: *mut u8, ext: u16, msg_type: u8, len: u32) -> c_int;
    fn ref_sv2_build_setup_connection(
        buf: *mut u8,
        buf_len: usize,
        host: *const c_char,
        port: u16,
        vendor: *const c_char,
        hw: *const c_char,
        fw: *const c_char,
        dev: *const c_char,
        flags: u32,
    ) -> c_int;
    fn ref_sv2_build_open_standard_mining_channel(
        buf: *mut u8,
        buf_len: usize,
        req: u32,
        user: *const c_char,
        rate: f32,
    ) -> c_int;
    fn ref_sv2_build_open_extended_mining_channel(
        buf: *mut u8,
        buf_len: usize,
        req: u32,
        user: *const c_char,
        rate: f32,
        min_en: u16,
    ) -> c_int;
    fn ref_sv2_build_submit_shares_standard(
        buf: *mut u8,
        buf_len: usize,
        ch: u32,
        seq: u32,
        job: u32,
        nonce: u32,
        ntime: u32,
        version: u32,
    ) -> c_int;
    fn ref_sv2_build_submit_shares_extended(
        buf: *mut u8,
        buf_len: usize,
        ch: u32,
        seq: u32,
        job: u32,
        nonce: u32,
        ntime: u32,
        version: u32,
        en: *const u8,
        en_len: u8,
    ) -> c_int;
    fn ref_sv2_parse_setup_connection_success(
        p: *const u8,
        len: u32,
        v: *mut u16,
        flags: *mut u32,
    ) -> c_int;
    fn ref_sv2_parse_open_channel_success(
        p: *const u8,
        len: u32,
        req: *mut u32,
        ch: *mut u32,
        target: *mut u8,
        prefix: *mut u8,
        prefix_len: *mut u8,
        group: *mut u32,
    ) -> c_int;
    fn ref_sv2_parse_open_extended_channel_success(
        p: *const u8,
        len: u32,
        req: *mut u32,
        ch: *mut u32,
        target: *mut u8,
        en_size: *mut u16,
        prefix: *mut u8,
        prefix_len: *mut u8,
        group: *mut u32,
    ) -> c_int;
    fn ref_sv2_parse_new_mining_job(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        job: *mut u32,
        has: *mut bool,
        min_ntime: *mut u32,
        version: *mut u32,
        merkle: *mut u8,
    ) -> c_int;
    fn ref_sv2_parse_set_new_prev_hash(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        job: *mut u32,
        prev: *mut u8,
        min_ntime: *mut u32,
        nbits: *mut u32,
    ) -> c_int;
    fn ref_sv2_parse_set_extranonce_prefix(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        prefix: *mut u8,
        prefix_len: *mut u8,
    ) -> c_int;
    fn ref_sv2_parse_set_target(p: *const u8, len: u32, ch: *mut u32, target: *mut u8) -> c_int;
    fn ref_sv2_parse_submit_shares_success(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        accepted: *mut u32,
    ) -> c_int;
    fn ref_sv2_parse_submit_shares_error(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        seq: *mut u32,
        code: *mut c_char,
        code_size: usize,
    ) -> c_int;
    fn ref_sv2_parse_new_extended_mining_job(p: *const u8, len: u32, ch: *mut u32) -> *mut ExtJob;
    fn ref_sv2_ext_job_free(job: *mut ExtJob);
    fn ref_sv2_target_to_pdiff(target: *const u8) -> u32;

    fn sv2_parse_frame_header(data: *const u8, h: *mut FrameHeader) -> c_int;
    fn sv2_encode_frame_header(dest: *mut u8, ext: u16, msg_type: u8, len: u32) -> c_int;
    fn sv2_build_setup_connection(
        buf: *mut u8,
        buf_len: usize,
        host: *const c_char,
        port: u16,
        vendor: *const c_char,
        hw: *const c_char,
        fw: *const c_char,
        dev: *const c_char,
        flags: u32,
    ) -> c_int;
    fn sv2_build_open_standard_mining_channel(
        buf: *mut u8,
        buf_len: usize,
        req: u32,
        user: *const c_char,
        rate: f32,
    ) -> c_int;
    fn sv2_build_open_extended_mining_channel(
        buf: *mut u8,
        buf_len: usize,
        req: u32,
        user: *const c_char,
        rate: f32,
        min_en: u16,
    ) -> c_int;
    fn sv2_build_submit_shares_standard(
        buf: *mut u8,
        buf_len: usize,
        ch: u32,
        seq: u32,
        job: u32,
        nonce: u32,
        ntime: u32,
        version: u32,
    ) -> c_int;
    fn sv2_build_submit_shares_extended(
        buf: *mut u8,
        buf_len: usize,
        ch: u32,
        seq: u32,
        job: u32,
        nonce: u32,
        ntime: u32,
        version: u32,
        en: *const u8,
        en_len: u8,
    ) -> c_int;
    fn sv2_parse_setup_connection_success(
        p: *const u8,
        len: u32,
        v: *mut u16,
        flags: *mut u32,
    ) -> c_int;
    fn sv2_parse_open_channel_success(
        p: *const u8,
        len: u32,
        req: *mut u32,
        ch: *mut u32,
        target: *mut u8,
        prefix: *mut u8,
        prefix_len: *mut u8,
        group: *mut u32,
    ) -> c_int;
    fn sv2_parse_open_extended_channel_success(
        p: *const u8,
        len: u32,
        req: *mut u32,
        ch: *mut u32,
        target: *mut u8,
        en_size: *mut u16,
        prefix: *mut u8,
        prefix_len: *mut u8,
        group: *mut u32,
    ) -> c_int;
    fn sv2_parse_new_mining_job(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        job: *mut u32,
        has: *mut bool,
        min_ntime: *mut u32,
        version: *mut u32,
        merkle: *mut u8,
    ) -> c_int;
    fn sv2_parse_set_new_prev_hash(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        job: *mut u32,
        prev: *mut u8,
        min_ntime: *mut u32,
        nbits: *mut u32,
    ) -> c_int;
    fn sv2_parse_set_extranonce_prefix(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        prefix: *mut u8,
        prefix_len: *mut u8,
    ) -> c_int;
    fn sv2_parse_set_target(p: *const u8, len: u32, ch: *mut u32, target: *mut u8) -> c_int;
    fn sv2_parse_submit_shares_success(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        accepted: *mut u32,
    ) -> c_int;
    fn sv2_parse_submit_shares_error(
        p: *const u8,
        len: u32,
        ch: *mut u32,
        seq: *mut u32,
        code: *mut c_char,
        code_size: usize,
    ) -> c_int;
    fn sv2_parse_new_extended_mining_job(p: *const u8, len: u32, ch: *mut u32) -> *mut ExtJob;
    fn sv2_ext_job_free(job: *mut ExtJob);
    fn sv2_target_to_pdiff(target: *const u8) -> u32;
}

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
    b.iter().map(|b| format!("{b:02x}")).collect()
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

macro_rules! compare {
    ($i:expr, $what:expr, $a:expr, $b:expr) => {
        assert_eq!($a, $b, "case {}: {} differs", $i, $what)
    };
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
        let (ptr, len) = (p.as_ptr(), p.len() as u32);
        total += 1;
        // SAFETY: valid payload and output buffers for every call.
        unsafe {
            match kind {
                0 | 1 => {
                    let mut a = (0u32, 0u32, [0u8; 32], 0u16, [0u8; 32], 0u8, 0u32);
                    let mut b = a;
                    let (ra, rb) = if kind == 0 {
                        (
                            ref_sv2_parse_open_channel_success(
                                ptr,
                                len,
                                &mut a.0,
                                &mut a.1,
                                a.2.as_mut_ptr(),
                                a.4.as_mut_ptr(),
                                &mut a.5,
                                &mut a.6,
                            ),
                            sv2_parse_open_channel_success(
                                ptr,
                                len,
                                &mut b.0,
                                &mut b.1,
                                b.2.as_mut_ptr(),
                                b.4.as_mut_ptr(),
                                &mut b.5,
                                &mut b.6,
                            ),
                        )
                    } else {
                        (
                            ref_sv2_parse_open_extended_channel_success(
                                ptr,
                                len,
                                &mut a.0,
                                &mut a.1,
                                a.2.as_mut_ptr(),
                                &mut a.3,
                                a.4.as_mut_ptr(),
                                &mut a.5,
                                &mut a.6,
                            ),
                            sv2_parse_open_extended_channel_success(
                                ptr,
                                len,
                                &mut b.0,
                                &mut b.1,
                                b.2.as_mut_ptr(),
                                &mut b.3,
                                b.4.as_mut_ptr(),
                                &mut b.5,
                                &mut b.6,
                            ),
                        )
                    };
                    compare!(i, "open channel result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "open channel success", a, b);
                        ok += 1;
                    }
                }
                2 => {
                    let mut a = (0u32, 0u32, false, 0u32, 0u32, [0u8; 32]);
                    let mut b = a;
                    let ra = ref_sv2_parse_new_mining_job(
                        ptr,
                        len,
                        &mut a.0,
                        &mut a.1,
                        &mut a.2,
                        &mut a.3,
                        &mut a.4,
                        a.5.as_mut_ptr(),
                    );
                    let rb = sv2_parse_new_mining_job(
                        ptr,
                        len,
                        &mut b.0,
                        &mut b.1,
                        &mut b.2,
                        &mut b.3,
                        &mut b.4,
                        b.5.as_mut_ptr(),
                    );
                    compare!(i, "new mining job result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "new mining job", a, b);
                        ok += 1;
                    }
                }
                3 => {
                    let (mut ca, mut cb) = (0xdead_u32, 0xdead_u32);
                    let ja = ref_sv2_parse_new_extended_mining_job(ptr, len, &mut ca);
                    let jb = sv2_parse_new_extended_mining_job(ptr, len, &mut cb);
                    compare!(i, "extended job channel id", ca, cb);
                    compare!(i, "extended job result", ja.is_null(), jb.is_null());
                    if jb.is_null() {
                        rec.add(format!("{i} 3 {cb:#x} null"));
                    }
                    if !ja.is_null() {
                        let (a, b) = (&*ja, &*jb);
                        let fields = |j: &ExtJob| {
                            let n = j.merkle_path_count as usize;
                            let bytes = |p: *mut u8, n: u16| {
                                if p.is_null() {
                                    Vec::new()
                                } else {
                                    std::slice::from_raw_parts(p, n as usize).to_vec()
                                }
                            };
                            (
                                (
                                    j.job_id,
                                    j.version,
                                    j.version_rolling_allowed,
                                    j.prev_hash,
                                    j.ntime,
                                    j.nbits,
                                    j.clean_jobs,
                                ),
                                j.merkle_path[..n].to_vec(),
                                j.merkle_path[n..].iter().all(|b| *b == [0; 32]),
                                (
                                    j.coinbase_prefix.is_null(),
                                    bytes(j.coinbase_prefix, j.coinbase_prefix_len),
                                ),
                                (
                                    j.coinbase_suffix.is_null(),
                                    bytes(j.coinbase_suffix, j.coinbase_suffix_len),
                                ),
                                (j.extranonce_prefix, j.extranonce_prefix_len),
                            )
                        };
                        compare!(i, "extended job", fields(a), fields(b));
                        rec.add(format!("{i} 3 {cb:#x} {}", ext_fields_text(&fields(b))));
                        ref_sv2_ext_job_free(ja);
                        sv2_ext_job_free(jb);
                        ok += 1;
                    }
                }
                4 => {
                    let size = 1 + rng.below(80);
                    let (mut a, mut b) = (
                        (0u32, 0u32, vec![0x55 as c_char; 81]),
                        (0u32, 0u32, vec![0x55 as c_char; 81]),
                    );
                    let ra = ref_sv2_parse_submit_shares_error(
                        ptr,
                        len,
                        &mut a.0,
                        &mut a.1,
                        a.2.as_mut_ptr(),
                        size,
                    );
                    let rb = sv2_parse_submit_shares_error(
                        ptr,
                        len,
                        &mut b.0,
                        &mut b.1,
                        b.2.as_mut_ptr(),
                        size,
                    );
                    compare!(i, "submit error result", ra, rb);
                    rec.add(if rb == 0 {
                        // the code buffer as bytes (c_char is signed on x86)
                        let code: Vec<u8> = b.2.iter().map(|&c| c as u8).collect();
                        format!("{i} {kind} 0 {} {} {} {}", b.0, b.1, size, hex(&code))
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "submit error", a, b);
                        ok += 1;
                    }
                }
                5 => {
                    let mut a = (0u32, [0u8; 32], 0u8);
                    let mut b = a;
                    let ra = ref_sv2_parse_set_extranonce_prefix(
                        ptr,
                        len,
                        &mut a.0,
                        a.1.as_mut_ptr(),
                        &mut a.2,
                    );
                    let rb = sv2_parse_set_extranonce_prefix(
                        ptr,
                        len,
                        &mut b.0,
                        b.1.as_mut_ptr(),
                        &mut b.2,
                    );
                    compare!(i, "extranonce prefix result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "extranonce prefix", a, b);
                        ok += 1;
                    }
                }
                6 => {
                    let (mut a, mut b) = ((0u16, 0u32), (0u16, 0u32));
                    let ra = ref_sv2_parse_setup_connection_success(ptr, len, &mut a.0, &mut a.1);
                    let rb = sv2_parse_setup_connection_success(ptr, len, &mut b.0, &mut b.1);
                    compare!(i, "setup success result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "setup success", a, b);
                    }
                }
                7 => {
                    let mut a = (0u32, 0u32, [0u8; 32], 0u32, 0u32);
                    let mut b = a;
                    let ra = ref_sv2_parse_set_new_prev_hash(
                        ptr,
                        len,
                        &mut a.0,
                        &mut a.1,
                        a.2.as_mut_ptr(),
                        &mut a.3,
                        &mut a.4,
                    );
                    let rb = sv2_parse_set_new_prev_hash(
                        ptr,
                        len,
                        &mut b.0,
                        &mut b.1,
                        b.2.as_mut_ptr(),
                        &mut b.3,
                        &mut b.4,
                    );
                    compare!(i, "prev hash result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "prev hash", a, b);
                    }
                }
                8 => {
                    let (mut a, mut b) = ((0u32, [0u8; 32]), (0u32, [0u8; 32]));
                    let ra = ref_sv2_parse_set_target(ptr, len, &mut a.0, a.1.as_mut_ptr());
                    let rb = sv2_parse_set_target(ptr, len, &mut b.0, b.1.as_mut_ptr());
                    compare!(i, "set target result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "set target", a, b);
                    }
                }
                9 => {
                    let (mut a, mut b) = ((0u32, 0u32), (0u32, 0u32));
                    let ra = ref_sv2_parse_submit_shares_success(ptr, len, &mut a.0, &mut a.1);
                    let rb = sv2_parse_submit_shares_success(ptr, len, &mut b.0, &mut b.1);
                    compare!(i, "submit success result", ra, rb);
                    rec.add(if rb == 0 {
                        format!("{i} {kind} 0 {b:?}")
                    } else {
                        format!("{i} {kind} {rb}")
                    });
                    if ra == 0 {
                        compare!(i, "submit success", a, b);
                    }
                }
                _ => {
                    let mut t = [0u8; 32];
                    let lead = rng.below(33);
                    for (j, b) in t.iter_mut().enumerate() {
                        *b = if j >= 32 - lead { 0 } else { rng.next() as u8 };
                    }
                    let pdiff = sv2_target_to_pdiff(t.as_ptr());
                    compare!(i, "pdiff", ref_sv2_target_to_pdiff(t.as_ptr()), pdiff);
                    let h = rng.bytes(6);
                    let (mut ha, mut hb) = (FrameHeader::default(), FrameHeader::default());
                    ref_sv2_parse_frame_header(h.as_ptr(), &mut ha);
                    sv2_parse_frame_header(h.as_ptr(), &mut hb);
                    compare!(i, "frame header", ha, hb);
                    rec.add(format!("{i} {kind} {pdiff} {hb:?}"));
                }
            }
        }
    }
    eprintln!("{total} payloads, {ok} parsed successfully by both");
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
        let (mut a, mut b) = (vec![0xaau8; 1200], vec![0xaau8; 1200]);
        let (pa, pb) = (a.as_mut_ptr(), b.as_mut_ptr());
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
        // SAFETY: buffers of 1200 bytes, buf_len never exceeds that.
        let (ra, rb) = unsafe {
            match kind {
                0 => {
                    let s: Vec<CString> = (0..5).map(|_| rng.cstring()).collect();
                    let port = rng.next() as u16;
                    (
                        ref_sv2_build_setup_connection(
                            pa,
                            buf_len,
                            s[0].as_ptr(),
                            port,
                            s[1].as_ptr(),
                            s[2].as_ptr(),
                            s[3].as_ptr(),
                            s[4].as_ptr(),
                            u1,
                        ),
                        sv2_build_setup_connection(
                            pb,
                            buf_len,
                            s[0].as_ptr(),
                            port,
                            s[1].as_ptr(),
                            s[2].as_ptr(),
                            s[3].as_ptr(),
                            s[4].as_ptr(),
                            u1,
                        ),
                    )
                }
                1 => {
                    let user = rng.cstring();
                    (
                        ref_sv2_build_open_standard_mining_channel(
                            pa,
                            buf_len,
                            u1,
                            user.as_ptr(),
                            rate,
                        ),
                        sv2_build_open_standard_mining_channel(
                            pb,
                            buf_len,
                            u1,
                            user.as_ptr(),
                            rate,
                        ),
                    )
                }
                2 => {
                    let user = rng.cstring();
                    let min_en = rng.next() as u16;
                    (
                        ref_sv2_build_open_extended_mining_channel(
                            pa,
                            buf_len,
                            u1,
                            user.as_ptr(),
                            rate,
                            min_en,
                        ),
                        sv2_build_open_extended_mining_channel(
                            pb,
                            buf_len,
                            u1,
                            user.as_ptr(),
                            rate,
                            min_en,
                        ),
                    )
                }
                3 => (
                    ref_sv2_build_submit_shares_standard(pa, buf_len, u1, u2, u3, u4, u5, u6),
                    sv2_build_submit_shares_standard(pb, buf_len, u1, u2, u3, u4, u5, u6),
                ),
                4 => {
                    let en = rng.bytes(rng.below(256));
                    (
                        ref_sv2_build_submit_shares_extended(
                            pa,
                            buf_len,
                            u1,
                            u2,
                            u3,
                            u4,
                            u5,
                            u6,
                            en.as_ptr(),
                            en.len() as u8,
                        ),
                        sv2_build_submit_shares_extended(
                            pb,
                            buf_len,
                            u1,
                            u2,
                            u3,
                            u4,
                            u5,
                            u6,
                            en.as_ptr(),
                            en.len() as u8,
                        ),
                    )
                }
                _ => {
                    let (ext, ty, len) = (rng.next() as u16, rng.next() as u8, rng.next() as u32);
                    (
                        ref_sv2_encode_frame_header(pa, ext, ty, len),
                        sv2_encode_frame_header(pb, ext, ty, len),
                    )
                }
            }
        };
        assert_eq!(ra, rb, "case {i} kind {kind}: return value");
        if ra > 0 {
            assert_eq!(
                a[..ra as usize],
                b[..ra as usize],
                "case {i} kind {kind}: frame"
            );
        }
        rec.add(format!("{i} {kind} {rb} {}", hex(&b[..rb.max(0) as usize])));
    }
    rec.finish();
}
