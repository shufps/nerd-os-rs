//! The v1 documents: `GET /api/system/info` and `GET /api/system/asic`.
//!
//! Difference: `coreVoltageActual` of a NaN or out-of-range output voltage is
//! 0 or saturated; the C++ `(int)` cast of such a float was undefined.

use core::ffi::c_int;
use std::ffi::CString;

use crate::ajson::Json;
use crate::http::json::{
    float, floats, history, manager_info, pid_obj, u32_list, FanInfo, ManagerInfo, Obj,
};

/// `system_info_t`: the values of `GET /api/system/info`.
#[derive(Clone, Debug, Default)]
pub struct SystemInfo {
    pub asic_count: c_int,
    pub small_core_count: c_int,
    pub device_model: Option<CString>,
    pub hostip: Option<CString>,
    pub mac_addr: Option<CString>,
    pub wifi_rssi: c_int,
    pub power: f32,
    pub max_power: f32,
    pub min_power: f32,
    pub max_voltage: f32,
    pub min_voltage: f32,
    /// mA
    pub current: f32,
    pub min_current_a: f32,
    pub max_current_a: f32,
    pub temp: f32,
    pub vr_temp: f32,
    pub vr_temp_int: f32,
    pub hashrate_timestamp: u64,
    pub shutdown: bool,
    pub hashrate: f32,
    pub hashrate_1m: f64,
    pub hashrate_10m: f64,
    pub hashrate_1h: f64,
    pub hashrate_1d: f64,
    pub core_voltage: c_int,
    pub default_core_voltage: c_int,
    pub vout: f32,
    pub manual_fan_speed: u16,
    pub fan_count: c_int,
    pub fans: [FanInfo; 2],
    pub last_ping_rtt: f64,
    pub recent_ping_loss: f64,
    pub duplicate_hw_nonces: u64,
    pub pool_difficulty: u32,
    pub network_difficulty: f64,
    pub found_blocks: u32,
    pub total_found_blocks: u32,
    pub shares_accepted: u64,
    pub shares_rejected: u64,
    pub best_diff: u64,
    pub best_session_diff: u64,
    pub manager: ManagerInfo,
    /// `asic_count` entries
    pub asic_temps: Vec<f32>,
    pub history_requested: bool,
    pub history_start: u64,
    pub history_span_ms: u64,
    pub history_current: u64,
    pub history_limit: u32,
    pub now_ms: u64,
    pub pid_available: bool,
    /// channel 0 (`getPidSettings()`)
    pub pid_target_temp: u16,
    pub pid_p: u16,
    pub pid_i: u16,
    pub pid_d: u16,
    pub hostname: Option<CString>,
    pub ssid: Option<CString>,
    pub stratum_url: Option<CString>,
    pub stratum_port: u16,
    pub stratum_user: Option<CString>,
    pub stratum_enonce_subscribe: bool,
    pub stratum_tls: bool,
    pub fallback_stratum_url: Option<CString>,
    pub fallback_stratum_port: u16,
    pub fallback_stratum_user: Option<CString>,
    pub fallback_stratum_enonce_subscribe: bool,
    pub fallback_stratum_tls: bool,
    pub stratum_protocol: u16,
    pub fallback_stratum_protocol: u16,
    pub sv2_authority_pubkey: Option<CString>,
    pub fallback_sv2_authority_pubkey: Option<CString>,
    pub sv2_channel_type: u16,
    pub fallback_sv2_channel_type: u16,
    /// mV
    pub voltage: f32,
    pub frequency: c_int,
    pub default_frequency: c_int,
    pub job_interval: c_int,
    pub stratum_difficulty: u32,
    pub overheat_temp: u16,
    pub flip_screen: bool,
    pub invert_screen: bool,
    pub auto_screen_off: bool,
    /// seconds
    pub auto_screen_off_timeout: u32,
    pub invert_fan_polarity: bool,
    pub stratum_keep: bool,
    pub vr_frequency: u32,
    pub default_vr_frequency: u32,
    pub otp: bool,
    pub asic_model: Option<CString>,
    pub uptime_seconds: i64,
    pub last_reset_reason: Option<CString>,
    pub wifi_status: Option<CString>,
    pub free_heap: u64,
    pub free_heap_int: u64,
    pub version: Option<CString>,
    pub running_partition: Option<CString>,
    pub default_theme: Option<CString>,
}

