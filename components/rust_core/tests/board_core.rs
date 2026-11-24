//! Golden tests of the board logic (rust_core::board::board_core): validation,
//! settings clamping, chip temperature maximum and the PMBus fault decoding
//! over all status combinations that matter, against the outputs of the
//! former Board/NerdQaxePlus code (tests/golden/board_core.txt, recorded by
//! the differential tests).

use rust_core::board::board_core::{self, ChipTemps, Limits};

mod golden;
use golden::Rec;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const LIMITS: [Limits; 4] = [
    Limits {
        abs_max_frequency: 0,
        abs_min_voltage_mv: 0,
        abs_max_voltage_mv: 0,
    },
    Limits {
        abs_max_frequency: 730,
        abs_min_voltage_mv: 900,
        abs_max_voltage_mv: 1200,
    },
    Limits {
        abs_max_frequency: 800,
        abs_min_voltage_mv: 1050,
        abs_max_voltage_mv: 1400,
    },
    Limits {
        abs_max_frequency: 1200,
        abs_min_voltage_mv: 0,
        abs_max_voltage_mv: 1400,
    },
];

#[test]
fn validation_and_clamping() {
    let mut r = Rng(0xb0a7);
    let mut rec = Rec::new("board_core", "validation_and_clamping");
    let mut values: Vec<f32> = (0..=3000).map(|i| i as f32 / 1000.0).collect();
    values.extend((0..20_000).map(|_| (r.next() % 2_000_000) as f32 / 1000.0));
    // the neighbours of the limits
    let up = |x: f32| f32::from_bits(x.to_bits() + 1);
    let down = |x: f32| f32::from_bits(x.to_bits() - 1);
    values.extend([0.0, -0.0, down(0.9), 0.9, 1.2, up(1.2), -1.0]);
    for l in &LIMITS {
        let (mf, mn, mx) = (
            l.abs_max_frequency,
            l.abs_min_voltage_mv,
            l.abs_max_voltage_mv,
        );
        for &v in &values {
            let vv = board_core::validate_voltage(v, l);
            let f = v * 1000.0;
            let vf = board_core::validate_frequency(f, l);
            rec.add(format!("{mf} {mn} {mx} {:08x} {vv} {vf}", v.to_bits()));
        }
        for _ in 0..20_000 {
            let (freq, volt) = (r.next() as u16, r.next() as u16 % 2000);
            let (f, v) = board_core::clamp_settings(freq, volt, l);
            // deliberate difference to the former code: the minimum is
            // applied as well
            if mn != 0 && (volt as i32) < mn {
                assert_eq!(v, mn);
            }
            rec.add(format!("{mf} {mn} {mx} clamp {freq} {volt} -> {f} {v}"));
        }
    }
    rec.finish();
}

#[test]
fn chip_temperatures() {
    let mut r = Rng(0x7e3);
    let mut rec = Rec::new("board_core", "chip_temperatures");
    let t = ChipTemps::new();
    for _ in 0..5000 {
        let n = (r.next() % 13) as usize;
        let temps: Vec<f32> = (0..n)
            .map(|_| match r.next() % 5 {
                0 => f32::NAN,
                1 => -((r.next() % 50) as f32),
                _ => (r.next() % 120_000) as f32 / 1000.0,
            })
            .collect();
        t.init(n as i32);
        for (i, v) in temps.iter().enumerate() {
            t.set(i as i32, *v);
        }
        let max = t.max();
        let bits: Vec<u32> = temps.iter().map(|v| v.to_bits()).collect();
        rec.add(format!("{bits:08x?} {:08x}", max.to_bits()));
    }
    rec.finish();
}

#[test]
fn pmbus_faults() {
    // the bits that matter and 0xff/0x00 for each register: all combinations
    let values = [0x00u8, 0xff, 0x80, 0x10, 0x90, 0x04, 0x40, 0x7f, 0x14, 0x84];
    let mut faults = [0u32; 6];
    let mut rec = Rec::new("board_core", "pmbus_faults");
    for &b in &values {
        for &i in &values {
            for &v in &values {
                for &n in &values {
                    for &t in &values {
                        let regs = [b, i, v, n, t];
                        let (e, s) = board_core::pmbus_fault(b, i, v, n, t);
                        faults[e as usize] += 1;
                        rec.add(format!("{regs:02x?} {e} {s:08x}"));
                    }
                }
            }
        }
    }
    // every outcome occurs
    assert!(
        faults.iter().enumerate().all(|(e, &n)| e == 1 || n > 0),
        "{faults:?}"
    );
    rec.finish();
}
