//! The typed config getters of `main/nvs_config.h` (`Config::get*()` /
//! `Config::is*()`), with their NVS keys and Kconfig defaults, for the Rust
//! code that reads several of them (the HTTP documents). Strings come as
//! `CString` (cut at a NUL, like the C copies).

use std::ffi::CString;

use crate::config;
use crate::sdkconfig as k;

fn s(key: &[u8], def: &str) -> CString {
    let v = config::get_str(key)
        .map(|v| crate::cutil::until_nul(&v).to_vec())
        .unwrap_or_else(|| def.as_bytes().to_vec());
    CString::new(v).unwrap_or_default()
}

fn u16v(key: &[u8], def: i64) -> u16 {
    config::get_u16(key, def as u16)
}

fn flag(key: &[u8], def: i64) -> bool {
    u16v(key, def) != 0
}

/// 0: the primary pool, else the fallback.
fn pool<'a>(i: usize, p: &'a [u8], f: &'a [u8]) -> &'a [u8] {
    if i == 0 {
        p
    } else {
        f
    }
}

pub fn wifi_ssid() -> CString {
    s(b"wifissid", k::ESP_WIFI_SSID)
}
pub fn hostname() -> CString {
    s(b"hostname", k::LWIP_LOCAL_HOSTNAME)
}
pub fn stratum_url(i: usize) -> CString {
    if i == 0 {
        s(b"stratumurl", k::STRATUM_URL)
    } else {
        s(b"fbstratumurl", k::STRATUM_FALLBACK_URL)
    }
}
pub fn stratum_user(i: usize) -> CString {
    if i == 0 {
        s(b"stratumuser", k::STRATUM_USER)
    } else {
        s(b"fbstratumuser", k::STRATUM_FALLBACK_USER)
    }
}
pub fn stratum_port(i: usize) -> u16 {
    if i == 0 {
        u16v(b"stratumport", k::STRATUM_PORT)
    } else {
        u16v(b"fbstratumport", k::STRATUM_FALLBACK_PORT)
    }
}
pub fn stratum_enonce_subscribe(i: usize) -> bool {
    if i == 0 {
        flag(b"stratumesub", k::STRATUM_ENONCE_SUBSCRIBE_VALUE)
    } else {
        flag(b"fbstratumesub", k::STRATUM_FALLBACK_ENONCE_SUBSCRIBE_VALUE)
    }
}
pub fn stratum_tls(i: usize) -> bool {
    if i == 0 {
        flag(b"stratumtls", k::STRATUM_TLS_VALUE)
    } else {
        flag(b"fbstratumtls", k::STRATUM_FALLBACK_TLS_VALUE)
    }
}
pub fn stratum_protocol(i: usize) -> u16 {
    u16v(pool(i, b"sv2_proto", b"fbsv2_proto"), 0)
}
pub fn sv2_authority_pubkey(i: usize) -> CString {
    s(pool(i, b"sv2_auth_pk", b"fbsv2_authpk"), "")
}
pub fn sv2_channel_type(i: usize) -> u16 {
    u16v(pool(i, b"sv2_chan_type", b"fbsv2_chtype"), 0)
}
pub fn sv2_require_auth(i: usize) -> bool {
    flag(pool(i, b"sv2_reqauth", b"fbsv2_reqauth"), 0)
}
pub fn coinbase_verify_mode(i: usize) -> u16 {
    u16v(pool(i, b"cb_verify_mode", b"fb_cb_vfy_mode"), 0)
}
pub fn coinbase_max_fee(i: usize) -> u16 {
    u16v(pool(i, b"cb_max_fee", b"fb_cb_max_fee"), 30)
}
pub fn coinbase_verify_force(i: usize) -> bool {
    flag(pool(i, b"cb_vfy_force", b"fb_cb_vfy_frc"), 0)
}
pub fn stratum_difficulty() -> u32 {
    config::get_u64(b"stratumdiff", k::STRATUM_DIFFICULTY as u64) as u32
}
pub fn stratum_keepalive() -> bool {
    flag(b"stratum_keep", k::STRATUM_KEEPALIVE_ENABLE_VALUE)
}
pub fn pool_mode() -> u16 {
    u16v(b"pool_mode", 0)
}
pub fn pool_balance() -> u16 {
    u16v(b"pool_balance", 50)
}
pub fn fan_speed() -> u16 {
    u16v(b"fanspeed", k::FAN_SPEED)
}
pub fn overheat_temp() -> u16 {
    u16v(b"overheat_temp", k::OVERHEAT_TEMP)
}
pub fn fan_manual_speed(ch: usize) -> u16 {
    if ch == 0 {
        u16v(b"fanspeed", k::FAN_SPEED)
    } else {
        u16v(b"fan1speed", 100)
    }
}
pub fn fan_overheat_temp(ch: usize) -> u16 {
    if ch == 0 {
        u16v(b"overheat_temp", k::OVERHEAT_TEMP)
    } else {
        u16v(b"fan1_overheat", k::OVERHEAT_TEMP)
    }
}
pub fn fan_pid_use_max() -> bool {
    flag(b"fan_pid_max", 1)
}
pub fn invert_screen() -> bool {
    flag(b"invertscreen", 0)
}
pub fn auto_screen_off() -> bool {
    flag(b"autoscreenoff", k::AUTO_SCREEN_OFF_VALUE)
}
pub fn show_block_found() -> bool {
    flag(b"block_found_en", k::SHOW_BLOCK_FOUND_ENABLE_VALUE)
}
pub fn otp_enabled() -> bool {
    flag(b"otp_enabled", 0)
}
pub fn can_enabled() -> bool {
    flag(b"can_enabled", 0)
}
pub fn mempool_custom() -> bool {
    flag(b"mempool_cust", 0)
}
pub fn mempool_url() -> CString {
    s(b"mempool_url", k::MEMPOOL_URL)
}
pub fn influx_enabled() -> bool {
    flag(b"influx_enable", k::INFLUX_ENABLE_VALUE)
}
pub fn influx_url() -> CString {
    s(b"influx_url", k::INFLUX_URL)
}
pub fn influx_port() -> u16 {
    u16v(b"influx_port", k::INFLUX_PORT)
}
pub fn influx_token() -> CString {
    s(b"influx_token", k::INFLUX_TOKEN)
}
pub fn influx_bucket() -> CString {
    s(b"influx_bucket", k::INFLUX_BUCKET)
}
pub fn influx_org() -> CString {
    s(b"influx_org", k::INFLUX_ORG)
}
pub fn influx_prefix() -> CString {
    s(b"influx_prefix", k::INFLUX_PREFIX)
}
