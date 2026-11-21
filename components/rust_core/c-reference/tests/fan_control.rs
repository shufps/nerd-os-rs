//! Differential tests of the fan control: rust_core::fan_control against the
//! former FanController and PID library (c-fan/, verbatim) on a recording
//! board stub: the same fan accesses (bit-exact duty fractions), speeds,
//! RPMs and overheat flags over long random temperature sequences, with
//! settings reloads in between.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use c_reference as _;
use rust_core::fan_control::{ChannelConfig, FanController, Fans};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn reffan_setup(fans: c_int, cfg: *const u16, use_max: bool);
    fn reffan_init(sample_ms: c_int);
    fn reffan_load();
    fn reffan_set_rpm(ch: c_int, rpm: u16);
    fn reffan_update(chip: f32, vr: f32);
    fn reffan_get(ch: c_int, what: c_int) -> u16;
    fn reffan_trace() -> *const c_char;
    fn reffan_clear_trace();
}

static LOCK: Mutex<()> = Mutex::new(());

struct Fake {
    rpm: [u16; 2],
    trace: String,
}

impl Fans for Fake {
    fn rpm(&mut self, ch: usize) -> u16 {
        self.trace += &format!("get {ch}\n");
        self.rpm[ch]
    }
    fn set_speed(&mut self, ch: usize, fraction: f32) {
        self.trace += &format!("set {ch} {:08x}\n", fraction.to_bits());
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
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn random_config(r: &mut Rng) -> [ChannelConfig; 2] {
    let mut c = [ChannelConfig::default(); 2];
    for (ch, c) in c.iter_mut().enumerate() {
        c.mode = [0u8, 2, 3, 1, 2][r.below(5) as usize];
        if ch == 0 && c.mode == 3 && r.below(2) == 0 {
            c.mode = 2;
        }
        c.manual_speed = r.below(101) as u16;
        c.overheat_temp = [0u16, 70, 75, 90, 105][r.below(5) as usize];
        c.target_temp = 40 + r.below(40) as u16;
        c.p = r.below(2000) as u16;
        c.i = r.below(200) as u16;
        c.d = r.below(3000) as u16;
    }
    c
}

fn flat(c: &[ChannelConfig; 2]) -> [u16; 14] {
    let mut out = [0u16; 14];
    for (ch, c) in c.iter().enumerate() {
        out[7 * ch..7 * ch + 7].copy_from_slice(&[
            c.mode as u16,
            c.manual_speed,
            c.overheat_temp,
            c.target_temp,
            c.p,
            c.i,
            c.d,
        ]);
    }
    out
}

unsafe fn ref_trace() -> String {
    let t = CStr::from_ptr(reffan_trace())
        .to_string_lossy()
        .into_owned();
    reffan_clear_trace();
    t
}

#[test]
fn control_sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xfa17);
    let mut rec = Rec::new("fan_control", "control_sequences");
    let mut pid_steps = 0;
    let mut overheats = 0;
    for run in 0..300 {
        let fans = 1 + (run % 2) as c_int;
        let use_max = r.below(2) == 0;
        let mut cfg = random_config(&mut r);
        unsafe {
            reffan_setup(fans, flat(&cfg).as_ptr(), use_max);
            reffan_init(2000);
            let fc = FanController::new(fans as usize, 2000, cfg, use_max);
            let mut fake = Fake {
                rpm: [0; 2],
                trace: String::new(),
            };
            let mut chip = 30.0 + r.below(40) as f32;
            let mut vr = 30.0 + r.below(40) as f32;
            for step in 0..400 {
                // drifting temperatures with occasional jumps
                chip += (r.below(2001) as f32 - 1000.0) / 400.0;
                vr += (r.below(2001) as f32 - 1000.0) / 500.0;
                if r.below(50) == 0 {
                    chip = r.below(120) as f32 + r.below(1000) as f32 / 1000.0;
                }
                chip = chip.clamp(-10.0, 130.0);
                vr = vr.clamp(-10.0, 130.0);
                for ch in 0..2 {
                    let rpm = r.below(9000) as u16;
                    fake.rpm[ch] = rpm;
                    reffan_set_rpm(ch as c_int, rpm);
                }
                if r.below(60) == 0 {
                    // settings changed at runtime
                    cfg = random_config(&mut r);
                    let use_max = r.below(2) == 0;
                    reffan_setup(fans, flat(&cfg).as_ptr(), use_max);
                    reffan_load();
                    fc.load(cfg, use_max);
                }
                reffan_update(chip, vr);
                fc.update(&mut fake, chip, vr);
                let t = ref_trace();
                assert_eq!(
                    fake.trace, t,
                    "run {run} step {step} chip {chip} vr {vr} {cfg:?}"
                );
                let mut out = format!(
                    "run {run} step {step} chip {:08x} vr {:08x} rpm {:?} {:?}\n{}",
                    chip.to_bits(),
                    vr.to_bits(),
                    fake.rpm,
                    flat(&cfg),
                    fake.trace
                );
                fake.trace.clear();
                for ch in -1..3 {
                    let vals = (
                        fc.rpm(ch),
                        fc.speed_perc(ch),
                        fc.overheated(ch) as u16,
                        fc.overheat_temp(ch),
                    );
                    assert_eq!(vals.0, reffan_get(ch, 0));
                    assert_eq!(vals.1, reffan_get(ch, 1), "run {run} step {step}");
                    assert_eq!(vals.2, reffan_get(ch, 2));
                    assert_eq!(vals.3, reffan_get(ch, 3));
                    out += &format!("ch {ch} {vals:?}\n");
                }
                rec.add(out);
                if cfg[0].mode == 2 && !fc.overheated(0) {
                    pid_steps += 1;
                }
                overheats += fc.overheated(0) as u32;
            }
        }
    }
    // not vacuous: the PID path and the overheat path both ran a lot
    assert!(pid_steps > 20_000, "{pid_steps}");
    assert!(overheats > 1_000, "{overheats}");
    rec.finish();
}
