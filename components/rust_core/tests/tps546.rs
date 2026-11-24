//! Golden tests of the TPS546 core voltage regulator (NerdAxe and rev7
//! drivers): the SMBus operations and values for random register
//! contents, against the outputs of the former C++ drivers
//! (tests/golden/tps546.txt, recorded by the differential tests).

use rust_core::drivers::tps546::{probe, Bus, Config, Tps546, Variant};

mod golden;
use golden::Rec;

const DEVICE_IDS: [[u8; 6]; 3] = [
    [0x54, 0x49, 0x54, 0x6b, 0x24, 0x41],
    [0x54, 0x49, 0x54, 0x6d, 0x24, 0x41],
    [0x54, 0x49, 0x54, 0x6d, 0x24, 0x62],
];

/// A register file, recording the operations.
struct Fake {
    regs: [u16; 256],
    trace: String,
    fail: bool,
}

impl Fake {
    fn new(regs: [u16; 256]) -> Self {
        Self {
            regs,
            trace: String::new(),
            fail: false,
        }
    }
}

impl Bus for Fake {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8> {
        self.trace += &format!("rb {addr:02x} {cmd:02x}\n");
        (!self.fail).then_some(self.regs[cmd as usize] as u8)
    }
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16> {
        self.trace += &format!("rw {addr:02x} {cmd:02x}\n");
        (!self.fail).then_some(self.regs[cmd as usize])
    }
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool {
        self.trace += &format!("wb {addr:02x} {cmd:02x} {data:02x}\n");
        !self.fail
    }
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool {
        self.trace += &format!("ww {addr:02x} {cmd:02x} {data:04x}\n");
        !self.fail
    }
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool {
        self.trace += &format!("wc {addr:02x} {cmd:02x}\n");
        !self.fail
    }
    fn read_block(&mut self, addr: u8, cmd: u8, out: &mut [u8]) -> bool {
        self.trace += &format!("rr {addr:02x} {cmd:02x} {}\n", out.len() + 1);
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.regs[cmd.wrapping_add(1 + i as u8) as usize] as u8;
        }
        !self.fail
    }
    fn write_block(&mut self, addr: u8, cmd: u8, data: &[u8]) -> bool {
        self.trace += &format!("wblk {addr:02x} {cmd:02x} {:02x}", data.len());
        for b in data {
            self.trace += &format!(" {b:02x}");
        }
        self.trace += "\n";
        !self.fail
    }
}

