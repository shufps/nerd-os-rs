//! Golden tests of the power sequencing (rust_core::board::power): the calls
//! and delays on recording hardware, results and board state for random
//! parameters, limits and failing buck/ASIC steps, against the outputs of
//! the former NerdQaxePlus/NerdQaxePlus2/NerdAxe code
//! (tests/golden/board_power.txt, recorded by the differential tests).

use rust_core::board::board_core::Limits;
use rust_core::board::power::{self, Hw, Params, Variant};

mod golden;
use golden::Rec;

/// The parameters (named as in the differential tests, their `{:?}` is
/// part of the golden outputs).
#[derive(Clone, Copy, Debug)]
struct RefParams {
    init_mv: i32,
    asic_mv: i32,
    phases: i32,
    imax: i32,
    ifault: f32,
    freq: i32,
    count: i32,
    max_diff: u32,
    vr_freq: u32,
    min_mv: i32,
    max_mv: i32,
    buck_init_ok: bool,
    set_vout_ok: bool,
    disable_ok: bool,
    chips: i32,
    baud: i32,
}

struct Fake {
    p: RefParams,
    trace: String,
    state: [i32; 4],
}

impl Fake {
    fn rec(&mut self, s: String) {
        self.trace += &s;
        self.trace += "\n";
    }
}

