//! The configuration in NVS (port of `main/nvs_config.cpp`): the document
//! (`config.rs`) is stored as one JSON blob ("json_cfg" in namespace "main");
//! at boot it is loaded, or, without a usable blob, migrated from the old
//! individual NVS keys (only keys that exist, then the migration fixups).
//! Changes are written by a flush task with its stack in internal RAM
//! (flash writes turn the cache off); requests within 100 ms are batched.
//! Before the task runs (boot), a flush writes right away.

use core::ffi::{c_void, CStr};
use std::sync::{Condvar, Mutex};

use crate::config;
use crate::idf::nvs::{err_name, Nvs, READWRITE};
use crate::idf::{task, ESP_OK};
use crate::log::log;
use crate::util::ajson::{Json, Num};

const TAG: &CStr = c"nvs_config";
const NAMESPACE: &CStr = c"main";
const JSON_KEY: &CStr = c"json_cfg";

/// The keys the migration copies (as the former `migrate_from_legacy()`).
pub const LEGACY_STR_KEYS: [&CStr; 20] = [
    c"wifissid",
    c"wifipass",
    c"hostname",
    c"stratumurl",
    c"stratumuser",
    c"stratumpass",
    c"fbstratumurl",
    c"fbstratumuser",
    c"fbstratumpass",
    c"influx_url",
    c"influx_token",
    c"influx_bucket",
    c"influx_org",
    c"influx_prefix",
    c"alrt_disc_url",
    c"swarmconfig",
    c"otp_secret",
    c"otp_sess_key",
    c"sv2_auth_pk",
    c"fbsv2_authpk",
];

pub const LEGACY_U16_KEYS: [&CStr; 52] = [
    c"stratumport",
    c"stratumesub",
    c"stratumtls",
    c"stratum_keep",
    c"fbstratumport",
    c"fbstratumesub",
    c"fbstratumtls",
    c"asicfrequency",
    c"asicvoltage",
    c"asicjobinterval",
    c"flipscreen",
    c"invertscreen",
    c"pwmfanpol",
    c"autofanspeed",
    c"fanspeed",
    c"selftest",
    c"autoscreenoff",
    c"overheat_temp",
    c"influx_enable",
    c"influx_port",
    c"pid_temp",
    c"pid_p",
    c"pid_i",
    c"pid_d",
    c"fan1speed",
    c"fan1mode",
    c"fan1_pid_temp",
    c"fan1_pid_p",
    c"fan1_pid_i",
    c"fan1_pid_d",
    c"fan1_overheat",
    c"fan_pid_max",
    c"alrt_disc_en",
    c"alrt_disc_bf_en",
    c"alrt_disc_bd_en",
    c"alrt_disc_cb_en",
    c"block_found_en",
    c"can_enabled",
    c"cb_verify_mode",
    c"cb_max_fee",
    c"cb_vfy_force",
    c"fb_cb_vfy_mode",
    c"fb_cb_max_fee",
    c"fb_cb_vfy_frc",
    c"otp_enabled",
    c"otp_used_mask",
    c"pool_balance",
    c"pool_mode",
    c"sv2_proto",
    c"sv2_chan_type",
    c"fbsv2_proto",
    c"fbsv2_chtype",
];

pub const LEGACY_U64_KEYS: [&CStr; 6] = [
    c"stratumdiff",
    c"vr_frequency",
    c"bestdiff",
    c"totalblocks",
    c"otp_last_step",
    c"otp_boot_id",
];
/// Writes the serialized document to NVS.
fn write_blob(json: &[u8]) -> bool {
    let nvs = match Nvs::open(NAMESPACE, READWRITE) {
        Ok(n) => n,
        Err(e) => {
            log!(Error, TAG, "nvs_open failed: {}", err_name(e));
            return false;
        }
    };
    let e = nvs.set_blob(JSON_KEY, json);
    if e != ESP_OK {
        log!(Error, TAG, "nvs_set_blob failed: {}", err_name(e));
        return false;
    }
    let e = nvs.commit();
    if e != ESP_OK {
        log!(Error, TAG, "nvs_commit failed: {}", err_name(e));
        return false;
    }
    log!(Info, TAG, "Config flushed ({} bytes)", json.len());
    true
}

