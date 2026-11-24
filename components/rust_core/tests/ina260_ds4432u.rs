//! Golden tests of the NerdAxe INA260 power monitor and DS4432U DAC:
//! rust_core::{ina260, ds4432u} on a recording register bus (register
//! accesses and results) against the outputs of the former C/C++ code
//! (tests/golden/ina260_ds4432u.txt, recorded by the differential tests;
//! the current deliberately differs: the Rust version reads a signed
//! register).

use rust_core::drivers::i2c::RegBus;
use rust_core::{drivers::ds4432u, drivers::ina260};

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
fn ina260_registers() {
    let mut rec = Rec::new("ina260_ds4432u", "ina260_registers");
    let mut f = Fake {
        regs: [0; 256],
        trace: String::new(),
        fail: false,
    };
    // registers 1..4 hold the big-endian bytes of current, voltage and power
    for v in (0..=0xffffu32).step_by(7).chain([0x7fff, 0x8000, 0xffff]) {
        let [hi, lo] = (v as u16).to_be_bytes();
        for (reg, b) in [(1u8, hi), (2, lo), (3, hi ^ 0x5a), (4, lo ^ 0xa5)] {
            f.regs[reg as usize] = b;
        }
        f.trace.clear();
        let inst = ina260::installed(&mut f);
        let c = ina260::current(&mut f);
        // deliberate difference: the register is signed
        if v & 0x8000 != 0 {
            assert_eq!(c, (v as u16 as i16) as f32 * 1.25);
            assert!(c < 0.0);
        }
        let volt = ina260::voltage(&mut f);
        let pow = ina260::power(&mut f);
        rec.add(format!(
            "{v:04x}: {inst} {:#x} {:#x} {:#x}\n{}",
            c.to_bits(),
            volt.to_bits(),
            pow.to_bits(),
            f.trace
        ));
        assert_eq!(f.trace, "rr 40 02 2\nrr 40 01 2\nrr 40 02 2\nrr 40 03 2\n");
    }
    // the last register value, 0xffff: -1 LSB (the C++ driver: 81918.75 mA)
    assert_eq!(ina260::current(&mut f), -1.25);
    f.fail = true;
    f.trace.clear();
    let inst = ina260::installed(&mut f);
    assert!(!inst);
    let c = ina260::current(&mut f);
    let volt = ina260::voltage(&mut f);
    let pow = ina260::power(&mut f);
    rec.add(format!(
        "bus error: {inst} {:#x} {:#x} {:#x}\n{}",
        c.to_bits(),
        volt.to_bits(),
        pow.to_bits(),
        f.trace
    ));
    rec.finish();
}

#[test]
fn ds4432u_codes() {
    let mut rec = Rec::new("ina260_ds4432u", "ds4432u_codes");
    for fail in [false, true] {
        let mut f = Fake {
            regs: [0; 256],
            trace: String::new(),
            fail,
        };
        for output in 0..3u8 {
            for code in [0u8, 1, 0x7f, 0x80, 0x9c, 0xff] {
                f.regs[0xf8] = code ^ 0x11;
                f.regs[0xf9] = code ^ 0x22;
                let ok = ds4432u::set_code(&mut f, output, code);
                let got = ds4432u::code(&mut f, output);
                rec.add(format!(
                    "fail {fail} out {output} code {code:02x}: {ok} {got:?}"
                ));
            }
        }
        let test = ds4432u::test(&mut f);
        rec.add(format!("fail {fail}: test {test}\n{}", f.trace));
        let t = &f.trace;
        assert!(t.starts_with("wr 48 f8 00\nrr 48 f8 1\n") && t.contains("wr 48 f9 ff\n"));
    }
    rec.finish();
}
