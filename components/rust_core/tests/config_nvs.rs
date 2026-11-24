//! Golden tests of the keys the configuration migration copies from the old
//! individual NVS entries (rust_core::config::nvs): against the lists of the
//! former migrate_from_legacy() with nvs_config.h's names
//! (tests/golden/config_nvs.txt, recorded by the differential tests).

use std::ffi::CStr;

use rust_core::config::nvs::{LEGACY_STR_KEYS, LEGACY_U16_KEYS, LEGACY_U64_KEYS};

mod golden;
use golden::Rec;

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
        rec.add(format!("{which}\n{}", n.join("\n")));
    }
    rec.finish();
}
