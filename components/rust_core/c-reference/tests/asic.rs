//! Differential tests of the ASIC drivers: the former C++ drivers (c-asic/)
//! and the current Rust driver (`src/asic`) run against the
//! same scripted UART; every byte sent to the chips must be identical.

use std::cell::Cell;
use std::ffi::c_void;
use std::sync::Mutex;

use c_reference as _;
use rust_core::asic::TaskResult;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

// The two builds export the same functions with prefixes ref_/new_.
macro_rules! driver_fns {
    (ref_) => {
        extern "C" {
            fn ref_asic_new(chip: i32) -> *mut c_void;
            fn ref_asic_free(a: *mut c_void);
            fn ref_asic_send(a: *mut c_void, header: u8, data: *const u8, len: u8);
            fn ref_asic_send_work(
                a: *mut c_void,
                job_id: u32,
                version: u32,
                target: u32,
                ntime: u32,
                starting_nonce: u32,
                merkle_root_be: *const u8,
                prev_block_hash_be: *const u8,
            ) -> u8;
            fn ref_asic_set_difficulty(a: *mut c_void, difficulty: i32);
            fn ref_asic_set_address_interval(a: *mut c_void, interval: u8);
            fn ref_asic_process(a: *mut c_void, result: *mut TaskResult) -> bool;
            fn ref_asic_set_vr_frequency(a: *mut c_void, f: u32);
            fn ref_asic_default_vr_frequency(a: *mut c_void) -> u32;
            fn ref_asic_init(
                a: *mut c_void,
                frequency: u64,
                count: u16,
                difficulty: u32,
                vr: u32,
            ) -> u8;
            fn ref_asic_set_frequency(a: *mut c_void, f: f32) -> bool;
            fn ref_asic_send_hash_frequency(a: *mut c_void, f: f32, actual: *mut f32) -> bool;
            fn ref_asic_set_max_baud(a: *mut c_void) -> i32;
            fn ref_asic_request_temp(a: *mut c_void);
            fn ref_asic_reset_counter(a: *mut c_void, reg: u8);
            fn ref_asic_read_counter(a: *mut c_void, reg: u8);
            fn ref_asic_send_raw_job(a: *mut c_void, job: *const u8);
            fn ref_asic_name(a: *mut c_void) -> *const std::ffi::c_char;
            fn ref_asic_small_core_count(a: *mut c_void) -> u16;
        }
    };
    (new_) => {
        extern "C" {
            fn new_asic_new(chip: i32) -> *mut c_void;
            fn new_asic_free(a: *mut c_void);
            fn new_asic_send(a: *mut c_void, header: u8, data: *const u8, len: u8);
            fn new_asic_send_work(
                a: *mut c_void,
                job_id: u32,
                version: u32,
                target: u32,
                ntime: u32,
                starting_nonce: u32,
                merkle_root_be: *const u8,
                prev_block_hash_be: *const u8,
            ) -> u8;
            fn new_asic_set_difficulty(a: *mut c_void, difficulty: i32);
            fn new_asic_set_address_interval(a: *mut c_void, interval: u8);
            fn new_asic_process(a: *mut c_void, result: *mut TaskResult) -> bool;
            fn new_asic_set_vr_frequency(a: *mut c_void, f: u32);
            fn new_asic_default_vr_frequency(a: *mut c_void) -> u32;
            fn new_asic_init(
                a: *mut c_void,
                frequency: u64,
                count: u16,
                difficulty: u32,
                vr: u32,
            ) -> u8;
            fn new_asic_set_frequency(a: *mut c_void, f: f32) -> bool;
            fn new_asic_send_hash_frequency(a: *mut c_void, f: f32, actual: *mut f32) -> bool;
            fn new_asic_set_max_baud(a: *mut c_void) -> i32;
            fn new_asic_request_temp(a: *mut c_void);
            fn new_asic_reset_counter(a: *mut c_void, reg: u8);
            fn new_asic_read_counter(a: *mut c_void, reg: u8);
            fn new_asic_send_raw_job(a: *mut c_void, job: *const u8);
            fn new_asic_name(a: *mut c_void) -> *const std::ffi::c_char;
            fn new_asic_small_core_count(a: *mut c_void) -> u16;
        }
    };
}
driver_fns!(ref_);
driver_fns!(new_);

extern "C" {
    fn serial_reset();
    fn serial_push_read(data: *const u8, len: usize);
    fn serial_sent(len: *mut usize) -> *const u8;
    fn serial_clears() -> i32;
}

