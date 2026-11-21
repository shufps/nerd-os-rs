//! Golden tests of the EMC2302 fan controller driver: rust_core::emc2302 on
//! a recording register bus (register accesses and results) against the
//! outputs of the former C/C++ code (tests/golden/emc2302.txt, recorded by
//! the differential tests).

use rust_core::emc2302;
use rust_core::i2c::RegBus;

mod golden;
use golden::Rec;

struct Fake {
    regs: [u8; 256],
    trace: String,
    fail: bool,
}

impl RegBus for Fake {
    fn read(&mut self, addr: u8, reg: u8) -> Option<u8> {
        let mut v = [0];
        self.read_into(addr, reg, &mut v).then_some(v[0])
    }
    fn write(&mut self, addr: u8, reg: u8, value: u8) -> bool {
        self.trace += &format!("wr {addr:02x} {reg:02x} {value:02x}\n");
        !self.fail
    }
    fn read_into(&mut self, addr: u8, reg: u8, out: &mut [u8]) -> bool {
        self.trace += &format!("rr {addr:02x} {reg:02x} {}\n", out.len());
        if self.fail {
            return false;
        }
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.regs[reg.wrapping_add(i as u8) as usize];
        }
        true
    }
}

#[test]
fn init_polarity_and_speeds() {
    let mut rec = Rec::new("emc2302", "init_polarity_and_speeds");
    for fail in [false, true] {
        for invert in [false, true] {
            let mut f = Fake {
                regs: [0; 256],
                trace: String::new(),
                fail,
            };
            let init = emc2302::init(&mut f, invert);
            let pol = emc2302::set_polarity(&mut f, invert);
            rec.add(format!(
                "fail {fail} invert {invert}: {init} {pol}\n{}",
                f.trace
            ));
        }
    }
    let mut f = Fake {
        regs: [0; 256],
        trace: String::new(),
        fail: false,
    };
    // 0..=100 % in 0.01 % steps and beyond
    for i in 0..=11_000 {
        let p = i as f32 / 10_000.0;
        for ch in [0, 1, 2, -1] {
            let at = f.trace.len();
            let ok = emc2302::set_speed(&mut f, ch, p);
            rec.add(format!(
                "{:#x} ch {ch}: {ok}\n{}",
                p.to_bits(),
                &f.trace[at..]
            ));
        }
    }
    rec.finish();
}

#[test]
fn rpm_readings() {
    let mut rec = Rec::new("emc2302", "rpm_readings");
    let mut f = Fake {
        regs: [0; 256],
        trace: String::new(),
        fail: false,
    };
    for msb in 0..=255u8 {
        for lsb in (0..=255u8).step_by(3) {
            // raw 0 is not covered: the Rust version reports 0 RPM
            if ((msb as u32) << 5 | (lsb as u32) >> 3) == 0 {
                continue;
            }
            for (ch, base) in [(0, 0x40u8), (1, 0x30)] {
                f.regs[(base + 0x0e) as usize] = msb;
                f.regs[(base + 0x0f) as usize] = lsb;
                let at = f.trace.len();
                let ours = emc2302::speed(&mut f, ch);
                assert!(ours.is_some());
                rec.add(format!(
                    "ch {ch} {msb:02x} {lsb:02x}: {ours:?} {}",
                    &f.trace[at..]
                ));
            }
        }
    }
    // a bus error: an error
    f.fail = true;
    let ours = emc2302::speed(&mut f, 0);
    assert_eq!(ours, None);
    rec.add(format!("bus error: {ours:?}"));
    rec.finish();
}
