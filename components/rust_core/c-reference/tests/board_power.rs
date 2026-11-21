//! Differential tests of the power sequencing: rust_core::board_power
//! against the former NerdQaxePlus/NerdQaxePlus2 code (c-board/, verbatim)
//! on recording hardware: the same calls and delays in the same order, the
//! same results and board state, for random parameters, limits and failing
//! buck/ASIC steps.

use std::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use c_reference as _;
use rust_core::board_core::Limits;
use rust_core::board_power::{self, Hw, Params, Variant};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
struct RefParams {
    init_mv: c_int,
    asic_mv: c_int,
    phases: c_int,
    imax: c_int,
    ifault: f32,
    freq: c_int,
    count: c_int,
    max_diff: u32,
    vr_freq: u32,
    min_mv: c_int,
    max_mv: c_int,
    buck_init_ok: bool,
    set_vout_ok: bool,
    disable_ok: bool,
    chips: c_int,
    baud: c_int,
}

extern "C" {
    fn ref_power(
        p: *const RefParams,
        rev7: bool,
        what: c_int,
        volts: f32,
        trace: *mut c_char,
        cap: usize,
        state: *mut c_int,
    ) -> c_int;
    fn ref_power_axe(
        p: *const RefParams,
        which: c_int,
        what: c_int,
        volts: f32,
        trace: *mut c_char,
        cap: usize,
        state: *mut c_int,
    ) -> c_int;
}

/// The reference's trace and failure flags are global.
static LOCK: Mutex<()> = Mutex::new(());

struct Fake {
    p: RefParams,
    trace: String,
    state: [c_int; 4],
}

impl Fake {
    fn rec(&mut self, s: String) {
        self.trace += &s;
        self.trace += "\n";
    }
}