/// One implementation's entry points.
struct Driver {
    new: unsafe extern "C" fn(i32) -> *mut c_void,
    free: unsafe extern "C" fn(*mut c_void),
    send: unsafe extern "C" fn(*mut c_void, u8, *const u8, u8),
    #[allow(clippy::type_complexity)]
    send_work:
        unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32, u32, *const u8, *const u8) -> u8,
    set_difficulty: unsafe extern "C" fn(*mut c_void, i32),
    set_address_interval: unsafe extern "C" fn(*mut c_void, u8),
    process: unsafe extern "C" fn(*mut c_void, *mut TaskResult) -> bool,
    set_vr_frequency: unsafe extern "C" fn(*mut c_void, u32),
    default_vr_frequency: unsafe extern "C" fn(*mut c_void) -> u32,
    init: unsafe extern "C" fn(*mut c_void, u64, u16, u32, u32) -> u8,
    set_frequency: unsafe extern "C" fn(*mut c_void, f32) -> bool,
    send_hash_frequency: unsafe extern "C" fn(*mut c_void, f32, *mut f32) -> bool,
    set_max_baud: unsafe extern "C" fn(*mut c_void) -> i32,
    request_temp: unsafe extern "C" fn(*mut c_void),
    reset_counter: unsafe extern "C" fn(*mut c_void, u8),
    read_counter: unsafe extern "C" fn(*mut c_void, u8),
    send_raw_job: unsafe extern "C" fn(*mut c_void, *const u8),
    name: unsafe extern "C" fn(*mut c_void) -> *const std::ffi::c_char,
    small_core_count: unsafe extern "C" fn(*mut c_void) -> u16,
}

const REFERENCE: Driver = Driver {
    new: ref_asic_new,
    free: ref_asic_free,
    send: ref_asic_send,
    send_work: ref_asic_send_work,
    set_difficulty: ref_asic_set_difficulty,
    set_address_interval: ref_asic_set_address_interval,
    process: ref_asic_process,
    set_vr_frequency: ref_asic_set_vr_frequency,
    default_vr_frequency: ref_asic_default_vr_frequency,
    init: ref_asic_init,
    set_frequency: ref_asic_set_frequency,
    send_hash_frequency: ref_asic_send_hash_frequency,
    set_max_baud: ref_asic_set_max_baud,
    request_temp: ref_asic_request_temp,
    reset_counter: ref_asic_reset_counter,
    read_counter: ref_asic_read_counter,
    send_raw_job: ref_asic_send_raw_job,
    name: ref_asic_name,
    small_core_count: ref_asic_small_core_count,
};
const CURRENT: Driver = Driver {
    new: new_asic_new,
    free: new_asic_free,
    send: new_asic_send,
    send_work: new_asic_send_work,
    set_difficulty: new_asic_set_difficulty,
    set_address_interval: new_asic_set_address_interval,
    process: new_asic_process,
    set_vr_frequency: new_asic_set_vr_frequency,
    default_vr_frequency: new_asic_default_vr_frequency,
    init: new_asic_init,
    set_frequency: new_asic_set_frequency,
    send_hash_frequency: new_asic_send_hash_frequency,
    set_max_baud: new_asic_set_max_baud,
    request_temp: new_asic_request_temp,
    reset_counter: new_asic_reset_counter,
    read_counter: new_asic_read_counter,
    send_raw_job: new_asic_send_raw_job,
    name: new_asic_name,
    small_core_count: new_asic_small_core_count,
};

const CHIPS: [i32; 4] = [0, 1, 2, 3]; // BM1366, BM1368, BM1370, BM1373

/// The scripted UART is global: one test at a time.
static SERIAL: Mutex<()> = Mutex::new(());

/// What a driver did: the bytes sent, the UART flushes and the result.
#[derive(Debug, PartialEq)]
struct Run<T> {
    sent: Vec<u8>,
    clears: i32,
    result: T,
}

/// Runs `f` on a fresh driver of each implementation with the same reads
/// queued; returns [reference, current].
fn run<T>(chip: i32, reads: &[Vec<u8>], f: impl Fn(&Driver, *mut c_void) -> T) -> [Run<T>; 2] {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    [&REFERENCE, &CURRENT].map(|d| {
        // SAFETY: objects created and freed here; the stub's buffer is copied.
        unsafe {
            serial_reset();
            for r in reads {
                serial_push_read(r.as_ptr(), r.len());
            }
            let a = (d.new)(chip);
            let result = f(d, a);
            (d.free)(a);
            let mut len = 0;
            let p = serial_sent(&mut len);
            let sent = if len == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(p, len).to_vec()
            };
            Run {
                sent,
                clears: serial_clears(),
                result,
            }
        }
    })
}

