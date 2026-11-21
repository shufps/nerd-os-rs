//! Differential tests of the buck controller drivers: rust_core::tps536x7
//! against the former TPS53647/TPS53667 C++ drivers (c-drivers/, verbatim)
//! on a recording I2C stub (cpp-shim/i2c_stub.cpp). Both must issue the
//! same SMBus operations and return the same values for the same register
//! contents.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use c_reference as _;
use rust_core::tps536x7::{Bus, Chip, Tps536x7};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn i2c_stub_reset();
    fn i2c_stub_set(reg: u8, value: u16);
    fn i2c_stub_trace() -> *const c_char;
    fn i2c_stub_clear_trace();
}

macro_rules! reference {
    ($($name:ident: $init:ident, $set_vout:ident, $read:ident, $vid:ident, $status_reg:ident, $status:ident, $clear:ident;)*) => {
        extern "C" {
            $(
                fn $init(chip: c_int, phases: c_int, imax: c_int, ifault: f32) -> bool;
                fn $set_vout(v: f32) -> bool;
                fn $read(what: c_int) -> f32;
                fn $vid() -> u16;
                fn $status_reg(which: c_int) -> u8;
                fn $status();
                fn $clear();
            )*
        }
    };
}
reference! {
    plain: ref_tps_init, ref_tps_set_vout, ref_tps_read, ref_tps_vout_vid, ref_tps_status_reg, ref_tps_status, ref_tps_clear_faults;
    q1373: refq_tps_init, refq_tps_set_vout, refq_tps_read, refq_tps_vout_vid, refq_tps_status_reg, refq_tps_status, refq_tps_clear_faults;
}

/// The C++ reference of one build.
struct Ref {
    init: unsafe extern "C" fn(c_int, c_int, c_int, f32) -> bool,
    set_vout: unsafe extern "C" fn(f32) -> bool,
    read: unsafe extern "C" fn(c_int) -> f32,
    vid: unsafe extern "C" fn() -> u16,
    status_reg: unsafe extern "C" fn(c_int) -> u8,
    status: unsafe extern "C" fn(),
    clear: unsafe extern "C" fn(),
}

const PLAIN: Ref = Ref {
    init: ref_tps_init,
    set_vout: ref_tps_set_vout,
    read: ref_tps_read,
    vid: ref_tps_vout_vid,
    status_reg: ref_tps_status_reg,
    status: ref_tps_status,
    clear: ref_tps_clear_faults,
};
const Q1373: Ref = Ref {
    init: refq_tps_init,
    set_vout: refq_tps_set_vout,
    read: refq_tps_read,
    vid: refq_tps_vout_vid,
    status_reg: refq_tps_status_reg,
    status: refq_tps_status,
    clear: refq_tps_clear_faults,
};

/// The stub and its register file are global.
static LOCK: Mutex<()> = Mutex::new(());

/// The Rust side: the same register file, the same trace format.
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

/// (chip index for the reference, Rust chip, reference build)
fn variants() -> [(c_int, Chip, &'static Ref); 4] {
    [
        (0, Chip::Tps53647, &PLAIN),
        (
            1,
            Chip::Tps53667 {
                no_phase_shedding: false,
            },
            &PLAIN,
        ),
        (0, Chip::Tps53647, &Q1373),
        (
            1,
            Chip::Tps53667 {
                no_phase_shedding: true,
            },
            &Q1373,
        ),
    ]
}

unsafe fn setup(regs: &[u16; 256]) {
    i2c_stub_reset();
    for (i, v) in regs.iter().enumerate() {
        i2c_stub_set(i as u8, *v);
    }
}

#[test]
fn init_sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x5eed_1234);
    let mut rec = Rec::new("tps536x7", "init_sequences");
    for (chip_idx, chip, rf) in variants() {
        for device_code in [0x01f0u16, 0x01f8, 0x0000, 0xffff] {
            for _ in 0..200 {
                let phases = (r.next() % 9) as c_int - 1;
                let imax = (r.next() % 300) as c_int;
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
                unsafe {
                    setup(&regs);
                    let ok_ref = (rf.init)(chip_idx, phases, imax, ifault);
                    let t_ref = ref_trace();
                    let mut fake = Fake {
                        regs,
                        trace: String::new(),
                    };
                    let ok = Tps536x7::new(chip).init(&mut fake, phases, imax, ifault);
                    assert_eq!(ok, ok_ref, "{chip:?} code {device_code:04x}");
                    assert_eq!(
                        fake.trace, t_ref,
                        "{chip:?} phases {phases} imax {imax} ifault {ifault}"
                    );
                    rec.add(format!(
                        "{chip:?} {chip_idx} code {device_code:04x} phases {phases} imax {imax} ifault {:#x}: {ok}\n{}",
                        ifault.to_bits(),
                        fake.trace
                    ));
                    // not vacuous: a found chip gets the whole sequence
                    if ok {
                        assert!(
                            t_ref.contains("wc 71 12\n") && t_ref.contains("ww 71 46 "),
                            "{t_ref}"
                        );
                    } else {
                        assert_eq!(t_ref, "rw 71 fc\n");
                    }
                }
            }
        }
    }
    rec.finish();
}