impl Hw for Fake {
    fn vreg(&mut self, on: bool) {
        self.rec(format!("vreg {}", on as u8));
    }
    fn ldo(&mut self, on: bool) {
        self.rec(format!("ldo {}", on as u8));
    }
    fn asic_reset(&mut self, released: bool) {
        self.rec(format!("reset {}", released as u8));
    }
    fn delay_ms(&mut self, ms: u32) {
        self.rec(format!("delay {ms}"));
    }
    fn buck_init(&mut self, phases: c_int, imax: c_int, ifault: f32) -> bool {
        self.rec(format!(
            "buck_init {phases} {imax} {:08x}",
            ifault.to_bits()
        ));
        self.p.buck_init_ok
    }
    fn buck_set_vout(&mut self, volts: f32) -> bool {
        self.rec(format!("vout {:08x}", volts.to_bits()));
        self.p.set_vout_ok
    }
    fn buck_disable(&mut self) -> bool {
        self.rec("disable".into());
        self.p.disable_ok
    }
    fn dac_set(&mut self, code: u8) {
        self.rec(format!("dac {code:02x}"));
    }
    fn serial_clear(&mut self) {
        self.rec("clear".into());
    }
    fn serial_set_baud(&mut self, baud: c_int) {
        self.rec(format!("baud {baud}"));
    }
    fn asic_init(&mut self, f: c_int, c: c_int, d: u32, vr: u32) -> c_int {
        self.rec(format!("asic_init {f} {c} {d} {vr}"));
        self.p.chips
    }
    fn asic_max_baud(&mut self) -> c_int {
        self.rec("max_baud".into());
        self.p.baud
    }
    fn state(&mut self, what: c_int, value: c_int) {
        self.state[what as usize] = value;
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

fn params(p: &RefParams) -> Params {
    Params {
        init_voltage_mv: p.init_mv,
        asic_voltage_mv: p.asic_mv,
        num_phases: p.phases,
        imax: p.imax,
        ifault: p.ifault,
        asic_frequency: p.freq,
        asic_count: p.count,
        max_difficulty: p.max_diff,
        vr_frequency: p.vr_freq,
        limits: Limits {
            abs_max_frequency: 0,
            abs_min_voltage_mv: p.min_mv,
            abs_max_voltage_mv: p.max_mv,
        },
    }
}

#[test]
fn sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x5e9);
    let mut rec = Rec::new("board_power", "sequences");
    let mut seen_init_ok = [0u32; 2];
    for round in 0..20_000 {
        let rev7 = r.below(2) == 1;
        let p = RefParams {
            init_mv: [1050, 1200, 1250, 900][r.below(4) as usize],
            asic_mv: 800 + r.below(700) as c_int,
            phases: 1 + r.below(6) as c_int,
            imax: r.below(200) as c_int,
            ifault: r.below(200_000) as f32 / 1000.0,
            freq: r.below(1000) as c_int,
            count: 1 + r.below(12) as c_int,
            max_diff: r.next() as u32,
            vr_freq: r.next() as u32,
            min_mv: [0, 900, 1050][r.below(3) as usize],
            max_mv: [0, 1200, 1400][r.below(3) as usize],
            buck_init_ok: r.below(6) != 0,
            set_vout_ok: r.below(6) != 0,
            disable_ok: r.below(4) != 0,
            chips: if r.below(5) == 0 {
                0
            } else {
                1 + r.below(12) as c_int
            },
            baud: r.below(5_000_000) as c_int,
        };
        let what = r.below(3) as c_int;
        let volts = match r.below(4) {
            0 => 0.0,
            1 => r.below(2000) as f32 / 1000.0,
            _ => p.asic_mv as f32 / 1000.0,
        };
        let mut buf = vec![0 as c_char; 8192];
        let mut state_ref = [0 as c_int; 4];
        let r_ref = unsafe {
            ref_power(
                &p,
                rev7,
                what,
                volts,
                buf.as_mut_ptr(),
                buf.len(),
                state_ref.as_mut_ptr(),
            )
        } != 0;
        let t_ref = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        let mut f = Fake {
            p,
            trace: String::new(),
            state: [0; 4],
        };
        let v = if rev7 {
            Variant::Rev7
        } else {
            Variant::Classic
        };
        let pr = params(&p);
        let res = match what {
            0 => board_power::init_asics(&mut f, v, &pr),
            1 => board_power::set_voltage(&mut f, v, volts, &pr.limits),
            _ => {
                board_power::shutdown(&mut f, v, &pr.limits);
                false
            }
        };
        assert_eq!(
            f.trace, t_ref,
            "round {round} what {what} rev7 {rev7} {p:?} {volts}"
        );
        if what != 2 {
            assert_eq!(res, r_ref, "round {round}");
        }
        assert_eq!(f.state, state_ref, "round {round} {p:?}");
        rec.add(format!(
            "what {what} rev7 {rev7} volts {:08x} {p:?}\nres {res} state {:?}\n{}",
            volts.to_bits(),
            f.state,
            f.trace
        ));
        if what == 0 && res {
            seen_init_ok[rev7 as usize] += 1;
        }
    }
    // not vacuous: successful power-ups of both variants
    assert!(seen_init_ok.iter().all(|&n| n > 500), "{seen_init_ok:?}");
    rec.finish();
}

