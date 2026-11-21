//! The data handlers of the HTTP API (port of `GET_system_info()`,
//! `GET_system_asic()`, `POST_reset_stats()`, `PATCH_update_settings()` in
//! `main/http_server/handler_system.cpp`, and of
//! `v2/handler_v2_dashboard.cpp` and `v2/handler_v2_settings.cpp`): they
//! fill the snapshots of `http_json.rs` from the board, the power
//! management, the system, the pool manager and the config, the same
//! fields from the same sources as before. The core dump download stays in
//! C++ (built only with core dumps in flash).

use crate::idf::idf_const;
use core::ffi::{c_int, CStr};
use std::ffi::CString;

use crate::board::api::Info as BoardInfo;
use crate::cfg;
use crate::history::api::{history_current, history_current_timestamp};
use crate::http_json::{self, AsicInfo, Dashboard, FanInfo, SystemInfo, V2Settings};
use crate::http_routes::Handler;
use crate::idf::httpd::{HandlerFn, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::power_mgmt;

const TAG: &CStr = c"http_system";
idf_const!(CAP_SPIRAM: u32 = MALLOC_CAP_SPIRAM, 1 << 10);
idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);
/// `Board::Error::COINBASE_VERIFY_FAULT`
const COINBASE_VERIFY_FAULT: c_int = 6;

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_partition_t as Partition;
#[cfg(not(target_os = "espidf"))]
/// `esp_partition_t` (host model)
#[repr(C)]
pub struct Partition {
    pub flash_chip: *mut core::ffi::c_void,
    pub type_: c_int,
    pub subtype: c_int,
    pub address: u32,
    pub size: u32,
    pub erase_size: u32,
    pub label: [core::ffi::c_char; 17],
    pub encrypted: bool,
    pub readonly: bool,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::esp_ota_get_running_partition;
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_ota_get_running_partition() -> *const Partition {
        core::ptr::null()
    }
}

/// The Rust handler of `h`, if it is one of these.
pub fn handler(h: Handler) -> Option<HandlerFn> {
    Some(match h {
        Handler::SystemInfo => system_info,
        Handler::SystemAsic => system_asic,
        Handler::ResetStats => reset_stats,
        Handler::SettingsPatch => settings_patch,
        Handler::V2Dashboard => dashboard,
        Handler::V2SettingsGet => v2_settings_get,
        Handler::V2SettingsPatch => v2_settings_patch,
        _ => return None,
    })
}

// ---------------------------------------------------------------------------
// sources

fn board() -> BoardInfo {
    crate::board::api::info()
}

fn small_core_count() -> c_int {
    crate::board::get().map_or(0, |b| b.small_core_count() as c_int)
}

fn stats() -> crate::pool_stats::Summary {
    crate::pool_manager::get().map_or_else(Default::default, |m| m.summary())
}

fn manager_info() -> http_json::ManagerInfo {
    crate::pool_manager::get().map_or_else(Default::default, |m| m.info())
}

fn coinbase_info(pool: c_int) -> Option<crate::pool_coinbase::Info> {
    crate::pool_manager::get().map(|m| m.coinbase_info(pool))
}

fn uptime_seconds() -> i64 {
    (crate::cutil::now_us() - crate::system::start_us()) / 1_000_000
}

fn now_ms() -> u64 {
    (crate::cutil::now_us() / 1000) as u64
}

/// The history query of the request (`?ts=..&limit=..`).
fn history_query(req: &mut Request, v2: bool) -> crate::http_query::HistoryQuery {
    let q = req.query(128);
    crate::http_query::history_query(q.as_deref().map(CStr::to_bytes), v2)
}

/// The fan settings of channel `ch` (as shown by the settings pages).
fn fan_settings(b: &BoardInfo, ch: usize) -> FanInfo {
    let pid = b.pid[ch];
    FanInfo {
        label: Some(b.fan_labels[ch].into()),
        mode: b.fan_mode[ch],
        manual_speed: cfg::fan_manual_speed(ch),
        overheat_temp: cfg::fan_overheat_temp(ch),
        rpm: 0,
        speed_perc: 0,
        target_temp: pid.target_temp,
        p: pid.p,
        i: pid.i,
        d: pid.d,
    }
}

fn send_doc(req: &mut Request, doc: crate::ajson::Json) -> EspErr {
    crate::http_server::send_json(req, &doc.serialize())
}

fn set_json(req: &mut Request) {
    req.set_type(c"application/json");
}