/// The bytes sent, as hex: a frame (55 aa, header, length, ...) per string.
fn frames(sent: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut p = 0;
    while p < sent.len() {
        let rest = sent.len() - p;
        let n = if rest >= 4 && sent[p] == 0x55 && sent[p + 1] == 0xaa {
            (2 + sent[p + 3] as usize).clamp(4, rest)
        } else {
            rest
        };
        out.push(hex(&sent[p..p + n]));
        p += n;
    }
    out
}

/// One run for the golden file: `head`, then the frames sent, 16 per entry.
fn add_sent(rec: &mut Rec, head: String, sent: &[u8], clears: i32) {
    let fr = frames(sent);
    rec.add(format!("{head} clears {clears} frames {}", fr.len()));
    for chunk in fr.chunks(16) {
        rec.add(chunk.join("\n"));
    }
}

fn hex(b: &[u8]) -> String {
    use std::fmt::Write as _;
    b.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
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
    fn below(&self, n: u64) -> u64 {
        self.next() % n
    }
    fn bytes(&self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
}

/// Commands up to 29 data bytes, the range the reference crc5() covers
/// (frames up to 31 bytes; real commands have at most 6 data bytes).
#[test]
fn frames_match_reference() {
    let rng = Rng(Cell::new(0x1234_5678));
    let mut rec = Rec::new("asic", "frames_match_reference");
    for i in 0..3000 {
        let header = rng.next() as u8;
        let max = if header & 0x20 != 0 { 120 } else { 30 };
        let data = rng.bytes(rng.below(max) as usize);
        let [r, c] = run(1, &[], |d, a| unsafe {
            (d.send)(a, header, data.as_ptr(), data.len() as u8)
        });
        assert_eq!(c, r, "case {i}: header {header:02x}, {} bytes", data.len());
        let head = format!("case {i}: header {header:02x} data {}", hex(&data));
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

#[test]
fn jobs_match_reference() {
    let rng = Rng(Cell::new(0x0bad_cafe));
    let mut rec = Rec::new("asic", "jobs_match_reference");
    for chip in CHIPS {
        for job_id in 0..=255u32 {
            let (merkle, prev) = (rng.bytes(32), rng.bytes(32));
            let fields: Vec<u32> = (0..4).map(|_| rng.next() as u32).collect();
            let [r, c] = run(chip, &[], |d, a| unsafe {
                (d.send_work)(
                    a,
                    job_id,
                    fields[0],
                    fields[1],
                    fields[2],
                    fields[3],
                    merkle.as_ptr(),
                    prev.as_ptr(),
                )
            });
            assert_eq!(c, r, "chip {chip}, job {job_id}");
            assert_eq!(c.sent.len(), 88);
            let head = format!("chip {chip} job {job_id}: {:?}", c.result);
            add_sent(&mut rec, head, &c.sent, c.clears);
        }
    }
    rec.finish();
}

fn response(rng: &Rng) -> Vec<u8> {
    let mut b = rng.bytes(11);
    b[0] = 0xaa;
    b[1] = 0x55;
    b
}

#[test]
fn responses_match_reference() {
    let rng = Rng(Cell::new(0xfeed_beef));
    let mut rec = Rec::new("asic", "responses_match_reference");
    for chip in CHIPS {
        for i in 0..4000 {
            let interval = [0u8, 1, 2, 4, 8, 16, 32, 64, 128][rng.below(9) as usize];
            let mut read = response(&rng);
            if i % 7 == 0 {
                read[rng.below(2) as usize] ^= 1 << rng.below(8); // bad preamble
            }
            let reads = [read.clone()];
            let [r, c] = run(chip, &reads, |d, a| unsafe {
                (d.set_address_interval)(a, interval);
                let mut result = TaskResult::default();
                let ok = (d.process)(a, &mut result);
                (ok, result)
            });
            assert_eq!(c, r, "chip {chip}, interval {interval}, {read:02x?}");
            let head = format!(
                "chip {chip} interval {interval} read {}: {:?}",
                hex(&read),
                c.result
            );
            add_sent(&mut rec, head, &c.sent, c.clears);
        }
        // a timeout
        let [r, c] = run(chip, &[], |d, a| unsafe {
            (d.process)(a, &mut TaskResult::default())
        });
        assert_eq!((c.result, c.clears), (false, 0));
        assert_eq!(c, r);
        let head = format!("chip {chip} timeout: {:?}", c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

/// A short read with a valid preamble is rejected.
#[test]
fn short_read_is_rejected() {
    for len in 2..11 {
        let read = vec![0xaa, 0x55, 1, 2, 3, 4, 5, 6, 7, 8, 0x80][..len].to_vec();
        let [_, c] = run(1, &[read], |d, a| unsafe {
            (d.process)(a, &mut TaskResult::default())
        });
        assert_eq!((c.result, c.clears), (false, 1), "{len} bytes");
    }
}

#[test]
fn difficulty_masks_match_reference() {
    let rng = Rng(Cell::new(0x5eed));
    let mut rec = Rec::new("asic", "difficulty_masks_match_reference");
    for chip in CHIPS {
        // a sequence on one object: unchanged masks are not sent again
        let diffs: Vec<i32> = (0..400)
            .map(|i| match i % 5 {
                0 => rng.next() as i32,
                1 => rng.below(1 << 16) as i32,
                2 => 1 << rng.below(31),
                3 => [0, 1, 2, -1, i32::MIN, i32::MAX][rng.below(6) as usize],
                _ => 256,
            })
            .collect();
        let [r, c] = run(chip, &[], |d, a| unsafe {
            for &diff in &diffs {
                (d.set_difficulty)(a, diff);
            }
        });
        assert_eq!(c, r, "chip {chip}");
        let head = format!("chip {chip} difficulties {diffs:?}: {:?}", c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

#[test]
fn vr_frequency_matches_reference() {
    let rng = Rng(Cell::new(0xabcdef));
    let mut rec = Rec::new("asic", "vr_frequency_matches_reference");
    for chip in CHIPS {
        let values: Vec<u32> = (0..2000)
            .map(|i| match i % 3 {
                0 => rng.next() as u32 | 1,
                1 => 1 + rng.below(200_000) as u32,
                _ => [1, 2, 3, u32::MAX, 35_000, 196_608_000][rng.below(6) as usize],
            })
            .collect();
        let [r, c] = run(chip, &[], |d, a| unsafe {
            for &v in &values {
                (d.set_vr_frequency)(a, v);
            }
            (d.default_vr_frequency)(a)
        });
        assert_eq!(c, r, "chip {chip}");
        let head = format!("chip {chip} {} values: {:?}", values.len(), c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

/// 0 Hz is refused; the reference does not handle it, so only the Rust
/// version runs.
#[test]
fn vr_frequency_zero_is_not_sent() {
    let _guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: object created and freed here.
    unsafe {
        serial_reset();
        let a = new_asic_new(1);
        new_asic_set_vr_frequency(a, 0);
        new_asic_free(a);
        let mut len = 0;
        serial_sent(&mut len);
        assert_eq!(len, 0);
    }
}

fn chip_id(chip: i32) -> [u8; 11] {
    let id = [0x66, 0x68, 0x70, 0x72][chip as usize];
    [0xaa, 0x55, 0x13, id, 0, 0, 0, 0, 0, 0, 0x0f]
}

#[test]
fn init_sequences_match_reference() {
    let rng = Rng(Cell::new(0x1373));
    let mut rec = Rec::new("asic", "init_sequences_match_reference");
    for chip in CHIPS {
        for i in 0..150 {
            let chips = rng.below(13) as usize;
            let mut reads: Vec<Vec<u8>> = (0..chips).map(|_| chip_id(chip).to_vec()).collect();
            if i % 4 == 0 {
                reads.insert(rng.below(chips as u64 + 1) as usize, rng.bytes(11));
            }
            let frequency = [400, 490, 525, 600, 56][rng.below(5) as usize] + rng.below(50);
            let (count, difficulty) = (rng.below(13) as u16, 1 + rng.below(4096) as u32);
            let vr = 10_000 + rng.below(100_000) as u32;
            let [r, c] = run(chip, &reads, |d, a| unsafe {
                let n = (d.init)(a, frequency, count, difficulty, vr);
                let baud = (d.set_max_baud)(a);
                (d.request_temp)(a);
                (d.reset_counter)(a, 0x8c);
                (d.read_counter)(a, 0x8c);
                (n, baud)
            });
            assert_eq!(
                c, r,
                "chip {chip}, case {i}: {chips} chips, {frequency} MHz"
            );
            let reads_hex: Vec<String> = reads.iter().map(|r| hex(r)).collect();
            let head = format!(
                "chip {chip} case {i}: {frequency} MHz count {count} difficulty {difficulty} vr {vr} reads {reads_hex:?}: {:?}",
                c.result
            );
            add_sent(&mut rec, head, &c.sent, c.clears);
        }
    }
    rec.finish();
}

#[test]
fn frequency_ramps_match_reference() {
    let rng = Rng(Cell::new(0x6_25));
    let mut rec = Rec::new("asic", "frequency_ramps_match_reference");
    for chip in CHIPS {
        let targets: Vec<f32> = (0..30)
            .map(|_| match rng.below(3) {
                0 => 50.0 + rng.below(1000) as f32 * 0.5,
                1 => (8 + rng.below(160)) as f32 * 6.25,
                _ => rng.below(1200) as f32,
            })
            .collect();
        let [r, c] = run(chip, &[], |d, a| unsafe {
            targets
                .iter()
                .map(|&t| (d.set_frequency)(a, t))
                .collect::<Vec<_>>()
        });
        assert_eq!(c, r, "chip {chip}");
        let bits: Vec<u32> = targets.iter().map(|t| t.to_bits()).collect();
        let head = format!("chip {chip} targets {bits:x?}: {:?}", c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

/// The PLL search over the whole range in 0.01 MHz steps, plus odd values.
#[test]
fn pll_settings_match_reference() {
    let mut targets: Vec<f32> = (0..200_000).map(|i| i as f32 * 0.01).collect();
    targets.extend([
        -1.0,
        -0.0,
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::MAX,
        f32::MIN_POSITIVE,
        1e9,
        6.25 * 160.0,
    ]);
    let mut rec = Rec::new("asic", "pll_settings_match_reference");
    for chunk in targets.chunks(5000) {
        let [r, c] = run(2, &[], |d, a| unsafe {
            chunk
                .iter()
                .map(|&t| {
                    let mut actual = 0.0f32;
                    let ok = (d.send_hash_frequency)(a, t, &mut actual);
                    // compare bit patterns (NaN != NaN)
                    (ok, if ok { actual.to_bits() } else { 0 })
                })
                .collect::<Vec<_>>()
        });
        assert_eq!(c, r, "around {} MHz", chunk[0]);
        let lines: Vec<String> = chunk
            .iter()
            .zip(&c.result)
            .map(|(t, (ok, bits))| format!("{:#x}: {ok} {bits:#x}", t.to_bits()))
            .collect();
        for l in lines.chunks(16) {
            rec.add(l.join("\n"));
        }
        let head = format!("chunk from {:#x}", chunk[0].to_bits());
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

/// Ramps between many start and target frequencies, including off-grid
/// starts, equal values and targets without PLL settings.
#[test]
fn frequency_ramps_from_any_start_match_reference() {
    let rng = Rng(Cell::new(0xa2));
    let mut rec = Rec::new("asic", "frequency_ramps_from_any_start_match_reference");
    for chip in CHIPS {
        for i in 0..300 {
            let pick = |rng: &Rng| match rng.below(4) {
                0 => 50.0 + rng.below(100_000) as f32 * 0.01,
                1 => (8 + rng.below(160)) as f32 * 6.25,
                2 => rng.below(1200) as f32 + 0.5,
                _ => [0.0, -5.0, 3000.0, 1e7, f32::NAN][rng.below(5) as usize],
            };
            let (start, target) = (pick(&rng), pick(&rng));
            let [r, c] = run(chip, &[], |d, a| unsafe {
                let mut actual = 0.0f32;
                // a start frequency (when it has PLL settings), then the ramp
                let started = (d.send_hash_frequency)(a, start, &mut actual);
                let ramped = (d.set_frequency)(a, target);
                (started, ramped)
            });
            assert_eq!(c, r, "chip {chip}, case {i}: {start} -> {target} MHz");
            let head = format!(
                "chip {chip} case {i}: {:#x} -> {:#x}: {:?}",
                start.to_bits(),
                target.to_bits(),
                c.result
            );
            add_sent(&mut rec, head, &c.sent, c.clears);
        }
    }
    rec.finish();
}

#[test]
fn chip_info_and_raw_jobs_match_reference() {
    let rng = Rng(Cell::new(0x82));
    let mut rec = Rec::new("asic", "chip_info_and_raw_jobs_match_reference");
    for chip in CHIPS {
        let jobs: Vec<Vec<u8>> = (0..50).map(|_| rng.bytes(82)).collect();
        let [r, c] = run(chip, &[], |d, a| unsafe {
            for job in &jobs {
                (d.send_raw_job)(a, job.as_ptr());
            }
            let name = std::ffi::CStr::from_ptr((d.name)(a)).to_owned();
            (name, (d.small_core_count)(a), (d.default_vr_frequency)(a))
        });
        assert_eq!(c, r, "chip {chip}");
        let head = format!("chip {chip}: {:?}", c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}
