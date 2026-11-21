//! Differential tests of the NerdAxe INA260 power monitor and DS4432U DAC:
//! rust_core::{ina260, ds4432u} against the former C++ drivers
//! (c-drivers/nerdaxe/, verbatim) on the recording I2C stub: the same
//! register accesses and results, except for the current, which the Rust
//! version reads as a signed register.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use c_reference as _;
use rust_core::i2c::RegBus;
use rust_core::{ds4432u, ina260};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn i2c_stub_reset();
    fn i2c_stub_set(reg: u8, value: u16);
    fn i2c_stub_fail(fail: bool);
    fn i2c_stub_trace() -> *const c_char;
    fn i2c_stub_clear_trace();
    fn refina_installed() -> bool;
    fn refina_current() -> f32;
    fn refina_voltage() -> f32;
    fn refina_power() -> f32;
    fn refds_test() -> bool;
    fn refds_set(output: u8, code: u8) -> c_int;
    fn refds_get(output: u8, code: *mut u8) -> c_int;
}

static LOCK: Mutex<()> = Mutex::new(());

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

unsafe fn ref_trace() -> String {
    let t = CStr::from_ptr(i2c_stub_trace())
        .to_string_lossy()
        .into_owned();
    i2c_stub_clear_trace();
    t
}

#[test]
fn ina260_registers() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("ina260_ds4432u", "ina260_registers");
    unsafe {
        i2c_stub_reset();
        let mut f = Fake {
            regs: [0; 256],
            trace: String::new(),
            fail: false,
        };
        // the stub serves consecutive registers' low bytes: registers 1..4
        // hold the big-endian bytes of current, voltage and power
        for v in (0..=0xffffu32).step_by(7).chain([0x7fff, 0x8000, 0xffff]) {
            let [hi, lo] = (v as u16).to_be_bytes();
            for (reg, b) in [(1u8, hi), (2, lo), (3, hi ^ 0x5a), (4, lo ^ 0xa5)] {
                i2c_stub_set(reg, b as u16);
                f.regs[reg as usize] = b;
            }
            f.trace.clear();
            let inst = ina260::installed(&mut f);
            assert_eq!(inst, refina_installed());
            let c = ina260::current(&mut f);
            let c_ref = refina_current();
            // deliberate difference: the register is signed
            if v & 0x8000 == 0 {
                assert_eq!(c.to_bits(), c_ref.to_bits(), "{v:04x}");
            } else {
                assert_eq!(c, (v as u16 as i16) as f32 * 1.25);
                assert!(c < 0.0 && c_ref > 0.0);
            }
            let volt = ina260::voltage(&mut f);
            assert_eq!(volt.to_bits(), refina_voltage().to_bits());
            let pow = ina260::power(&mut f);
            assert_eq!(pow.to_bits(), refina_power().to_bits());
            let t = ref_trace();
            assert_eq!(f.trace, t);
            rec.add(format!(
                "{v:04x}: {inst} {:#x} {:#x} {:#x}\n{}",
                c.to_bits(),
                volt.to_bits(),
                pow.to_bits(),
                f.trace
            ));
            assert_eq!(t, "rr 40 02 2\nrr 40 01 2\nrr 40 02 2\nrr 40 03 2\n");
        }
        // the last register value, 0xffff: -1 LSB (the reference: 81918.75 mA)
        assert_eq!(ina260::current(&mut f), -1.25);
        assert_eq!(refina_current(), 81918.75);
        i2c_stub_fail(true);
        f.fail = true;
        f.trace.clear();
        i2c_stub_clear_trace();
        let inst = ina260::installed(&mut f);
        assert!(!inst && !refina_installed());
        let c = ina260::current(&mut f);
        assert_eq!(c, refina_current());
        let volt = ina260::voltage(&mut f);
        assert_eq!(volt, refina_voltage());
        let pow = ina260::power(&mut f);
        assert_eq!(pow, refina_power());
        assert_eq!(f.trace, ref_trace());
        rec.add(format!(
            "bus error: {inst} {:#x} {:#x} {:#x}\n{}",
            c.to_bits(),
            volt.to_bits(),
            pow.to_bits(),
            f.trace
        ));
        i2c_stub_fail(false);
    }
    rec.finish();
}

#[test]
fn ds4432u_codes() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("ina260_ds4432u", "ds4432u_codes");
    unsafe {
        for fail in [false, true] {
            i2c_stub_reset();
            i2c_stub_fail(fail);
            let mut f = Fake {
                regs: [0; 256],
                trace: String::new(),
                fail,
            };
            for output in 0..3u8 {
                for code in [0u8, 1, 0x7f, 0x80, 0x9c, 0xff] {
                    i2c_stub_set(0xf8, code as u16 ^ 0x11);
                    i2c_stub_set(0xf9, code as u16 ^ 0x22);
                    f.regs[0xf8] = code ^ 0x11;
                    f.regs[0xf9] = code ^ 0x22;
                    let ok = ds4432u::set_code(&mut f, output, code);
                    assert_eq!(ok, refds_set(output, code) == 0);
                    let mut v = 0u8;
                    let rc = refds_get(output, &mut v);
                    let got = ds4432u::code(&mut f, output);
                    assert_eq!(got, (rc == 0).then_some(v));
                    rec.add(format!(
                        "fail {fail} out {output} code {code:02x}: {ok} {got:?}"
                    ));
                }
            }
            let test = ds4432u::test(&mut f);
            assert_eq!(test, refds_test());
            let t = ref_trace();
            assert_eq!(f.trace, t);
            rec.add(format!("fail {fail}: test {test}\n{}", f.trace));
            assert!(t.starts_with("wr 48 f8 00\nrr 48 f8 1\n") && t.contains("wr 48 f9 ff\n"));
        }
        i2c_stub_fail(false);
    }
    rec.finish();
}