fn running_partition() -> CString {
    // SAFETY: no arguments; returns a pointer into the static partition table or NULL
    let p = unsafe { sys::esp_ota_get_running_partition() };
    if p.is_null() {
        return CString::default();
    }
    // SAFETY: non-null and static; the label array is NUL-terminated
    unsafe { CStr::from_ptr((*p).label.as_ptr()) }.into()
}

/// Reloads what a settings change touches: board, fans, display, pools.
pub(crate) fn reload_settings() {
    crate::board::api::board_load_settings();
    crate::power_mgmt::fan_load_settings();
    crate::display::load_settings();
    crate::pool_manager::api::pool_mgr_load_settings();
}

// ---------------------------------------------------------------------------
// v1

// the snapshot is filled field by field (as the former handler)
#[allow(clippy::field_reassign_with_default)]
fn system_info(req: &mut Request) -> EspErr {
    set_json(req);
    let q = history_query(req, false);
    if q.has_current {
        log!(Info, TAG, "cur: {}", q.current);
    }
    let b = board();
    let st = stats();

    let ssid = cfg::wifi_ssid();
    let hostname = cfg::hostname();
    let urls = [cfg::stratum_url(0), cfg::stratum_url(1)];
    let users = [cfg::stratum_user(0), cfg::stratum_user(1)];
    let sv2 = [cfg::sv2_authority_pubkey(0), cfg::sv2_authority_pubkey(1)];
    let hostip = crate::system::ip();
    let wifi_status = crate::system::wifi_status();
    let version = CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let partition = running_partition();

    // at least one entry, as the former calloc()
    let asic_temps: Vec<f32> = (0..b.asic_count.max(1))
        .map(|i| {
            if i < b.asic_count {
                crate::board_core::CHIP_TEMPS.get(i)
            } else {
                0.0
            }
        })
        .collect();

    let mut s = SystemInfo::default();
    s.asic_count = b.asic_count;
    s.small_core_count = small_core_count();
    s.device_model = Some(b.device_model.into());
    s.hostip = Some(hostip);
    s.mac_addr = Some(crate::network::mac_addr().into());
    s.wifi_rssi = crate::network::wifi_rssi();

    s.power = power_mgmt::get(0);
    s.max_power = b.max_pin;
    s.min_power = b.min_pin;
    s.max_voltage = b.max_vin;
    s.min_voltage = b.min_vin;
    s.current = power_mgmt::get(2);
    s.min_current_a = b.min_current_a;
    s.max_current_a = b.max_current_a;
    s.temp = power_mgmt::get(3);
    s.vr_temp = power_mgmt::get(4);
    s.vr_temp_int = power_mgmt::get(5);
    s.hashrate_timestamp = history_current_timestamp();
    s.shutdown = power_mgmt::is_shutdown();
    s.hashrate = crate::system::current_hashrate();
    s.hashrate_1m = history_current(0);
    s.hashrate_10m = history_current(1);
    s.hashrate_1h = history_current(2);
    s.hashrate_1d = history_current(3);
    s.core_voltage = b.asic_voltage_mv;
    s.default_core_voltage = b.default_voltage_mv;
    s.vout = crate::board::api::board_power_value(3);
    s.manual_fan_speed = cfg::fan_speed();
    s.fan_count = b.num_fans;
    for ch in 0..2 {
        let mut f = FanInfo::default();
        if (ch as c_int) < s.fan_count {
            f = fan_settings(&b, ch);
        }
        f.rpm = power_mgmt::fan_get(ch as c_int, 0);
        f.speed_perc = power_mgmt::fan_get(ch as c_int, 1);
        s.fans[ch] = f;
    }
    s.fans[0].mode = b.fan_mode[0];

    s.last_ping_rtt = crate::pool_manager::api::pool_mgr_ping_rtt();
    s.recent_ping_loss = crate::pool_manager::api::pool_mgr_ping_loss();
    s.duplicate_hw_nonces = crate::mining_tasks::api::asic_result_duplicates();

    s.pool_difficulty = st.pool_difficulty;
    s.network_difficulty = st.network_difficulty;
    s.found_blocks = st.found_blocks;
    s.total_found_blocks = st.total_found_blocks;
    s.shares_accepted = st.accepted;
    s.shares_rejected = st.rejected;
    s.best_diff = st.total_best;
    s.best_session_diff = st.best_session;
    s.manager = manager_info();

    s.asic_temps = asic_temps;

    s.history_requested = q.requested;
    s.history_start = q.start;
    s.history_span_ms = q.span_ms;
    s.history_current = q.current;
    s.history_limit = q.limit;
    s.now_ms = now_ms();

    let pid = b.pid[0];
    s.pid_available = true;
    s.pid_target_temp = pid.target_temp;
    s.pid_p = pid.p;
    s.pid_i = pid.i;
    s.pid_d = pid.d;

    let [url, fallback_url] = urls;
    let [user, fallback_user] = users;
    let [sv2, fallback_sv2] = sv2;
    s.hostname = Some(hostname);
    s.ssid = Some(ssid);
    s.stratum_url = Some(url);
    s.stratum_port = cfg::stratum_port(0);
    s.stratum_user = Some(user);
    s.stratum_enonce_subscribe = cfg::stratum_enonce_subscribe(0);
    s.stratum_tls = cfg::stratum_tls(0);
    s.fallback_stratum_url = Some(fallback_url);
    s.fallback_stratum_port = cfg::stratum_port(1);
    s.fallback_stratum_user = Some(fallback_user);
    s.fallback_stratum_enonce_subscribe = cfg::stratum_enonce_subscribe(1);
    s.fallback_stratum_tls = cfg::stratum_tls(1);
    s.stratum_protocol = cfg::stratum_protocol(0);
    s.fallback_stratum_protocol = cfg::stratum_protocol(1);
    s.sv2_authority_pubkey = Some(sv2);
    s.fallback_sv2_authority_pubkey = Some(fallback_sv2);
    s.sv2_channel_type = cfg::sv2_channel_type(0);
    s.fallback_sv2_channel_type = cfg::sv2_channel_type(1);
    s.voltage = power_mgmt::get(1);
    s.frequency = b.asic_frequency;
    s.default_frequency = b.default_frequency;
    s.job_interval = b.job_interval_ms;
    s.stratum_difficulty = cfg::stratum_difficulty();
    s.overheat_temp = cfg::overheat_temp();
    s.flip_screen = b.flip_screen;
    s.invert_screen = cfg::invert_screen();
    s.auto_screen_off = cfg::auto_screen_off();
    s.invert_fan_polarity = b.fan_invert;
    s.stratum_keep = cfg::stratum_keepalive();
    s.vr_frequency = b.vr_frequency;
    s.default_vr_frequency = b.default_vr_frequency;
    s.otp = cfg::otp_enabled();

    s.asic_model = Some(b.asic_model.into());
    s.uptime_seconds = uptime_seconds();
    s.last_reset_reason = Some(crate::system::reset_reason().into());
    s.wifi_status = Some(wifi_status);
    s.free_heap = crate::idf::task::free_heap(CAP_SPIRAM) as u64;
    s.free_heap_int = crate::idf::task::free_heap(CAP_INTERNAL) as u64;
    s.version = Some(version);
    s.running_partition = Some(partition);
    s.default_theme = Some(b.default_theme.into());

    send_doc(req, http_json::system_info(&s))
}

