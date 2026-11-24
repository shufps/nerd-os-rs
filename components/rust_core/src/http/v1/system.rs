//! The v1 endpoints under `/api/system` (port of `handler_system.cpp`,
//! `handler_restart.cpp` and `handler_shutdown.cpp`): info, asic, the
//! settings patch, reset-stats, restart and shutdown (no OTP for restart and
//! shutdown on purpose: home automation).

use core::ffi::{c_int, CStr};
use std::ffi::CString;

use super::json::{self, AsicInfo, SystemInfo};
use crate::cfg;
use crate::history::api::{history_current, history_current_timestamp};
use crate::http::json::FanInfo;
use crate::http::ota::running_partition;
use crate::http::reply::{send_doc, send_str, set_json};
use crate::http::sources::{
    board, fan_settings, history_query, now_ms, pool_manager_info, reload_settings,
    small_core_count, stats, uptime_seconds, CAP_INTERNAL, CAP_SPIRAM,
};
use crate::idf::httpd::Request;
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;
use crate::power_mgmt;

pub(crate) fn restart(req: &mut Request) -> EspErr {
    log!(
        Info,
        c"http_restart",
        "Restarting System because of API Request"
    );
    send_str(req, c"System will restart shortly.");
    // let the response go out
    crate::idf::task::delay_ms(1000);
    crate::power_mgmt::restart()
}

pub(crate) fn shutdown(req: &mut Request) -> EspErr {
    log!(
        Info,
        c"http_shutdown",
        "Shutting down System because of API Request"
    );
    send_str(req, c"System will shutdown shortly.");
    crate::idf::task::delay_ms(1000);
    crate::power_mgmt::shutdown();
    ESP_OK
}

const TAG: &CStr = c"http_system";

// the snapshot is filled field by field (as the former handler)
#[allow(clippy::field_reassign_with_default)]
pub(crate) fn info(req: &mut Request) -> EspErr {
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
                crate::board::board_core::CHIP_TEMPS.get(i)
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
    s.manager = pool_manager_info();

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
    s.auto_screen_off_timeout = cfg::screen_off_secs();
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

    send_doc(req, json::system_info(&s))
}

pub(crate) fn asic(req: &mut Request) -> EspErr {
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
    send_doc(req, json::asic_info(&a))
}

/// `?allTime=1` also clears the persisted all-time stats; that cannot be
/// undone, so it needs OTP (when enabled).
pub(crate) fn reset_stats(req: &mut Request) -> EspErr {
    let q = req.query(32);
    let all_time = crate::http::query::all_time(q.as_deref().map(CStr::to_bytes));
    if all_time && crate::http::server::validate_otp(req, false) != ESP_OK {
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

pub(crate) fn patch(req: &mut Request) -> EspErr {
    let dual = crate::pool_manager::api::pool_mgr_is_dual();
    if crate::http::reply::apply_pool_settings(req, true, dual, c"system").is_err() {
        return ESP_FAIL;
    }
    crate::config_nvs::request_flush();
    req.end_chunks();
    reload_settings();
    ESP_OK
}
