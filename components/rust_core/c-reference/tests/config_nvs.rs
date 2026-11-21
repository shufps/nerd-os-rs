//! The keys the configuration migration copies from the old individual NVS
//! entries: rust_core::config_nvs against the lists of the former
//! migrate_from_legacy() (c-config/, verbatim, with nvs_config.h's names).

use std::ffi::{c_char, c_int, CStr};

use c_reference as _;
use rust_core::config_nvs::{LEGACY_STR_KEYS, LEGACY_U16_KEYS, LEGACY_U64_KEYS};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

extern "C" {
    fn ref_legacy_keys(which: c_int, n: *mut usize) -> *const *const c_char;
}

fn reference(which: c_int) -> Vec<String> {
    let mut n = 0;
    let p = unsafe { ref_legacy_keys(which, &mut n) };
    (0..n)
        .map(|i| {
            unsafe { CStr::from_ptr(*p.add(i)) }
                .to_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

fn names(keys: &[&CStr]) -> Vec<String> {
    keys.iter()
        .map(|k| k.to_str().unwrap().to_owned())
        .collect()
}

#[test]
fn legacy_keys_match_the_former_lists() {
    let mut rec = Rec::new("config_nvs", "legacy_keys_match_the_former_lists");
    for (which, keys) in [
        &LEGACY_STR_KEYS[..],
        &LEGACY_U16_KEYS[..],
        &LEGACY_U64_KEYS[..],
    ]
    .iter()
    .enumerate()
    {
        let n = names(keys);
        assert_eq!(n, reference(which as c_int));
        rec.add(format!("{which}\n{}", n.join("\n")));
    }
    rec.finish();
}
