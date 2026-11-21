//! Differential tests of the FXL6408 expander and TMP451 sensor drivers:
//! rust_core::{fxl6408, tmp451} against the former C++ drivers (c-drivers/,
//! verbatim) on the recording I2C stub, including the waits.

use std::cell::RefCell;
use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;
use std::time::Duration;

use c_reference as _;
use rust_core::fxl6408::Fxl6408;
use rust_core::i2c::RegBus;
use rust_core::tmp451::{Clock, Tmp451};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn i2c_stub_reset();
    fn i2c_stub_set(reg: u8, value: u16);
    fn i2c_stub_fail(fail: bool);
    fn i2c_stub_trace() -> *const c_char;
    fn i2c_stub_clear_trace();
    fn ref_fxl_new();
    fn ref_fxl_init() -> bool;
    fn ref_fxl_set_direction(pin: u8, output: bool) -> c_int;
    fn ref_fxl_write(pin: u8, level: bool) -> c_int;
    fn ref_fxl_pull_up(pin: u8) -> c_int;
    fn ref_fxl_input(v: *mut u8) -> c_int;
    fn ref_fxl_read_pin(pin: u8, level: *mut bool) -> c_int;
    fn ref_tmp_new(addr: u8);
    fn ref_tmp_init() -> c_int;
    fn ref_tmp_temperature(ch: c_int) -> f32;
    fn ref_tmp_raw(ch: c_int) -> f32;
    fn ref_tmp_cal(s: f32, a: f32, b: f32, c: f32, d: f32);
}

static LOCK: Mutex<()> = Mutex::new(());

/// The register file, the trace (with the waits) and the clock.
struct Fake {
    regs: [u8; 256],
    trace: RefCell<String>,
    fail: bool,
    ms: RefCell<u64>,
}

impl Fake {
    fn new(regs: [u8; 256], fail: bool) -> Self {
        Self {
            regs,
            trace: RefCell::new(String::new()),
            fail,
            ms: RefCell::new(0),
        }
    }
    fn take(&self) -> String {
        std::mem::take(&mut self.trace.borrow_mut())
    }
}

impl RegBus for &Fake {
    fn read(&mut self, addr: u8, reg: u8) -> Option<u8> {
        let mut v = [0];
        self.read_into(addr, reg, &mut v).then_some(v[0])
    }
    fn write(&mut self, addr: u8, reg: u8, value: u8) -> bool {
        *self.trace.borrow_mut() += &format!("wr {addr:02x} {reg:02x} {value:02x}\n");
        !self.fail
    }
    fn read_into(&mut self, addr: u8, reg: u8, out: &mut [u8]) -> bool {
        *self.trace.borrow_mut() += &format!("rr {addr:02x} {reg:02x} {}\n", out.len());
        if self.fail {
            return false;
        }
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.regs[reg.wrapping_add(i as u8) as usize];
        }
        true
    }
}

impl Clock for &Fake {
    fn sleep(&mut self, d: Duration) {
        *self.trace.borrow_mut() += &format!("delay {}\n", d.as_millis());
        *self.ms.borrow_mut() += d.as_millis() as u64;
    }
    fn now(&mut self) -> Duration {
        Duration::from_millis(*self.ms.borrow())
    }
}