/// `GET /api/system/info`
pub fn system_info(s: &SystemInfo) -> Json {
    let on = |v: f64| if s.shutdown { 0.0 } else { v };
    let fans = s.fan_count.clamp(0, 2) as usize;
    let fan = |ch: usize, f: fn(&FanInfo) -> u16| {
        if s.fan_count > 1 {
            f(&s.fans[ch]) as u64
        } else {
            0
        }
    };
    let mut d = Obj::new();
    d.int("asicCount", s.asic_count as i64)
        .int("smallCoreCount", s.small_core_count as i64)
        .cstr("deviceModel", s.device_model.as_deref())
        .cstr("hostip", s.hostip.as_deref())
        .cstr("macAddr", s.mac_addr.as_deref())
        .int("wifiRSSI", s.wifi_rssi as i64)
        .float("power", s.power)
        .float("maxPower", s.max_power)
        .float("minPower", s.min_power)
        .float("maxVoltage", s.max_voltage)
        .float("minVoltage", s.min_voltage)
        .float("current", s.current)
        .float("currentA", s.current / 1000.0)
        .float("minCurrentA", s.min_current_a)
        .float("maxCurrentA", s.max_current_a)
        .float("temp", s.temp)
        .float("vrTemp", s.vr_temp)
        .float("vrTempInt", s.vr_temp_int)
        .uint("hashRateTimestamp", s.hashrate_timestamp)
        .double("hashRate", on(s.hashrate as f64))
        .double("hashRate_1m", on(s.hashrate_1m))
        .double("hashRate_10m", on(s.hashrate_10m))
        .double("hashRate_1h", on(s.hashrate_1h))
        .double("hashRate_1d", on(s.hashrate_1d))
        .int("coreVoltage", s.core_voltage as i64)
        .int("defaultCoreVoltage", s.default_core_voltage as i64)
        .int("coreVoltageActual", (s.vout * 1000.0) as i32 as i64)
        .uint("fanspeed", s.fans[0].speed_perc as u64)
        .uint("manualFanSpeed", s.manual_fan_speed as u64)
        .uint("fanrpm", s.fans[0].rpm as u64)
        .uint("fanrpm2", fan(1, |f| f.rpm))
        .uint("fanspeed2", fan(1, |f| f.speed_perc))
        .int("fanCount", s.fan_count as i64)
        .double("lastpingrtt", s.last_ping_rtt)
        .double("recentpingloss", s.recent_ping_loss)
        .bool("shutdown", s.shutdown)
        .uint("duplicateHWNonces", s.duplicate_hw_nonces)
        .obj("stratum", manager_info(&s.manager, false))
        .uint("poolDifficulty", s.pool_difficulty as u64)
        .double("networkDifficulty", s.network_difficulty)
        .uint("foundBlocks", s.found_blocks as u64)
        .uint("totalFoundBlocks", s.total_found_blocks as u64)
        .uint("sharesAccepted", s.shares_accepted)
        .uint("sharesRejected", s.shares_rejected)
        .uint("bestDiff", s.best_diff)
        .uint("bestSessionDiff", s.best_session_diff);
    let temps = floats(&s.asic_temps, s.asic_count).map(float).collect();
    d.arr("asicTemps", temps);
    if !s.shutdown && s.history_requested {
        let end = s.history_start.wrapping_add(s.history_span_ms);
        d.obj(
            "history",
            history(
                s.history_start,
                end,
                s.history_current,
                s.history_limit,
                s.now_ms,
            ),
        );
    }
    let target = |t: u16| if s.pid_available { t as i64 } else { -1 };
    d.int("pidTargetTemp", target(s.pid_target_temp))
        .float("pidP", s.pid_p as f32 / 100.0)
        .float("pidI", s.pid_i as f32 / 100.0)
        .float("pidD", s.pid_d as f32 / 100.0);
    let fan_list = s.fans[..fans]
        .iter()
        .map(|f| {
            let mut o = Obj::new();
            o.cstr("label", f.label.as_deref())
                .int("mode", f.mode as i64)
                .uint("manualSpeed", f.manual_speed as u64)
                .uint("overheatTemp", f.overheat_temp as u64)
                .uint("rpm", f.rpm as u64)
                .uint("speedPerc", f.speed_perc as u64)
                .obj("pid", pid_obj(target(f.target_temp), f.p, f.i, f.d));
            o.into()
        })
        .collect();
    d.arr("fans", fan_list)
        .cstr("hostname", s.hostname.as_deref())
        .cstr("ssid", s.ssid.as_deref())
        .cstr("stratumURL", s.stratum_url.as_deref())
        .uint("stratumPort", s.stratum_port as u64)
        .cstr("stratumUser", s.stratum_user.as_deref())
        .bool("stratumEnonceSubscribe", s.stratum_enonce_subscribe)
        .bool("stratumTLS", s.stratum_tls)
        .cstr("fallbackStratumURL", s.fallback_stratum_url.as_deref())
        .uint("fallbackStratumPort", s.fallback_stratum_port as u64)
        .cstr("fallbackStratumUser", s.fallback_stratum_user.as_deref())
        .bool(
            "fallbackStratumEnonceSubscribe",
            s.fallback_stratum_enonce_subscribe,
        )
        .bool("fallbackStratumTLS", s.fallback_stratum_tls)
        .uint("stratumProtocol", s.stratum_protocol as u64)
        .uint(
            "fallbackStratumProtocol",
            s.fallback_stratum_protocol as u64,
        )
        .cstr("sv2AuthorityPubkey", s.sv2_authority_pubkey.as_deref())
        .cstr(
            "fallbackSv2AuthorityPubkey",
            s.fallback_sv2_authority_pubkey.as_deref(),
        )
        .uint("sv2ChannelType", s.sv2_channel_type as u64)
        .uint("fallbackSv2ChannelType", s.fallback_sv2_channel_type as u64)
        .float("voltage", s.voltage)
        .int("frequency", s.frequency as i64)
        .int("defaultFrequency", s.default_frequency as i64)
        .int("jobInterval", s.job_interval as i64)
        .uint("stratumDifficulty", s.stratum_difficulty as u64)
        .uint("overheat_temp", s.overheat_temp as u64)
        .int("flipscreen", s.flip_screen as i64)
        .int("invertscreen", s.invert_screen as i64)
        .int("autoscreenoff", s.auto_screen_off as i64)
        .int("autoscreenofftimeout", s.auto_screen_off_timeout as i64)
        .int("invertfanpolarity", s.invert_fan_polarity as i64)
        .int("autofanspeed", s.fans[0].mode as i64)
        .int("stratum_keep", s.stratum_keep as i64)
        .uint("vrFrequency", s.vr_frequency as u64)
        .uint("defaultVrFrequency", s.default_vr_frequency as u64)
        .bool("otp", s.otp)
        .cstr("ASICModel", s.asic_model.as_deref())
        .int("uptimeSeconds", s.uptime_seconds)
        .cstr("lastResetReason", s.last_reset_reason.as_deref())
        .cstr("wifiStatus", s.wifi_status.as_deref())
        .uint("freeHeap", s.free_heap)
        .uint("freeHeapInt", s.free_heap_int)
        .cstr("version", s.version.as_deref())
        .cstr("runningPartition", s.running_partition.as_deref())
        .cstr("defaultTheme", s.default_theme.as_deref());
    d.into()
}

