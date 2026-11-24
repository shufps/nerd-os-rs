//! Golden tests of the buck controller drivers: rust_core::drivers::tps536x7 (the
//! TPS53647/TPS53667 variants of both builds) on a recording SMBus fake
//! (operations and values for random register contents) against the outputs
//! of the former C/C++ code (tests/golden/tps536x7.txt, recorded by the
//! differential tests).

use rust_core::drivers::tps536x7::{Bus, Chip, Tps536x7};

mod golden;
use golden::Rec;

/// A register file, recording the operations.
struct Fake {
    regs: [u16; 256],
    trace: String,
}

impl Bus for Fake {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8> {
        self.trace += &format!("rb {addr:02x} {cmd:02x}\n");
        Some(self.regs[cmd as usize] as u8)
    }
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16> {
        self.trace += &format!("rw {addr:02x} {cmd:02x}\n");
        Some(self.regs[cmd as usize])
    }
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool {
        self.trace += &format!("wb {addr:02x} {cmd:02x} {data:02x}\n");
        true
    }
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool {
        self.trace += &format!("ww {addr:02x} {cmd:02x} {data:04x}\n");
        true
    }
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool {
        self.trace += &format!("wc {addr:02x} {cmd:02x}\n");
        true
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
}

/// (chip index, chip): the plain and the Q1373 build of both chips
fn variants() -> [(i32, Chip); 4] {
    [
        (0, Chip::Tps53647),
        (
            1,
            Chip::Tps53667 {
                no_phase_shedding: false,
            },
        ),
        (0, Chip::Tps53647),
        (
            1,
            Chip::Tps53667 {
                no_phase_shedding: true,
            },
        ),
    ]
}

#[test]
fn init_sequences() {
    let mut r = Rng(0x5eed_1234);
    let mut rec = Rec::new("tps536x7", "init_sequences");
    for (chip_idx, chip) in variants() {
        for device_code in [0x01f0u16, 0x01f8, 0x0000, 0xffff] {
            for _ in 0..200 {
                let phases = (r.next() % 9) as i32 - 1;
                let imax = (r.next() % 300) as i32;
                let ifault = match r.next() % 4 {
                    0 => (r.next() % 100) as f32,
                    1 => f32::from_bits(r.next() as u32),
                    2 => (r.next() % 100_000) as f32 / 100.0,
                    _ => -((r.next() % 50) as f32),
                };
                let mut regs = [0u16; 256];
                for v in regs.iter_mut() {
                    *v = r.next() as u16;
                }
                regs[0xfc] = device_code;
                let mut fake = Fake {
                    regs,
                    trace: String::new(),
                };
                let ok = Tps536x7::new(chip).init(&mut fake, phases, imax, ifault);
                rec.add(format!(
                    "{chip:?} {chip_idx} code {device_code:04x} phases {phases} imax {imax} ifault {:#x}: {ok}\n{}",
                    ifault.to_bits(),
                    fake.trace
                ));
                // not vacuous: a found chip gets the whole sequence
                if ok {
                    assert!(
                        fake.trace.contains("wc 71 12\n") && fake.trace.contains("ww 71 46 "),
                        "{}",
                        fake.trace
                    );
                } else {
                    assert_eq!(fake.trace, "rw 71 fc\n");
                }
            }
        }
    }
    rec.finish();
}

#[test]
fn set_vout_over_the_range() {
    let mut r = Rng(0xabcdef);
    let mut rec = Rec::new("tps536x7", "set_vout_over_the_range");
    for (chip_idx, chip) in variants() {
        let mut regs = [0u16; 256];
        regs[0xfc] = if chip_idx == 0 { 0x01f0 } else { 0x01f8 };
        let mut volts: Vec<f32> = (0..=2000).map(|i| i as f32 / 1000.0).collect();
        volts.extend([
            0.8, 1.0, 1.4, 0.7999999, 1.4000001, 0.995, 1.005, 1.2, 1.15, 1.0049999,
        ]);
        volts.extend((0..20_000).map(|_| 0.75 + (r.next() % 700_000) as f32 / 1_000_000.0));
        let mut fake = Fake {
            regs,
            trace: String::new(),
        };
        let mut d = Tps536x7::new(chip);
        d.init(&mut fake, 4, 60, 40.0);
        for v in volts {
            fake.trace.clear();
            let ok = d.set_vout(&mut fake, v);
            rec.add(format!(
                "{chip:?} {chip_idx} {:#x}: {ok}\n{}",
                v.to_bits(),
                fake.trace
            ));
            assert_eq!(fake.trace.starts_with("ww 71 21 "), ok && v != 0.0);
        }
    }
    rec.finish();
}

#[test]
fn getters_and_status() {
    let mut r = Rng(0x77);
    let mut rec = Rec::new("tps536x7", "getters_and_status");
    for (chip_idx, chip) in variants() {
        for _ in 0..2000 {
            let mut regs = [0u16; 256];
            for v in regs.iter_mut() {
                *v = r.next() as u16;
            }
            regs[0xfc] = if chip_idx == 0 { 0x01f0 } else { 0x01f8 };
            let mut fake = Fake {
                regs,
                trace: String::new(),
            };
            let mut d = Tps536x7::new(chip);
            d.init(&mut fake, 4, 60, 40.0);
            fake.trace.clear();

            let ours = [
                d.temperature(&mut fake),
                d.pin(&mut fake),
                d.pout(&mut fake),
                d.vin(&mut fake),
                d.iin(&mut fake),
                d.iout(&mut fake),
                d.vout(&mut fake),
            ];
            let vid = d.vout_vid(&mut fake);
            let regs_ours =
                [0x78u8, 0x7b, 0x7a, 0x7c, 0x7d].map(|reg| d.status_reg(&mut fake, reg));
            d.status(&mut fake);
            d.clear_faults(&mut fake);
            let bits = ours.map(|v| v.to_bits());
            rec.add(format!(
                "{chip:?} {chip_idx} {bits:x?} {vid:#x} {regs_ours:x?}\n{}",
                fake.trace
            ));
        }
    }
    rec.finish();
}