unsafe fn ref_trace() -> String {
    let t = CStr::from_ptr(i2c_stub_trace())
        .to_string_lossy()
        .into_owned();
    i2c_stub_clear_trace();
    t
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

unsafe fn setup(regs: &[u8; 256], fail: bool) {
    i2c_stub_reset();
    for (i, v) in regs.iter().enumerate() {
        i2c_stub_set(i as u8, *v as u16);
    }
    i2c_stub_fail(fail);
}

#[test]
fn fxl6408_sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x1357);
    let mut rec = Rec::new("sensors", "fxl6408_sequences");
    for round in 0..300 {
        let fail = round % 10 == 9;
        let mut regs = [0u8; 256];
        for v in regs.iter_mut() {
            *v = r.next() as u8;
        }
        unsafe {
            setup(&regs, fail);
            ref_fxl_new();
            let fake = Fake::new(regs, fail);
            let d = Fxl6408::default();
            let mut bus = &fake;
            let mut clock = &fake;
            let init = d.init(&mut bus, &mut |t| clock.sleep(t));
            assert_eq!(init, ref_fxl_init());
            let mut ops = format!("round {round} fail {fail}: init {init}\n");
            for _ in 0..40 {
                let pin = (r.next() % 10) as u8;
                let flag = r.next() % 2 == 0;
                let op = r.next() % 5;
                let res = match op {
                    0 => {
                        let x = d.set_direction(&mut bus, pin, flag);
                        assert_eq!(x, ref_fxl_set_direction(pin, flag));
                        format!("{x}")
                    }
                    1 => {
                        let x = d.write(&mut bus, pin, flag);
                        assert_eq!(x, ref_fxl_write(pin, flag));
                        format!("{x}")
                    }
                    2 => {
                        let x = d.enable_pull_up(&mut bus, pin);
                        assert_eq!(x, ref_fxl_pull_up(pin));
                        format!("{x}")
                    }
                    3 => {
                        let mut v = 0u8;
                        let e = ref_fxl_input(&mut v);
                        let x = d.input_status(&mut bus);
                        assert_eq!(x, (e, v));
                        format!("{x:?}")
                    }
                    _ => {
                        let mut level = false;
                        let e = ref_fxl_read_pin(pin, &mut level);
                        let x = d.read_pin(&mut bus, pin);
                        match x {
                            Ok(l) => assert_eq!((e, level), (0, l)),
                            Err(x) => assert_eq!(e, x),
                        }
                        format!("{x:?}")
                    }
                };
                ops += &format!("op {op} pin {pin} {flag}: {res}\n");
            }
            let t = ref_trace();
            let ours = fake.take();
            assert_eq!(ours, t, "round {round}");
            rec.add(format!("{ops}{ours}"));
            // not vacuous
            assert!(t.starts_with("rr 43 01 1\n"));
            if !fail {
                assert!(t.contains("wr 43 07 00\ndelay 100\n"), "{t}");
            }
        }
    }
    rec.finish();
}

#[test]
fn tmp451_conversions() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x2468);
    let mut rec = Rec::new("sensors", "tmp451_conversions");
    for round in 0..3000 {
        let fail = round % 50 == 49;
        let mut regs = [0u8; 256];
        for v in regs.iter_mut() {
            *v = r.next() as u8;
        }
        // mostly not busy; sometimes busy for good (the timeout path)
        regs[0x02] = if round % 7 == 6 {
            0x80
        } else {
            regs[0x02] & 0x7f
        };
        let addr = 0x4c + (round % 3) as u8;
        let cal = round % 4 == 0;
        unsafe {
            setup(&regs, fail);
            ref_tmp_new(addr);
            let fake = Fake::new(regs, fail);
            let d = Tmp451::new(addr);
            if cal {
                ref_tmp_cal(1.06, -25.4, -25.3, -25.2, -25.1);
                d.set_cal(1.06, [-25.4, -25.3, -25.2, -25.1]);
            }
            let mut bus = &fake;
            let mut clock = &fake;
            let init = d.init(&mut bus);
            assert_eq!(init, ref_tmp_init());
            let mut vals =
                format!("round {round} addr {addr:02x} fail {fail} cal {cal}: init {init}\n");
            for ch in [-1, 0, 1, 2, 3, 4, -2] {
                let ours = d.temperature(&mut bus, &mut clock, ch, &mut |_| true);
                let theirs = ref_tmp_temperature(ch);
                assert!(
                    ours.to_bits() == theirs.to_bits() || (ours.is_nan() && theirs.is_nan()),
                    "round {round} ch {ch}: {ours} vs {theirs}"
                );
                let (a, b) = (d.raw(ch), ref_tmp_raw(ch));
                assert!(a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan()));
                vals += &format!("ch {ch}: {} {}\n", bits(ours), bits(a));
            }
            let t = ref_trace();
            let trace = fake.take();
            assert_eq!(trace, t, "round {round}");
            rec.add(format!("{vals}{trace}"));
            // not vacuous: the switch delays and the remote registers
            if !fail {
                assert!(t.contains("delay 50\n") && t.contains("delay 75\n"), "{t}");
                assert!(t.contains(&format!("wr {addr:02x} 0f ff\n")));
            }
        }
    }
    rec.finish();
}

/// The bits of a value, any NaN as "nan" (NaNs are compared as such).
fn bits(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else {
        format!("{:#x}", v.to_bits())
    }
}
