//! Golden tests of the ASIC driver (`src/asic`, the BM1366/BM1368/BM1370/
//! BM1373 chains) on a scripted UART: every byte sent to the chips and the
//! results, against the outputs of the former C/C++ code
//! (tests/golden/asic.txt, recorded by the differential tests).

use std::cell::Cell;
use std::collections::VecDeque;

use rust_core::mining::asic::driver::{Driver, Uart};
use rust_core::mining::asic::{Chip, TaskResult};

mod golden;
use golden::Rec;

const CHIPS: [i32; 4] = [0, 1, 2, 3]; // BM1366, BM1368, BM1370, BM1373

/// The scripted UART: records the bytes sent and the flushes, returns the
/// queued reads, then 0 (a timeout).
#[derive(Default)]
struct Serial {
    sent: Vec<u8>,
    reads: VecDeque<Vec<u8>>,
    clears: i32,
}

impl Uart for Serial {
    fn send(&mut self, data: &[u8]) {
        self.sent.extend_from_slice(data);
    }
    fn rx(&mut self, buf: &mut [u8], _timeout_ms: u16) -> i32 {
        let Some(r) = self.reads.pop_front() else {
            return 0;
        };
        let n = r.len().min(buf.len());
        buf[..n].copy_from_slice(&r[..n]);
        n as i32
    }
    fn clear(&mut self) {
        self.clears += 1;
    }
    fn delay_ms(&mut self, _ms: u32) {}
}

/// What the driver did: the bytes sent, the UART flushes and the result.
#[derive(Debug, PartialEq)]
struct Run<T> {
    sent: Vec<u8>,
    clears: i32,
    result: T,
}

/// Runs `f` on a fresh driver with `reads` queued.
fn run<T>(chip: i32, reads: &[Vec<u8>], f: impl FnOnce(&Driver, &mut Serial) -> T) -> Run<T> {
    let mut uart = Serial {
        reads: reads.iter().cloned().collect(),
        ..Default::default()
    };
    let d = Driver::new(Chip::from_raw(chip).unwrap());
    let result = f(&d, &mut uart);
    Run {
        sent: uart.sent,
        clears: uart.clears,
        result,
    }
}

/// `process_work` as `(found, result)`.
fn process(d: &Driver, u: &mut Serial) -> (bool, TaskResult) {
    match d.process_work(u) {
        Some(r) => (true, r),
        None => (false, TaskResult::default()),
    }
}

/// `send_hash_frequency` and the frequency set.
fn send_hash_frequency(d: &Driver, u: &mut Serial, f: f32) -> (bool, f32) {
    let ok = d.send_hash_frequency(u, f);
    (ok, d.actual_frequency())
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

/// Commands up to 29 data bytes, the range the former crc5() covered
/// (frames up to 31 bytes; real commands have at most 6 data bytes).
#[test]
fn frames_match_reference() {
    let rng = Rng(Cell::new(0x1234_5678));
    let mut rec = Rec::new("asic", "frames_match_reference");
    for i in 0..3000 {
        let header = rng.next() as u8;
        let max = if header & 0x20 != 0 { 120 } else { 30 };
        let data = rng.bytes(rng.below(max) as usize);
        let c = run(1, &[], |d, u| d.send(u, header, &data));
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
            let c = run(chip, &[], |d, u| {
                // version, target, ntime, starting nonce
                d.send_work(
                    u,
                    job_id,
                    fields[3],
                    fields[1],
                    fields[2],
                    merkle.as_slice().try_into().unwrap(),
                    prev.as_slice().try_into().unwrap(),
                    fields[0],
                )
            });
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
            let c = run(chip, &reads, |d, u| {
                d.set_address_interval(interval);
                process(d, u)
            });
            let head = format!(
                "chip {chip} interval {interval} read {}: {:?}",
                hex(&read),
                c.result
            );
            add_sent(&mut rec, head, &c.sent, c.clears);
        }
        // a timeout
        let c = run(chip, &[], |d, u| d.process_work(u).is_some());
        assert_eq!((c.result, c.clears), (false, 0));
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
        let c = run(1, &[read], |d, u| d.process_work(u).is_some());
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
        let c = run(chip, &[], |d, u| {
            for &diff in &diffs {
                d.set_difficulty_mask(u, diff);
            }
        });
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
        let c = run(chip, &[], |d, u| {
            for &v in &values {
                d.set_vr_frequency(u, v);
            }
            d.chip().default_vr_frequency()
        });
        let head = format!("chip {chip} {} values: {:?}", values.len(), c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}

/// 0 Hz is refused.
#[test]
fn vr_frequency_zero_is_not_sent() {
    let c = run(1, &[], |d, u| d.set_vr_frequency(u, 0));
    assert!(c.sent.is_empty());
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
            let c = run(chip, &reads, |d, u| {
                let n = d.init(u, frequency, count, difficulty, vr);
                let baud = d.set_max_baud(u);
                d.request_chip_temp(u);
                d.reset_counter(u, 0x8c);
                d.read_counter(u, 0x8c);
                (n, baud)
            });
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
        let c = run(chip, &[], |d, u| {
            targets
                .iter()
                .map(|&t| d.set_frequency(u, t))
                .collect::<Vec<_>>()
        });
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
        let c = run(2, &[], |d, u| {
            chunk
                .iter()
                .map(|&t| {
                    let (ok, actual) = send_hash_frequency(d, u, t);
                    (ok, if ok { actual.to_bits() } else { 0 })
                })
                .collect::<Vec<_>>()
        });
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
            let c = run(chip, &[], |d, u| {
                // a start frequency (when it has PLL settings), then the ramp
                let started = d.send_hash_frequency(u, start);
                let ramped = d.set_frequency(u, target);
                (started, ramped)
            });
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
        let c = run(chip, &[], |d, u| {
            for job in &jobs {
                d.send_raw_job(u, job.as_slice().try_into().unwrap());
            }
            let ch = d.chip();
            (
                ch.name().to_owned(),
                ch.small_core_count(),
                ch.default_vr_frequency(),
            )
        });
        let head = format!("chip {chip}: {:?}", c.result);
        add_sent(&mut rec, head, &c.sent, c.clears);
    }
    rec.finish();
}