fn system_asic(req: &mut Request) -> EspErr {
    set_json(req);
    let b = board();
    let a = AsicInfo {
        asic_model: Some(b.asic_model.into()),
        device_model: Some(b.device_model.into()),
        asic_count: b.asic_count,
        default_frequency: b.default_frequency,
        default_voltage: b.default_voltage_mv,
        abs_max_frequency: b.abs_max_frequency,
        abs_min_voltage: b.abs_min_voltage_mv,
        abs_max_voltage: b.abs_max_voltage_mv,
        eco_frequency: b.eco_frequency,
        eco_voltage: b.eco_voltage_mv,
        swarm_color: Some(b.swarm_color.into()),
        frequency_options: b.frequencies.to_vec(),
        voltage_options: b.voltages.to_vec(),
    };
    send_doc(req, http_json::asic_info(&a))
}

/// `?allTime=1` also clears the persisted all-time stats; that cannot be
/// undone, so it needs OTP (when enabled).
fn reset_stats(req: &mut Request) -> EspErr {
    let q = req.query(32);
    let all_time = crate::http_query::all_time(q.as_deref().map(CStr::to_bytes));
    if all_time && crate::http_server::validate_otp(req, false) != ESP_OK {
        return ESP_FAIL;
    }
    crate::pool_manager::api::pool_mgr_reset_session_stats();
    if all_time {
        crate::pool_manager::api::pool_mgr_reset_all_time_stats();
    }
    log!(
        Info,
        TAG,
        "{} stats reset by user",
        if all_time {
            "Session and all-time"
        } else {
            "Session"
        }
    );
    req.set_status(c"204 No Content");
    req.send(&[])
}

fn settings_patch(req: &mut Request) -> EspErr {
    let dual = crate::pool_manager::api::pool_mgr_is_dual();
    if crate::http_handlers::apply_pool_settings(req, true, dual, c"system").is_err() {
        return ESP_FAIL;
    }
    crate::config_nvs::request_flush();
    req.end_chunks();
    reload_settings();
    ESP_OK
}