/// `asic_info_t`: the values of `GET /api/system/asic`.
#[derive(Clone, Debug, Default)]
pub struct AsicInfo {
    pub asic_model: Option<CString>,
    pub device_model: Option<CString>,
    pub asic_count: c_int,
    pub default_frequency: c_int,
    pub default_voltage: c_int,
    pub abs_max_frequency: c_int,
    pub abs_min_voltage: c_int,
    pub abs_max_voltage: c_int,
    pub eco_frequency: c_int,
    pub eco_voltage: c_int,
    pub swarm_color: Option<CString>,
    pub frequency_options: Vec<u32>,
    pub voltage_options: Vec<u32>,
}

/// `GET /api/system/asic`
pub fn asic_info(a: &AsicInfo) -> Json {
    let mut d = Obj::new();
    d.cstr("ASICModel", a.asic_model.as_deref())
        .cstr("deviceModel", a.device_model.as_deref())
        .int("asicCount", a.asic_count as i64)
        .int("defaultFrequency", a.default_frequency as i64)
        .int("defaultVoltage", a.default_voltage as i64)
        .int("absMaxFrequency", a.abs_max_frequency as i64)
        .int("absMinVoltage", a.abs_min_voltage as i64)
        .int("absMaxVoltage", a.abs_max_voltage as i64)
        .int("ecoFrequency", a.eco_frequency as i64)
        .int("ecoVoltage", a.eco_voltage as i64)
        .cstr("swarmColor", a.swarm_color.as_deref());
    d.arr("frequencyOptions", u32_list(&a.frequency_options))
        .arr("voltageOptions", u32_list(&a.voltage_options));
    d.into()
}
