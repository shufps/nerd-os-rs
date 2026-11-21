//! Differential tests of the board profiles: rust_core::board_profile
//! against the former board constructors (c-board/profile_ref.cpp, verbatim)
//! for every board and detection: every field equal, strings and lists by
//! content, the version rolling frequency by the ASIC that set it.

use std::ffi::{c_char, c_int, CStr};

use c_reference as _;
use rust_core::asic::Chip;
use rust_core::board_profile::{profile, BoardId, Detect, Profile};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn ref_board_profile(board: c_int, tps67: bool, six_phase: bool, out: *mut Profile) -> bool;
    fn board_profile(board: c_int, detect: *const Detect, out: *mut Profile) -> bool;
    fn board_profile_sizeof() -> usize;
}

fn s(p: *const c_char) -> String {
    assert!(!p.is_null());
    unsafe { CStr::from_ptr(p) }.to_str().unwrap().to_owned()
}

fn list(p: *const u32, n: usize) -> Vec<u32> {
    if n == 0 {
        return Vec::new();
    }
    unsafe { std::slice::from_raw_parts(p, n) }.to_vec()
}

/// the fields as comparable text (pointers resolved)
fn fields(p: &Profile) -> Vec<(&'static str, String)> {
    vec![
        ("device_model", s(p.device_model)),
        ("mining_agent", s(p.mining_agent)),
        ("version", p.version.to_string()),
        ("asic_model", s(p.asic_model)),
        ("asic_count", p.asic_count.to_string()),
        ("job_interval_ms", p.job_interval_ms.to_string()),
        (
            "frequencies",
            format!("{:?}", list(p.frequencies, p.frequency_count)),
        ),
        (
            "voltages",
            format!("{:?}", list(p.voltages, p.voltage_count)),
        ),
        ("default_frequency", p.default_frequency.to_string()),
        ("default_voltage_mv", p.default_voltage_mv.to_string()),
        ("abs_max_frequency", p.abs_max_frequency.to_string()),
        ("abs_min_voltage_mv", p.abs_min_voltage_mv.to_string()),
        ("abs_max_voltage_mv", p.abs_max_voltage_mv.to_string()),
        ("eco_frequency", p.eco_frequency.to_string()),
        ("eco_voltage_mv", p.eco_voltage_mv.to_string()),
        ("init_voltage_mv", p.init_voltage_mv.to_string()),
        ("num_phases", p.num_phases.to_string()),
        ("imax", p.imax.to_string()),
        ("ifault", p.ifault.to_bits().to_string()),
        ("fan_invert_polarity", p.fan_invert_polarity.to_string()),
        ("fan_perc", p.fan_perc.to_bits().to_string()),
        ("flip_screen", p.flip_screen.to_string()),
        ("num_temp_sensors", p.num_temp_sensors.to_string()),
        ("vr_max_temp", p.vr_max_temp.to_bits().to_string()),
        ("pid", format!("{:?}", p.pid)),
        ("fan_mode0", p.fan_mode0.to_string()),
        ("num_fans", p.num_fans.to_string()),
        ("fan_label0", s(p.fan_labels[0])),
        ("fan_label1", s(p.fan_labels[1])),
        ("max_pin", p.max_pin.to_bits().to_string()),
        ("min_pin", p.min_pin.to_bits().to_string()),
        ("max_vin", p.max_vin.to_bits().to_string()),
        ("min_vin", p.min_vin.to_bits().to_string()),
        ("min_current_a", p.min_current_a.to_bits().to_string()),
        ("max_current_a", p.max_current_a.to_bits().to_string()),
        ("max_difficulty", p.max_difficulty.to_string()),
        ("min_difficulty", p.min_difficulty.to_string()),
        (
            "min_difficulty_dual_pool",
            p.min_difficulty_dual_pool.to_string(),
        ),
        ("swarm_color", s(p.swarm_color)),
        ("default_theme", s(p.default_theme)),
        ("vr_frequency", p.vr_frequency.to_string()),
        ("has_hash_counter", p.has_hash_counter.to_string()),
        ("tps53667", p.tps53667.to_string()),
    ]
}

#[test]
fn profiles_match_the_former_constructors() {
    let mut rec = Rec::new("board_profile", "profiles_match_the_former_constructors");
    for b in 0..12 {
        let id = BoardId::from_int(b).unwrap();
        for (tps67, six) in [(false, false), (true, false), (false, true), (true, true)] {
            let mut r = std::mem::MaybeUninit::<Profile>::uninit();
            // the reference also checks the runtime copies (current =
            // default frequency/voltage/VR frequency)
            assert!(
                unsafe { ref_board_profile(b, tps67, six, r.as_mut_ptr()) },
                "{id:?}"
            );
            let mut r = unsafe { r.assume_init() };
            // the ASIC drivers are markers: their chip number
            r.vr_frequency = match r.vr_frequency {
                1366 => Chip::Bm1366.default_vr_frequency(),
                1368 => Chip::Bm1368.default_vr_frequency(),
                1370 => Chip::Bm1370.default_vr_frequency(),
                1373 => Chip::Bm1373.default_vr_frequency(),
                v => v,
            };
            let p = profile(
                id,
                &Detect {
                    tps53667: tps67,
                    six_phase: six,
                },
            );
            assert_eq!(
                fields(&p),
                fields(&r),
                "{id:?} tps53667={tps67} six_phase={six}"
            );
            let mut out = format!("{id:?} tps53667={tps67} six_phase={six}");
            for (k, v) in fields(&p) {
                out += &format!("\n{k} {v}");
            }
            rec.add(out);
        }
    }
    rec.finish();
}

#[test]
fn ffi() {
    assert_eq!(
        unsafe { board_profile_sizeof() },
        std::mem::size_of::<Profile>()
    );
    let mut p = std::mem::MaybeUninit::<Profile>::uninit();
    assert!(unsafe { board_profile(11, std::ptr::null(), p.as_mut_ptr()) });
    assert_eq!(s(unsafe { p.assume_init() }.device_model), "Q1373");
    for b in [-1, 12, 1000] {
        assert!(!unsafe { board_profile(b, std::ptr::null(), p.as_mut_ptr()) });
    }
}
