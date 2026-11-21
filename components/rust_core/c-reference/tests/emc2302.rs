//! Differential tests of the EMC2302 fan controller driver: rust_core::emc2302
//! against the former C++ driver (c-drivers/EMC2302.cpp, verbatim) on the
//! recording I2C stub: the same register accesses and results.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use c_reference as _;
use rust_core::emc2302;
use rust_core::i2c::RegBus;

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn i2c_stub_reset();
    fn i2c_stub_set(reg: u8, value: u16);
    fn i2c_stub_fail(fail: bool);
    fn i2c_stub_trace() -> *const c_char;
    fn i2c_stub_clear_trace();
    fn ref_emc_init(invert: bool) -> bool;
    fn ref_emc_polarity(invert: bool) -> bool;
    fn ref_emc_set_speed(channel: c_int, percent: f32) -> c_int;
    fn ref_emc_get_speed(channel: c_int, dst: *mut u16) -> c_int;
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
fn init_polarity_and_speeds() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("emc2302", "init_polarity_and_speeds");
    unsafe {
        for fail in [false, true] {
            for invert in [false, true] {
                i2c_stub_reset();
                i2c_stub_fail(fail);
                let mut f = Fake {
                    regs: [0; 256],
                    trace: String::new(),
                    fail,
                };
                let init = emc2302::init(&mut f, invert);
                assert_eq!(init, ref_emc_init(invert));
                let pol = emc2302::set_polarity(&mut f, invert);
                assert_eq!(pol, ref_emc_polarity(invert));
                assert_eq!(f.trace, ref_trace());
                rec.add(format!(
                    "fail {fail} invert {invert}: {init} {pol}\n{}",
                    f.trace
                ));
            }
        }
        i2c_stub_reset();
        let mut f = Fake {
            regs: [0; 256],
            trace: String::new(),
            fail: false,
        };
        // 0..=100 % in 0.01 % steps and beyond (negative values are not
        // generated: the reference does not define them)
        for i in 0..=11_000 {
            let p = i as f32 / 10_000.0;
            for ch in [0, 1, 2, -1] {
                let at = f.trace.len();
                let ok = emc2302::set_speed(&mut f, ch, p);
                assert_eq!(ok, ref_emc_set_speed(ch, p) == 0);
                rec.add(format!(
                    "{:#x} ch {ch}: {ok}\n{}",
                    p.to_bits(),
                    &f.trace[at..]
                ));
            }
        }
        assert_eq!(f.trace, ref_trace());
    }
    rec.finish();
}

#[test]
fn rpm_readings() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("emc2302", "rpm_readings");
    unsafe {
        i2c_stub_reset();
        let mut f = Fake {
            regs: [0; 256],
            trace: String::new(),
            fail: false,
        };
        for msb in 0..=255u8 {
            for lsb in (0..=255u8).step_by(3) {
                // raw 0 is not compared: the Rust version reports 0 RPM
                if ((msb as u32) << 5 | (lsb as u32) >> 3) == 0 {
                    continue;
                }
                for (ch, base) in [(0, 0x40u8), (1, 0x30)] {
                    i2c_stub_set(base + 0x0e, msb as u16);
                    i2c_stub_set(base + 0x0f, lsb as u16);
                    f.regs[(base + 0x0e) as usize] = msb;
                    f.regs[(base + 0x0f) as usize] = lsb;
                    let mut theirs = 0xffffu16;
                    let err = ref_emc_get_speed(ch, &mut theirs);
                    let at = f.trace.len();
                    let ours = emc2302::speed(&mut f, ch);
                    assert_eq!(err == 0, ours.is_some());
                    assert_eq!(ours, Some(theirs), "msb {msb:02x} lsb {lsb:02x}");
                    rec.add(format!(
                        "ch {ch} {msb:02x} {lsb:02x}: {ours:?} {}",
                        &f.trace[at..]
                    ));
                }
            }
        }
        assert_eq!(f.trace, ref_trace());
        // a bus error: 0, an error
        i2c_stub_fail(true);
        f.fail = true;
        let mut theirs = 7u16;
        assert_ne!(ref_emc_get_speed(0, &mut theirs), 0);
        let ours = emc2302::speed(&mut f, 0);
        assert_eq!((theirs, ours), (0, None));
        rec.add(format!("bus error: {ours:?}"));
    }
    rec.finish();
}