// ---------------------------------------------------------------------------
// v2

// the snapshot is filled field by field (as the former handler)
#[allow(clippy::field_reassign_with_default)]
fn dashboard(req: &mut Request) -> EspErr {
    set_json(req);
    let q = history_query(req, true);
    let b = board();
    let st = stats();
    let urls = [cfg::stratum_url(0), cfg::stratum_url(1)];
    let users = [cfg::stratum_user(0), cfg::stratum_user(1)];

    let mut d = Dashboard::default();
    d.uptime_seconds = uptime_seconds();
    d.shutdown = power_mgmt::is_shutdown();
    d.board_error = crate::system::error().0;
    d.overheat_temp = cfg::overheat_temp();
    d.vr_overheat_temp = cfg::fan_overheat_temp(1);

    d.hashrate_timestamp = history_current_timestamp();
    d.hashrate = crate::system::current_hashrate();
    d.hashrate_1m = history_current(0);
    d.hashrate_10m = history_current(1);
    d.hashrate_1h = history_current(2);
    d.hashrate_1d = history_current(3);
    d.best_diff = st.total_best;
    d.best_session_diff = st.best_session;
    d.shares_accepted = st.accepted;
    d.shares_rejected = st.rejected;
    d.frequency = b.asic_frequency;
    d.asic_count = b.asic_count;
    d.small_core_count = small_core_count();

    // per chip: valid and error hashrate (GH/s, the ASIC counters), temp
    for i in 0..b.asic_count.max(0) {
        d.chip_hashrate
            .push(crate::hashrate::api::hashrate_monitor_chip(i));
        d.chip_error_rate
            .push(crate::hashrate::api::hashrate_monitor_chip_errors(i));
        d.asic_temps.push(crate::board_core::CHIP_TEMPS.get(i));
    }

    d.watts = power_mgmt::get(0);
    d.min_power = b.min_pin;
    d.max_power = b.max_pin;
    d.voltage = power_mgmt::get(1);
    d.min_voltage = b.min_vin;
    d.max_voltage = b.max_vin;
    d.current = power_mgmt::get(2);
    d.min_current_a = b.min_current_a;
    d.max_current_a = b.max_current_a;
    d.vout = crate::board::api::board_power_value(3);

    d.asic_temp = power_mgmt::get(3);
    d.vr_temp = power_mgmt::get(4);
    d.vr_temp_int = power_mgmt::get(5);
    d.fan_count = b.num_fans;
    for ch in 0..2.min(d.fan_count.max(0) as usize) {
        d.fan_speed[ch] = power_mgmt::fan_get(ch as c_int, 1);
        d.fan_rpm[ch] = power_mgmt::fan_get(ch as c_int, 0);
    }

    d.manager = manager_info();
    d.pool_host = urls.map(Some);
    d.pool_user = users.map(Some);
    d.pool_port = [0, 1].map(|i| cfg::stratum_port(i) as c_int);

    d.has_can_extension = crate::board::api::board_has_can_extension();
    d.can_enabled = cfg::can_enabled();

    let cbs = [coinbase_info(0), coinbase_info(1)];
    for (p, info) in cbs.iter().enumerate() {
        let c = &mut d.coinbase[p];
        c.verify_mode = cfg::coinbase_verify_mode(p);
        c.max_fee = cfg::coinbase_max_fee(p);
        c.force = cfg::coinbase_verify_force(p);
        // nothing decoded before the manager exists
        let Some(info) = info else { continue };
        let r = &info.result;
        c.block_height = r.block_height;
        c.network_difficulty = r.network_difficulty;
        c.scriptsig = CString::new(r.scriptsig_bytes()).unwrap_or_default();
        c.total_value_satoshis = r.total_value_satoshis;
        c.user_value_satoshis = r.user_value_satoshis;
        if r.block_height > 0 && c.verify_mode > 0 {
            c.verification_ok = info.ok;
            c.fail_count = info.fail_count;
            c.check_count = info.check_count;
        }
    }

    d.history_requested = q.requested;
    d.history_start = q.start;
    d.history_span_ms = q.span_ms;
    d.history_current = q.current;
    d.history_limit = q.limit;
    d.now_ms = now_ms();

    send_doc(req, http_json::dashboard(&d))
}

fn v2_settings_get(req: &mut Request) -> EspErr {
    set_json(req);
    send_doc(req, v2_settings_doc())
}