#[test]
fn set_vout_over_the_range() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xabcdef);
    let mut rec = Rec::new("tps536x7", "set_vout_over_the_range");
    for (chip_idx, chip, rf) in variants() {
        let mut regs = [0u16; 256];
        regs[0xfc] = if chip_idx == 0 { 0x01f0 } else { 0x01f8 };
        let mut volts: Vec<f32> = (0..=2000).map(|i| i as f32 / 1000.0).collect();
        volts.extend([
            0.8, 1.0, 1.4, 0.7999999, 1.4000001, 0.995, 1.005, 1.2, 1.15, 1.0049999,
        ]);
        volts.extend((0..20_000).map(|_| 0.75 + (r.next() % 700_000) as f32 / 1_000_000.0));
        unsafe {
            setup(&regs);
            (rf.init)(chip_idx, 4, 60, 40.0);
            let mut fake = Fake {
                regs,
                trace: String::new(),
            };
            let mut d = Tps536x7::new(chip);
            d.init(&mut fake, 4, 60, 40.0);
            for v in volts {
                i2c_stub_clear_trace();
                fake.trace.clear();
                let ok_ref = (rf.set_vout)(v);
                let ok = d.set_vout(&mut fake, v);
                assert_eq!(ok, ok_ref, "{chip:?} {v}");
                let t_ref = ref_trace();
                assert_eq!(fake.trace, t_ref, "{chip:?} {v} ({:#x})", v.to_bits());
                rec.add(format!(
                    "{chip:?} {chip_idx} {:#x}: {ok}\n{}",
                    v.to_bits(),
                    fake.trace
                ));
                assert_eq!(t_ref.starts_with("ww 71 21 "), ok && v != 0.0);
            }
        }
    }
    rec.finish();
}

#[test]
fn getters_and_status() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x77);
    let mut rec = Rec::new("tps536x7", "getters_and_status");
    for (chip_idx, chip, rf) in variants() {
        for _ in 0..2000 {
            let mut regs = [0u16; 256];
            for v in regs.iter_mut() {
                *v = r.next() as u16;
            }
            regs[0xfc] = if chip_idx == 0 { 0x01f0 } else { 0x01f8 };
            unsafe {
                setup(&regs);
                (rf.init)(chip_idx, 4, 60, 40.0);
                i2c_stub_clear_trace();
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
                let theirs: Vec<f32> = (0..7).map(|w| (rf.read)(w)).collect();
                for (a, b) in ours.iter().zip(&theirs) {
                    assert_eq!(a.to_bits(), b.to_bits(), "{chip:?} {ours:?} {theirs:?}");
                }
                let vid = d.vout_vid(&mut fake);
                assert_eq!(vid, (rf.vid)());
                let regs_ours =
                    [0x78u8, 0x7b, 0x7a, 0x7c, 0x7d].map(|reg| d.status_reg(&mut fake, reg));
                let regs_ref: Vec<u8> = (0..5).map(|w| (rf.status_reg)(w)).collect();
                assert_eq!(regs_ours.to_vec(), regs_ref);
                d.status(&mut fake);
                (rf.status)();
                d.clear_faults(&mut fake);
                (rf.clear)();
                assert_eq!(fake.trace, ref_trace(), "{chip:?}");
                let bits = ours.map(|v| v.to_bits());
                rec.add(format!(
                    "{chip:?} {chip_idx} {bits:x?} {vid:#x} {regs_ours:x?}\n{}",
                    fake.trace
                ));
            }
        }
    }
    rec.finish();
}