impl Hw for Fake {
    fn vreg(&mut self, on: bool) {
        self.rec(format!("vreg {}", on as u8));
    }
    fn ldo(&mut self, on: bool) {
        self.rec(format!("ldo {}", on as u8));
    }
    fn asic_reset(&mut self, released: bool) {
        self.rec(format!("reset {}", released as u8));
    }
    fn delay_ms(&mut self, ms: u32) {
        self.rec(format!("delay {ms}"));
    }
    fn buck_init(&mut self, phases: i32, imax: i32, ifault: f32) -> bool {
        self.rec(format!(
            "buck_init {phases} {imax} {:08x}",
            ifault.to_bits()
        ));
        self.p.buck_init_ok
    }
    fn buck_set_vout(&mut self, volts: f32) -> bool {
        self.rec(format!("vout {:08x}", volts.to_bits()));
        self.p.set_vout_ok
    }
    fn buck_disable(&mut self) -> bool {
        self.rec("disable".into());
        self.p.disable_ok
    }
    fn dac_set(&mut self, code: u8) {
        self.rec(format!("dac {code:02x}"));
    }
    fn serial_clear(&mut self) {
        self.rec("clear".into());
    }
    fn serial_set_baud(&mut self, baud: i32) {
        self.rec(format!("baud {baud}"));
    }
    fn asic_init(&mut self, f: i32, c: i32, d: u32, vr: u32) -> i32 {
        self.rec(format!("asic_init {f} {c} {d} {vr}"));
        self.p.chips
    }
    fn asic_max_baud(&mut self) -> i32 {
        self.rec("max_baud".into());
        self.p.baud
    }
    fn state(&mut self, what: i32, value: i32) {
        self.state[what as usize] = value;
    }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn params(p: &RefParams) -> Params {
    Params {
        init_voltage_mv: p.init_mv,
        asic_voltage_mv: p.asic_mv,
        num_phases: p.phases,
        imax: p.imax,
        ifault: p.ifault,
        asic_frequency: p.freq,
        asic_count: p.count,
        max_difficulty: p.max_diff,
        vr_frequency: p.vr_freq,
        limits: Limits {
            abs_max_frequency: 0,
            abs_min_voltage_mv: p.min_mv,
            abs_max_voltage_mv: p.max_mv,
        },
    }
}

#[test]
fn sequences() {
    let mut r = Rng(0x5e9);
    let mut rec = Rec::new("board_power", "sequences");
    let mut seen_init_ok = [0u32; 2];
    for _round in 0..20_000 {
        let rev7 = r.below(2) == 1;
        let p = RefParams {
            init_mv: [1050, 1200, 1250, 900][r.below(4) as usize],
            asic_mv: 800 + r.below(700) as i32,
            phases: 1 + r.below(6) as i32,
            imax: r.below(200) as i32,
            ifault: r.below(200_000) as f32 / 1000.0,
            freq: r.below(1000) as i32,
            count: 1 + r.below(12) as i32,
            max_diff: r.next() as u32,
            vr_freq: r.next() as u32,
            min_mv: [0, 900, 1050][r.below(3) as usize],
            max_mv: [0, 1200, 1400][r.below(3) as usize],
            buck_init_ok: r.below(6) != 0,
            set_vout_ok: r.below(6) != 0,
            disable_ok: r.below(4) != 0,
            chips: if r.below(5) == 0 {
                0
            } else {
                1 + r.below(12) as i32
            },
            baud: r.below(5_000_000) as i32,
        };
        let what = r.below(3) as i32;
        let volts = match r.below(4) {
            0 => 0.0,
            1 => r.below(2000) as f32 / 1000.0,
            _ => p.asic_mv as f32 / 1000.0,
        };
        let mut f = Fake {
            p,
            trace: String::new(),
            state: [0; 4],
        };
        let v = if rev7 {
            Variant::Rev7
        } else {
            Variant::Classic
        };
        let pr = params(&p);
        let res = match what {
            0 => power::init_asics(&mut f, v, &pr),
            1 => power::set_voltage(&mut f, v, volts, &pr.limits),
            _ => {
                power::shutdown(&mut f, v, &pr.limits);
                false
            }
        };
        rec.add(format!(
            "what {what} rev7 {rev7} volts {:08x} {p:?}\nres {res} state {:?}\n{}",
            volts.to_bits(),
            f.state,
            f.trace
        ));
        if what == 0 && res {
            seen_init_ok[rev7 as usize] += 1;
        }
    }
    // not vacuous: successful power-ups of both variants
    assert!(seen_init_ok.iter().all(|&n| n > 500), "{seen_init_ok:?}");
    rec.finish();
}

/// The NerdAxe family; voltages within the DS4432U range (the C++ cast of a
/// larger code is undefined).
#[test]
fn nerdaxe_sequences() {
    let mut r = Rng(0xa9e);
    let mut rec = Rec::new("board_power", "nerdaxe_sequences");
    let mut seen = [0u32; 3];
    for _round in 0..20_000 {
        let which = r.below(3) as i32;
        let p = RefParams {
            init_mv: 1000 + r.below(900) as i32,
            asic_mv: 1000 + r.below(900) as i32,
            phases: 0,
            imax: 0,
            ifault: 0.0,
            freq: r.below(1000) as i32,
            count: 1 + r.below(2) as i32,
            max_diff: r.next() as u32,
            vr_freq: r.next() as u32,
            min_mv: [0, 1050][r.below(2) as usize],
            max_mv: [0, 1400][r.below(2) as usize],
            buck_init_ok: true,
            set_vout_ok: r.below(6) != 0,
            disable_ok: true,
            chips: if r.below(5) == 0 { 0 } else { 1 },
            baud: r.below(5_000_000) as i32,
        };
        let what = r.below(3) as i32;
        let volts = match r.below(3) {
            0 => 0.0,
            _ => (1000 + r.below(900)) as f32 / 1000.0,
        };
        let mut f = Fake {
            p,
            trace: String::new(),
            state: [0; 4],
        };
        let v = [Variant::Axe, Variant::AxeGamma, Variant::AxeGaia][which as usize];
        let pr = params(&p);
        let res = match what {
            0 => power::init_asics(&mut f, v, &pr),
            1 => power::set_voltage(&mut f, v, volts, &pr.limits),
            _ => {
                power::shutdown(&mut f, v, &pr.limits);
                false
            }
        };
        rec.add(format!(
            "{v:?} what {what} volts {:08x} {p:?}\nres {res} state {:?}\n{}",
            volts.to_bits(),
            f.state,
            f.trace
        ));
        if what == 0 && res {
            seen[which as usize] += 1;
        }
    }
    assert!(seen.iter().all(|&n| n > 1000), "{seen:?}");
    rec.finish();
}

#[test]
fn ds4432_codes() {
    let mut rec = Rec::new("board_power", "ds4432_codes");
    let mut checked = 0;
    for mv in 0..=2500u32 {
        for frac in [0.0f32, 0.0004, 0.0009] {
            let v = mv as f32 / 1000.0 + frac;
            let code = power::ds4432_code(v);
            // the DAC range: as the former code; outside, the Rust version
            // limits the magnitude
            let raw = (((0.6 / 3320.0) - ((v as f64 - 0.6) / 4750.0)) / 0.000098921 * 127.0)
                .abs()
                .ceil();
            if !(0.046..=2.39).contains(&(v as f64)) || raw <= 127.0 {
                rec.add(format!("{:08x} {code:02x}", v.to_bits()));
                checked += 1;
            } else {
                assert_eq!(code & 0x7f, 127, "{v}");
                assert_eq!(code & 0x80 != 0, (v as f64) < 1.451, "{v}");
            }
        }
    }
    assert!(checked > 3000, "{checked}");
    // 0.98 V: 0x82 before (about 1.43 V), now the lowest code
    assert_eq!(power::ds4432_code(0.98), 0xff);
    rec.finish();
}