/// The document of `GET /api/v2/settings` (also the source of the first
/// settings profile).
#[allow(clippy::field_reassign_with_default)]
pub(crate) fn v2_settings_doc() -> crate::ajson::Json {
    let b = board();
    let version = CString::new(crate::mdns::fw_version()).unwrap_or_default();
    let urls = [cfg::stratum_url(0), cfg::stratum_url(1)];
    let users = [cfg::stratum_user(0), cfg::stratum_user(1)];
    let sv2 = [cfg::sv2_authority_pubkey(0), cfg::sv2_authority_pubkey(1)];
    let hostname = cfg::hostname();
    let ssid = cfg::wifi_ssid();
    let mempool_url = cfg::mempool_url();

    let mut v = V2Settings::default();
    v.asic_model = Some(b.asic_model.into());
    v.device_model = Some(b.device_model.into());
    v.version = Some(version);
    v.otp = cfg::otp_enabled();
    v.ap_active = crate::netmgr::ap_active();

    v.has_can_extension = crate::board::api::board_has_can_extension();
    v.can_enabled = cfg::can_enabled();

    v.frequency = b.asic_frequency;
    v.core_voltage = b.asic_voltage_mv;
    v.vr_frequency = b.vr_frequency;
    v.default_frequency = b.default_frequency;
    v.default_core_voltage = b.default_voltage_mv;
    v.default_vr_frequency = b.default_vr_frequency;
    v.eco_frequency = b.eco_frequency;
    v.eco_core_voltage = b.eco_voltage_mv;
    v.abs_min_core_voltage = b.abs_min_voltage_mv;
    v.abs_max_core_voltage = b.abs_max_voltage_mv;
    v.frequency_options = b.frequencies.to_vec();
    v.voltage_options = b.voltages.to_vec();

    v.pool_mode = cfg::pool_mode();
    v.pool_balance = cfg::pool_balance();
    v.stratum_keep = cfg::stratum_keepalive();
    v.job_interval = b.job_interval_ms;
    v.stratum_difficulty = cfg::stratum_difficulty();

    for p in 0..2 {
        let pool = &mut v.pools[p];
        pool.url = Some(urls[p].clone());
        pool.user = Some(users[p].clone());
        pool.sv2_authority_pubkey = Some(sv2[p].clone());
        pool.coinbase_verify_mode = cfg::coinbase_verify_mode(p);
        pool.coinbase_max_fee = cfg::coinbase_max_fee(p);
        pool.coinbase_verify_force = cfg::coinbase_verify_force(p);
        pool.port = cfg::stratum_port(p);
        pool.enonce_subscribe = cfg::stratum_enonce_subscribe(p);
        pool.tls = cfg::stratum_tls(p);
        pool.protocol = cfg::stratum_protocol(p);
        pool.sv2_channel_type = cfg::sv2_channel_type(p);
        pool.sv2_require_auth = cfg::sv2_require_auth(p);
    }

    v.pid_available = true;
    v.fan_count = b.num_fans;
    for ch in 0..2.min(v.fan_count.max(0) as usize) {
        v.fans[ch] = fan_settings(&b, ch);
    }
    v.invert_fan_polarity = b.fan_invert;
    if crate::settings_apply::pid_use_max_board() {
        v.has_pid_use_max = true;
        v.pid_use_max = cfg::fan_pid_use_max();
    }

    v.hostname = Some(hostname);
    v.ssid = Some(ssid);
    v.mempool_custom = cfg::mempool_custom();
    v.mempool_url = Some(mempool_url);

    v.flip_screen = b.flip_screen;
    v.invert_screen = cfg::invert_screen();
    v.auto_screen_off = cfg::auto_screen_off();

    http_json::v2_settings(&v)
}

/// The same structure as `GET /api/v2/settings`, all fields optional; the
/// pools as a `pools[]` array.
fn v2_settings_patch(req: &mut Request) -> EspErr {
    let Ok(verify_changed) =
        crate::http_handlers::apply_pool_settings(req, false, false, c"v2_settings")
    else {
        return ESP_FAIL;
    };

    // re-run the verification (unblocked and counted afresh if its settings
    // changed)
    if let Some(m) = crate::pool_manager::get() {
        let cb = m.coinbase();
        for (i, &changed) in verify_changed.iter().enumerate() {
            if changed {
                cb.clear_blocked(i as c_int);
                cb.reset_counts(i as c_int);
            }
            m.rerun_verification(i as c_int);
        }
        if cb.blocked(0) == 0 && cb.blocked(1) == 0 {
            crate::system::clear_error(COINBASE_VERIFY_FAULT);
        }
    }

    crate::config_nvs::request_flush();
    req.end_chunks();
    reload_settings();
    ESP_OK
}