/// Copies the old individual keys that exist into the document.
fn migrate_from_legacy(nvs: &Nvs, single_fan: bool, overheat_default: u16) {
    log!(Info, TAG, "Migrating legacy NVS keys to JSON config ...");
    for key in LEGACY_STR_KEYS {
        if let Some(v) = nvs.get_str(key) {
            config::set(key.to_bytes(), Json::Str(v));
        }
    }
    for key in LEGACY_U16_KEYS {
        if let Some(v) = nvs.get_u16(key) {
            config::set(key.to_bytes(), Json::Num(Num::U(v as u64)));
        }
    }
    for key in LEGACY_U64_KEYS {
        if let Some(v) = nvs.get_u64(key) {
            config::set(key.to_bytes(), Json::Num(Num::U(v)));
        }
    }
    // overheat 0 -> 70, AFC -> manual 100 %, VReg overheat temp
    config::migration_fixups(single_fan, overheat_default);
    log!(Info, TAG, "Migration done");
}

struct Flush {
    pending: Mutex<bool>,
    wake: Condvar,
}

static FLUSH: Flush = Flush {
    pending: Mutex::new(false),
    wake: Condvar::new(),
};
static TASK_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static INITIALIZED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn flush_sync() {
    config::flush_with(write_blob);
}

unsafe extern "C" fn flush_task(_: *mut c_void) {
    log!(Info, TAG, "Flush task started");
    loop {
        {
            let mut p = FLUSH.pending.lock().unwrap_or_else(|e| e.into_inner());
            while !*p {
                p = FLUSH.wake.wait(p).unwrap_or_else(|e| e.into_inner());
            }
        }
        // a small delay batches rapid successive changes
        std::thread::sleep(std::time::Duration::from_millis(100));
        *FLUSH.pending.lock().unwrap_or_else(|e| e.into_inner()) = false;
        flush_sync();
    }
}

/// Persists the changes: signals the flush task, or (during boot) writes now.
pub fn request_flush() {
    use std::sync::atomic::Ordering;
    if TASK_RUNNING.load(Ordering::Acquire) {
        *FLUSH.pending.lock().unwrap_or_else(|e| e.into_inner()) = true;
        FLUSH.wake.notify_one();
    } else {
        flush_sync();
    }
}

/// Loads the document (or migrates the old keys) and starts the flush task.
/// `single_fan`/`overheat_default`: for the migration fixups (NerdAxe and
/// NerdAxe-γ; `CONFIG_OVERHEAT_TEMP`).
pub fn init(single_fan: bool, overheat_default: u16) {
    use std::sync::atomic::Ordering;
    if INITIALIZED.swap(true, Ordering::AcqRel) {
        return;
    }
    let nvs = match Nvs::open(NAMESPACE, READWRITE) {
        Ok(n) => n,
        Err(e) => {
            log!(Error, TAG, "nvs_open failed: {}", err_name(e));
            return;
        }
    };
    let mut migrate = true;
    if let Ok(blob) = nvs.get_blob(JSON_KEY) {
        if !blob.is_empty() {
            if config::load(&blob) {
                log!(Info, TAG, "Loaded JSON config ({} bytes)", blob.len());
                migrate = false;
            } else {
                log!(Warn, TAG, "JSON parse failed, re-migrating");
            }
        }
    }
    if migrate {
        config::clear();
        migrate_from_legacy(&nvs, single_fan, overheat_default);
        config::mark_dirty();
    }
    drop(nvs);
    // during boot, before the flush task runs
    flush_sync();
    if task::create_internal(flush_task, c"cfg_flush", 4096, 1) {
        TASK_RUNNING.store(true, Ordering::Release);
    }
}
