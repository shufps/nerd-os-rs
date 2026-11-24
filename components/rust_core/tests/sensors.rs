//! Golden tests of the FXL6408 expander and TMP451 sensor drivers:
//! rust_core::{fxl6408, tmp451} on a recording register bus (register
//! accesses with the waits, and results) against the outputs of the former
//! C/C++ code (tests/golden/sensors.txt, recorded by the differential tests).

use std::cell::RefCell;
use std::time::Duration;

use rust_core::drivers::fxl6408::Fxl6408;
use rust_core::drivers::i2c::RegBus;
use rust_core::drivers::tmp451::{Clock, Tmp451};

mod golden;
use golden::Rec;

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

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn fxl6408_sequences() {
    let mut r = Rng(0x1357);
    let mut rec = Rec::new("sensors", "fxl6408_sequences");
    for round in 0..300 {
        let fail = round % 10 == 9;
        let mut regs = [0u8; 256];
        for v in regs.iter_mut() {
            *v = r.next() as u8;
        }
        let fake = Fake::new(regs, fail);
        let d = Fxl6408::default();
        let mut bus = &fake;
        let mut clock = &fake;
        let init = d.init(&mut bus, &mut |t| clock.sleep(t));
        let mut ops = format!("round {round} fail {fail}: init {init}\n");
        for _ in 0..40 {
            let pin = (r.next() % 10) as u8;
            let flag = r.next() % 2 == 0;
            let op = r.next() % 5;
            let res = match op {
                0 => format!("{}", d.set_direction(&mut bus, pin, flag)),
                1 => format!("{}", d.write(&mut bus, pin, flag)),
                2 => format!("{}", d.enable_pull_up(&mut bus, pin)),
                3 => format!("{:?}", d.input_status(&mut bus)),
                _ => format!("{:?}", d.read_pin(&mut bus, pin)),
            };
            ops += &format!("op {op} pin {pin} {flag}: {res}\n");
        }
        let t = fake.take();
        rec.add(format!("{ops}{t}"));
        // not vacuous
        assert!(t.starts_with("rr 43 01 1\n"));
        if !fail {
            assert!(t.contains("wr 43 07 00\ndelay 100\n"), "{t}");
        }
    }
    rec.finish();
}

#[test]
fn tmp451_conversions() {
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
        let fake = Fake::new(regs, fail);
        let d = Tmp451::new(addr);
        if cal {
            d.set_cal(1.06, [-25.4, -25.3, -25.2, -25.1]);
        }
        let mut bus = &fake;
        let mut clock = &fake;
        let init = d.init(&mut bus);
        let mut vals =
            format!("round {round} addr {addr:02x} fail {fail} cal {cal}: init {init}\n");
        for ch in [-1, 0, 1, 2, 3, 4, -2] {
            let ours = d.temperature(&mut bus, &mut clock, ch, &mut |_| true);
            let a = d.raw(ch);
            vals += &format!("ch {ch}: {} {}\n", bits(ours), bits(a));
        }
        let t = fake.take();
        rec.add(format!("{vals}{t}"));
        // not vacuous: the switch delays and the remote registers
        if !fail {
            assert!(t.contains("delay 50\n") && t.contains("delay 75\n"), "{t}");
            assert!(t.contains(&format!("wr {addr:02x} 0f ff\n")));
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