/// The trace without block writes, and the block writes.
fn split(trace: &str, prefix: &str) -> (String, Vec<String>) {
    let (blocks, rest): (Vec<&str>, Vec<&str>) = trace.lines().partition(|l| l.starts_with(prefix));
    (
        rest.iter().flat_map(|l| [*l, "\n"]).collect(),
        blocks.iter().map(|l| l.to_string()).collect(),
    )
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

/// A VOUT_MODE whose exponent keeps 3 V within ULINEAR16 (-14..=15; the
/// C++ float to uint16_t conversion is undefined above), upper bits random.
fn vout_mode(r: &mut Rng) -> u16 {
    let e = (r.next() % 30) as i32 - 14;
    ((e & 0x1f) as u16) | ((r.next() as u16) & 0xe0)
}

/// Registers of a found converter: the device ID block at 0xad.
fn regs_with_id(r: &mut Rng, id: Option<usize>) -> [u16; 256] {
    let mut regs = [0u16; 256];
    for v in regs.iter_mut() {
        *v = r.next() as u16;
    }
    regs[0x20] = vout_mode(r);
    if let Some(i) = id {
        regs[0xad] = 6;
        for (k, b) in DEVICE_IDS[i].iter().enumerate() {
            regs[0xae + k] = *b as u16;
        }
    }
    regs
}

fn small_float(r: &mut Rng) -> f32 {
    match r.next() % 4 {
        0 => (r.next() % 64) as f32,
        1 => (r.next() % 100_000) as f32 / 1000.0,
        2 => (r.next() % 1_000_000) as f32 / 7.0,
        _ => -((r.next() % 50) as f32) / 3.0,
    }
}

#[test]
fn init_sequences() {
    let mut r = Rng(0x546_546);
    let mut rec = Rec::new("tps546", "init_sequences");
    let mut found = 0;
    for round in 0..3000 {
        let id = match round % 5 {
            0 => None,
            k => Some((k as usize - 1) % 3),
        };
        let regs = regs_with_id(&mut r, id);
        let vin = [
            small_float(&mut r),
            small_float(&mut r),
            small_float(&mut r),
            small_float(&mut r),
        ];
        let iout = [small_float(&mut r), small_float(&mut r)];
        let custom = round % 3 != 0;
        let d = Tps546::nerdaxe();
        if custom {
            d.set_vin_config(vin[0], vin[1], vin[2], vin[3]);
            d.set_iout_config(iout[0], iout[1]);
        }
        let mut fake = Fake::new(regs);
        let rc = d.init(&mut fake);
        let (t, blk) = split(&fake.trace, "wblk ");
        rec.add(format!("{rc}\n{}", fake.trace));
        if rc == 0 {
            found += 1;
            // the whole block, to the given command
            assert_eq!(
                blk,
                [
                    "wblk 24 99 03 42 41 58",
                    "wblk 24 9a 03 48 45 58",
                    "wblk 24 9b 03 00 00 01",
                ]
            );
            // not vacuous: the configuration was written
            assert!(t.contains("ww 24 33 ") && t.contains("ww 24 21 "));
        } else {
            assert!(id.is_none());
            assert_eq!(t, "rr 24 ad 7\n");
        }
    }
    assert_eq!(found, 2400);
    rec.finish();
}

/// An initialized driver on the registers.
fn initialized(regs: &[u16; 256]) -> (Tps546, Fake) {
    let d = Tps546::nerdaxe();
    d.set_vin_config(10.5, 9.5, 10.5, 14.0);
    d.set_iout_config(28.0, 33.0);
    let mut fake = Fake::new(*regs);
    assert_eq!(d.init(&mut fake), 0);
    fake.trace.clear();
    (d, fake)
}

#[test]
fn set_vout_over_the_range() {
    let mut r = Rng(0xabcdef);
    let mut rec = Rec::new("tps546", "set_vout_over_the_range");
    for _ in 0..30 {
        let regs = regs_with_id(&mut r, Some(0));
        let mut volts: Vec<f32> = (0..=3200).map(|i| i as f32 / 1000.0).collect();
        volts.extend([0.8, 3.0, 0.7999999, 3.0000002, -1.0, 1.2, 0.93, 0.94]);
        volts.extend((0..2000).map(|_| 0.7 + (r.next() % 2_400_000) as f32 / 1_000_000.0));
        let (d, mut fake) = initialized(&regs);
        for v in volts {
            fake.trace.clear();
            let ok = d.set_vout(&mut fake, v);
            rec.add(format!("{:#x} {ok}\n{}", v.to_bits(), fake.trace));
            // not vacuous
            assert_eq!(fake.trace.contains("ww 24 21 "), ok && v != 0.0, "{v}");
        }
    }
    rec.finish();
}

#[test]
fn getters_and_frequency() {
    let mut r = Rng(0x77);
    let mut rec = Rec::new("tps546", "getters_and_frequency");
    for round in 0..3000 {
        let regs = regs_with_id(&mut r, Some(round % 3));
        let (d, mut fake) = initialized(&regs);
        let values = [
            d.temperature(&mut fake),
            d.vin(&mut fake),
            d.iout(&mut fake),
            d.vout(&mut fake),
        ];
        let khz = (r.next() % 3000) as i32 - 10;
        d.set_frequency(&mut fake, khz);
        let f = d.frequency(&mut fake);
        d.print_status(&mut fake);
        let bits: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
        rec.add(format!("{bits:x?} {khz} {f}\n{}", fake.trace));
        assert!(fake
            .trace
            .starts_with("rw 24 8d\nrw 24 88\nrw 24 8c\nrw 24 8b\nrb 24 20\n"));
    }
    rec.finish();
}

#[test]
fn bus_errors() {
    let mut r = Rng(0x99);
    let regs = regs_with_id(&mut r, Some(1));
    let (d, mut fake) = initialized(&regs);
    fake.fail = true;
    // a read error gives 0.0
    assert_eq!(d.temperature(&mut fake), 0.0);
    assert_eq!(d.vin(&mut fake), 0.0);
    assert_eq!(d.iout(&mut fake), 0.0);
    assert_eq!(d.vout(&mut fake), 0.0);
    assert!(!d.set_vout(&mut fake, 1.0));
    assert!(!d.set_vout(&mut fake, 0.0));
    // a failed device ID read: not found
    let mut fake = Fake::new(regs);
    fake.fail = true;
    assert_eq!(Tps546::nerdaxe().init(&mut fake), -1);
    assert_eq!(fake.trace, "rr 24 ad 7\n");
}

#[test]
fn not_initialized() {
    let mut fake = Fake::new([0x1234; 256]);
    let d = Tps546::nerdaxe();
    assert_eq!(d.temperature(&mut fake), 0.0);
    assert_eq!(d.vout(&mut fake), 0.0);
    assert_eq!(fake.trace, "");
}

/// The rev7 class: two stacked converters, 2.0..3.0 V over the domains.
fn rev7_dual(domains: u16) -> Tps546 {
    let cfg = Config {
        vout_min: 2.0,
        vout_command: 2.4,
        ..Config::rev7_dual()
    };
    Tps546::new(Variant::Rev7, cfg, domains)
}

#[test]
fn rev7_init_sequences() {
    let mut r = Rng(0x7e7);
    let mut rec = Rec::new("tps546", "rev7_init_sequences");
    let mut found = 0;
    for round in 0..2000 {
        let id = match round % 5 {
            0 => None,
            k => Some((k as usize - 1) % 3),
        };
        let regs = regs_with_id(&mut r, id);
        let single = round % 2 == 0;
        let domains = 1 + (round % 3) as u16;
        let d = if single {
            Tps546::new(Variant::Rev7, Config::rev7_single(), 1)
        } else {
            rev7_dual(domains)
        };
        let mut fake = Fake::new(regs);
        let rc = d.init(&mut fake);
        rec.add(format!("{rc}\n{}", fake.trace));
        let t = &fake.trace;
        if rc == 0 {
            found += 1;
            // not vacuous: stack/sync/phase, limits, MFR blocks
            assert!(t.contains("ww 24 ec ") && t.contains("wb 24 04 "));
            assert!(t.contains("wblk 24 9b 03 00 00 01\n"));
            assert!(t.ends_with("wc 24 03\n"), "{t}");
            // the dual configuration writes its compensation
            assert_eq!(t.contains("wblk 24 b1 05 12 34 42 25 04\n"), !single);
        } else {
            assert_eq!(t, "rr 24 ad 7\n");
        }
    }
    assert_eq!(found, 1600);
    rec.finish();
}

#[test]
fn rev7_set_vout_and_getters() {
    let mut r = Rng(0x7e70);
    let mut rec = Rec::new("tps546", "rev7_set_vout_and_getters");
    for round in 0..40 {
        let regs = regs_with_id(&mut r, Some(round % 3));
        let domains = 1 + (round % 2) as u16;
        let d = rev7_dual(domains);
        let mut fake = Fake::new(regs);
        assert_eq!(d.init(&mut fake), 0);
        fake.trace.clear();
        let mut volts: Vec<f32> = (0..=1600).map(|i| i as f32 / 1000.0 * 2.0).collect();
        volts.extend([0.0, 1.0, 1.2, 1.5, 0.9999999, 1.5000001, -1.0]);
        volts.extend((0..500).map(|_| (r.next() % 3_000_000) as f32 / 1_000_000.0));
        for v in volts {
            fake.trace.clear();
            let ok = d.set_vout(&mut fake, v);
            rec.add(format!("{:#x} {ok}\n{}", v.to_bits(), fake.trace));
            assert_eq!(fake.trace.contains("ww 24 21 "), ok && v != 0.0, "{v}");
        }
        fake.trace.clear();
        let values = [
            d.temperature(&mut fake),
            d.vin(&mut fake),
            d.iout(&mut fake),
            d.vout(&mut fake),
            d.pout(&mut fake),
            d.iin(&mut fake),
        ];
        let mut status = Vec::new();
        for reg in [0x78u8, 0x7b, 0x7a, 0x7c, 0x7d] {
            status.push(d.status_reg(&mut fake, reg));
        }
        d.print_status(&mut fake);
        d.clear_faults(&mut fake);
        let dis = d.disable_vout(&mut fake);
        let pr = probe(&mut fake);
        let bits: Vec<u32> = values.iter().map(|v| v.to_bits()).collect();
        rec.add(format!("{bits:x?} {status:x?} {dis} {pr}\n{}", fake.trace));
        assert!(fake
            .trace
            .contains("rb 24 7d\nwc 24 03\nwb 24 01 00\nrr 24 ad 7\n"));
    }
    rec.finish();
}

#[test]
fn rev7_single_set_vout() {
    let mut r = Rng(0x51);
    let regs = regs_with_id(&mut r, Some(2));
    let d = Tps546::new(Variant::Rev7, Config::rev7_single(), 1);
    let mut fake = Fake::new(regs);
    assert_eq!(d.init(&mut fake), 0);
    fake.trace.clear();
    let mut rec = Rec::new("tps546", "rev7_single_set_vout");
    for i in 0..=3200 {
        let v = i as f32 / 1000.0;
        fake.trace.clear();
        let ok = d.set_vout(&mut fake, v);
        rec.add(format!("{:#x} {ok}\n{}", v.to_bits(), fake.trace));
    }
    rec.finish();
    // not initialized: disable is a no-op, status 0xff
    let d = rev7_dual(2);
    let mut fake = Fake::new(regs);
    assert!(d.disable_vout(&mut fake));
    assert_eq!(d.status_reg(&mut fake, 0x78), 0xff);
    assert_eq!(fake.trace, "");
}