/// The NerdAxe family; voltages within the DS4432U range (the C++ cast of a
/// larger code is undefined).
#[test]
fn nerdaxe_sequences() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xa9e);
    let mut rec = Rec::new("board_power", "nerdaxe_sequences");
    let mut seen = [0u32; 3];
    for round in 0..20_000 {
        let which = r.below(3) as c_int;
        let p = RefParams {
            init_mv: 1000 + r.below(900) as c_int,
            asic_mv: 1000 + r.below(900) as c_int,
            phases: 0,
            imax: 0,
            ifault: 0.0,
            freq: r.below(1000) as c_int,
            count: 1 + r.below(2) as c_int,
            max_diff: r.next() as u32,
            vr_freq: r.next() as u32,
            min_mv: [0, 1050][r.below(2) as usize],
            max_mv: [0, 1400][r.below(2) as usize],
            buck_init_ok: true,
            set_vout_ok: r.below(6) != 0,
            disable_ok: true,
            chips: if r.below(5) == 0 { 0 } else { 1 },
            baud: r.below(5_000_000) as c_int,
        };
        let what = r.below(3) as c_int;
        let volts = match r.below(3) {
            0 => 0.0,
            _ => (1000 + r.below(900)) as f32 / 1000.0,
        };
        let mut buf = vec![0 as c_char; 8192];
        let mut state_ref = [0 as c_int; 4];
        let r_ref = unsafe {
            ref_power_axe(
                &p,
                which,
                what,
                volts,
                buf.as_mut_ptr(),
                buf.len(),
                state_ref.as_mut_ptr(),
            )
        } != 0;
        let t_ref = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        let mut f = Fake {
            p,
            trace: String::new(),
            state: [0; 4],
        };
        let v = [Variant::Axe, Variant::AxeGamma, Variant::AxeGaia][which as usize];
        let pr = params(&p);
        let res = match what {
            0 => board_power::init_asics(&mut f, v, &pr),
            1 => board_power::set_voltage(&mut f, v, volts, &pr.limits),
            _ => {
                board_power::shutdown(&mut f, v, &pr.limits);
                false
            }
        };
        assert_eq!(
            f.trace, t_ref,
            "round {round} {v:?} what {what} {p:?} {volts}"
        );
        if what != 2 {
            assert_eq!(res, r_ref, "round {round}");
        }
        assert_eq!(f.state, state_ref, "round {round}");
        rec.add(format!(
            "{v:?} what {what} volts {:08x} {p:?}\nres {res} state {:?}\n{}",
            volts.to_bits(),
            f.state,
            f.trace
        ));
        if what == 0 && res {
            seen[which as usize] += 1;
        }
    }
    assert!(seen.iter().all(|&n| n > 1000), "{seen:?}");
    rec.finish();
}

#[test]
fn ds4432_codes() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = RefParams {
        init_mv: 0,
        asic_mv: 0,
        phases: 0,
        imax: 0,
        ifault: 0.0,
        freq: 0,
        count: 0,
        max_diff: 0,
        vr_freq: 0,
        min_mv: 0,
        max_mv: 0,
        buck_init_ok: true,
        set_vout_ok: true,
        disable_ok: true,
        chips: 0,
        baud: 0,
    };
    let mut rec = Rec::new("board_power", "ds4432_codes");
    let mut checked = 0;
    for mv in 0..=2500u32 {
        for frac in [0.0f32, 0.0004, 0.0009] {
            let v = mv as f32 / 1000.0 + frac;
            let code = board_power::ds4432_code(v);
            let mut buf = [0 as c_char; 16];
            let mut st = [0 as c_int; 4];
            let c_ref =
                unsafe { ref_power_axe(&p, 0, 3, v, buf.as_mut_ptr(), buf.len(), st.as_mut_ptr()) }
                    as u8;
            // the DAC range: identical; outside, the Rust version limits the
            // magnitude
            let raw = (((0.6 / 3320.0) - ((v as f64 - 0.6) / 4750.0)) / 0.000098921 * 127.0)
                .abs()
                .ceil();
            if !(0.046..=2.39).contains(&(v as f64)) || raw <= 127.0 {
                assert_eq!(code, c_ref, "{v}");
                rec.add(format!("{:08x} {code:02x}", v.to_bits()));
                checked += 1;
            } else {
                assert_eq!(code & 0x7f, 127, "{v}");
                assert_eq!(code & 0x80 != 0, (v as f64) < 1.451, "{v}");
            }
        }
    }
    assert!(checked > 3000, "{checked}");
    // 0.98 V: 0x82 before (about 1.43 V), now the lowest code
    assert_eq!(board_power::ds4432_code(0.98), 0xff);
    rec.finish();
}
