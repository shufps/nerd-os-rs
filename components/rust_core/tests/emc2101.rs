//! Golden tests of the EMC2101 fan controller/temperature sensor:
//! rust_core::emc2101 on a recording register bus (register accesses and
//! results) against the outputs of the former C/C++ code
//! (tests/golden/emc2101.txt, recorded by the differential tests).

use rust_core::emc2101;
use rust_core::i2c::RegBus;

mod golden;
use golden::Rec;

struct Fake {
    regs: [u8; 256],
    trace: String,
    fail: bool,
}

impl Fake {
    fn new(fail: bool) -> Self {
        Self {
            regs: [0; 256],
            trace: String::new(),
            fail,
        }
    }
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

fn set(f: &mut Fake, reg: u8, v: u8) {
    f.regs[reg as usize] = v;
}

#[test]
fn init_polarity_settings() {
    let mut rec = Rec::new("emc2101", "init_polarity_settings");
    for fail in [false, true] {
        for invert in [false, true] {
            let mut f = Fake::new(fail);
            let init = emc2101::init(&mut f, invert);
            let pol = emc2101::set_polarity(&mut f, invert);
            emc2101::set_ideality_factor(&mut f, 0x24);
            emc2101::set_beta_compensation(&mut f, 0x00);
            rec.add(format!(
                "fail {fail} invert {invert}: {init} {pol}\n{}",
                f.trace
            ));
            assert!(f.trace.starts_with("wr 4c 03 04\n"), "{}", f.trace);
        }
    }
    rec.finish();
}

#[test]
fn fan_speeds() {
    let mut rec = Rec::new("emc2101", "fan_speeds");
    let mut f = Fake::new(false);
    let mut fractions: Vec<f32> = (-100..=1000).map(|i| i as f32 / 1000.0).collect();
    fractions.extend([0.999_999, 1.0, 0.015_873, 0.5, 1e-9, -0.0]);
    for p in fractions {
        f.trace.clear();
        emc2101::set_speed(&mut f, p);
        rec.add(format!("{:#x}\n{}", p.to_bits(), f.trace));
    }
    // the boards' setFanSpeed(100) is full speed
    f.trace.clear();
    emc2101::set_speed(&mut f, 100.0);
    assert_eq!(f.trace, "wr 4c 4c 3f\n");
    rec.finish();
}

#[test]
fn tach_and_temperatures() {
    let mut rec = Rec::new("emc2101", "tach_and_temperatures");
    let mut f = Fake::new(false);
    let mut checked = 0;
    for reading in 0..=0xffffu32 {
        let [lsb, msb] = (reading as u16).to_le_bytes();
        set(&mut f, 0x46, lsb);
        set(&mut f, 0x47, msb);
        let rust = emc2101::speed(&mut f);
        // readings below 83 are out of the RPM range: 0
        if reading < 83 {
            assert_eq!(rust, 0);
            continue;
        }
        rec.add(format!("tach {reading:#06x}: {rust}"));
        checked += 1;
    }
    assert_eq!(checked, 0x10000 - 83);
    f.trace.clear();
    for msb in 0..=255u8 {
        for lsb in (0..=255u8).step_by(0x20) {
            set(&mut f, 0x01, msb);
            set(&mut f, 0x10, lsb);
            set(&mut f, 0x00, msb);
            let at = f.trace.len();
            let t = emc2101::external_temp(&mut f);
            let int = emc2101::internal_temp(&mut f);
            rec.add(format!(
                "temp {msb:02x} {lsb:02x}: {:#x} {int}\n{}",
                t.to_bits(),
                &f.trace[at..]
            ));
        }
    }
    assert!(f.trace.starts_with("rr 4c 01 1\nrr 4c 10 1\nrr 4c 00 1\n"));
    // bus errors: 0, after the first failed read
    f.fail = true;
    f.trace.clear();
    let sp = emc2101::speed(&mut f);
    let ext = emc2101::external_temp(&mut f);
    let int = emc2101::internal_temp(&mut f);
    rec.add(format!(
        "bus error: {sp} {:#x} {int}\n{}",
        ext.to_bits(),
        f.trace
    ));
    assert_eq!(f.trace, "rr 4c 46 1\nrr 4c 01 1\nrr 4c 00 1\n");
    rec.finish();
}
