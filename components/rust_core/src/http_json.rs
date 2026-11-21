//! The JSON documents of the HTTP GET endpoints (port of the document
//! building in `main/http_server/handler_system.cpp`, the stratum manager's
//! `getManagerInfoJson()` and `History::exportHistoryData()`). The C++
//! handlers collect the values into snapshot structs through the same
//! getters as before; the documents are built here with the member order and
//! the number types of the ArduinoJson code (a `float` is written with 6
//! decimal places, a `double` with 9 unless it is a lossless float), so the
//! output is byte-identical. Verified against the
//! former handler code by `tests/http_json.rs`.
//!
//! Difference: `coreVoltageActual` of a NaN or out-of-range output voltage is
//! 0 or saturated; the C++ `(int)` cast of such a float was undefined.

use core::ffi::{c_int, CStr};
use std::ffi::CString;

use crate::ajson::{Json, Num};

/// An object under construction (members in insertion order).
#[derive(Default)]
pub struct Obj(Vec<(Vec<u8>, Json)>);

impl Obj {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&mut self, k: &str, v: Json) -> &mut Self {
        self.0.push((k.as_bytes().to_vec(), v));
        self
    }
    pub fn int(&mut self, k: &str, v: i64) -> &mut Self {
        self.set(k, int(v))
    }
    pub fn uint(&mut self, k: &str, v: u64) -> &mut Self {
        self.set(k, Json::Num(Num::U(v)))
    }
    pub fn float(&mut self, k: &str, v: f32) -> &mut Self {
        self.set(k, Json::Num(Num::F(v)))
    }
    pub fn double(&mut self, k: &str, v: f64) -> &mut Self {
        self.set(k, Json::Num(Num::double(v)))
    }
    pub fn bool(&mut self, k: &str, v: bool) -> &mut Self {
        self.set(k, Json::Bool(v))
    }
    /// A C string; `None` is `null`.
    pub(crate) fn cstr(&mut self, k: &str, v: Option<&CStr>) -> &mut Self {
        self.set(k, cstr(v))
    }
    pub fn obj(&mut self, k: &str, v: Obj) -> &mut Self {
        self.set(k, v.into())
    }
    pub fn arr(&mut self, k: &str, v: Vec<Json>) -> &mut Self {
        self.set(k, Json::Arr(v))
    }
}

impl From<Obj> for Json {
    fn from(o: Obj) -> Json {
        Json::Obj(o.0)
    }
}

pub fn int(v: i64) -> Json {
    if v < 0 {
        Json::Num(Num::I(v))
    } else {
        Json::Num(Num::U(v as u64))
    }
}

pub fn float(v: f32) -> Json {
    Json::Num(Num::F(v))
}

/// A C string as ArduinoJson stores it: `None` (NULL) is `null`.
fn cstr(v: Option<&CStr>) -> Json {
    v.map_or(Json::Null, |v| Json::Str(v.to_bytes().to_vec()))
}

/// One pool of `manager_info_t`.
#[derive(Clone, Debug, Default)]
pub struct ManagerPool {
    pub connected: bool,
    pub accepted: u64,
    pub rejected: u64,
    pub best_session: u64,
    pub network_difficulty: f64,
    pub pool_difficulty: u32,
    pub pool_diff_err: bool,
    /// 0 without a ping task
    pub ping_rtt: f64,
    pub ping_loss: f64,
    /// `None`: not blocked
    pub verify_blocked: Option<CString>,
    pub protocol: c_int,
    pub encrypted: bool,
}

/// `manager_info_t`: what `getManagerInfoJson()` reported.
#[derive(Clone, Debug, Default)]
pub struct ManagerInfo {
    pub config_pool_mode: u16,
    pub pool_mode: c_int,
    pub pool_balance: u16,
    pub total_best_diff: u64,
    pub fallback: bool,
    pub using_fallback: bool,
    pub dual: bool,
    pub selected: c_int,
    pub pools: [ManagerPool; 2],
    /// failover: the shared counters
    pub accepted: u64,
    pub rejected: u64,
    pub best_session: u64,
    pub network_difficulty: f64,
    pub pool_difficulty: u32,
}

/// The stratum manager object; `verbose`: the v2 dashboard (both pools in
/// failover mode and the dashboard-only fields).
pub fn manager_info(m: &ManagerInfo, verbose: bool) -> Obj {
    let mut o = Obj::new();
    o.uint("poolMode", m.config_pool_mode as u64)
        .int("activePoolMode", m.pool_mode as i64)
        .uint("poolBalance", m.pool_balance as u64)
        .uint("totalBestDiff", m.total_best_diff);
    if m.fallback {
        o.bool("usingFallback", m.using_fallback);
    }
    let mut pools = Vec::new();
    for (i, p) in m.pools.iter().enumerate() {
        let active = m.dual || i as c_int == m.selected;
        if !verbose && !active {
            continue;
        }
        let mut pool = Obj::new();
        pool.bool("connected", p.connected);
        if m.dual {
            pool.uint("poolDifficulty", p.pool_difficulty as u64)
                .double("networkDifficulty", p.network_difficulty)
                .bool("poolDiffErr", p.pool_diff_err)
                .uint("accepted", p.accepted)
                .uint("rejected", p.rejected);
        }
        pool.double("pingRtt", p.ping_rtt)
            .double("pingLoss", p.ping_loss);
        if m.dual {
            pool.uint("bestDiff", p.best_session);
        }
        if verbose {
            pool.bool("active", active)
                .cstr(
                    "verifyBlocked",
                    Some(p.verify_blocked.as_deref().unwrap_or(c"")),
                )
                .int("activeProtocol", p.protocol as i64)
                .bool("encrypted", p.encrypted);
        }
        if !m.dual && active {
            pool.uint("poolDifficulty", m.pool_difficulty as u64)
                .double("networkDifficulty", m.network_difficulty)
                .bool("poolDiffErr", false)
                .uint("accepted", m.accepted)
                .uint("rejected", m.rejected)
                .uint("bestDiff", m.best_session);
        }
        pools.push(pool.into());
    }
    o.arr("pools", pools);
    o
}

/// The `history` object between the client timestamps.
pub fn history(start: u64, end: u64, current: u64, limit: u32, now_ms: u64) -> Obj {
    let mut cols: [Vec<Json>; 7] = Default::default();
    let more = crate::history::export(start, end, current, limit, now_ms, |r| {
        for (col, v) in cols.iter_mut().zip([
            r.hashrate_1m as i64,
            r.hashrate_10m as i64,
            r.hashrate_1h as i64,
            r.hashrate_1d as i64,
            r.vreg_temp as i64,
            r.asic_temp as i64,
            r.timestamp,
        ]) {
            col.push(int(v));
        }
    });
    let [h1m, h10m, h1h, h1d, vreg, asic, ts] = cols;
    let mut o = Obj::new();
    o.arr("hashrate_1m", h1m)
        .arr("hashrate_10m", h10m)
        .arr("hashrate_1h", h1h)
        .arr("hashrate_1d", h1d)
        .arr("timestamps", ts)
        .arr("vregTemp", vreg)
        .arr("asicTemp", asic)
        .uint("timestampBase", start)
        .bool("hasMore", more);
    o
}

/// `fan_info_t`: one fan channel.
#[derive(Clone, Debug, Default)]
pub struct FanInfo {
    pub label: Option<CString>,
    pub mode: c_int,
    pub manual_speed: u16,
    pub overheat_temp: u16,
    pub rpm: u16,
    pub speed_perc: u16,
    pub target_temp: u16,
    pub p: u16,
    pub i: u16,
    pub d: u16,
}

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

fn pid_obj(target: i64, p: u16, i: u16, d: u16) -> Obj {
    let mut o = Obj::new();
    o.int("targetTemp", target)
        .float("p", p as f32 / 100.0)
        .float("i", i as f32 / 100.0)
        .float("d", d as f32 / 100.0);
    o
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

fn u32_list(v: &[u32]) -> Vec<Json> {
    v.iter().map(|&v| Json::Num(Num::U(v as u64))).collect()
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

/// One pool's coinbase data of the dashboard.
#[derive(Clone, Debug, Default)]
pub struct DashCoinbase {
    pub block_height: u32,
    pub network_difficulty: f64,
    pub scriptsig: CString,
    pub total_value_satoshis: u64,
    pub user_value_satoshis: u64,
    pub verification_ok: bool,
    pub fail_count: u32,
    pub check_count: u32,
    pub verify_mode: u16,
    /// tenths of a percent
    pub max_fee: u16,
    pub force: bool,
}

/// `dashboard_t`: the values of `GET /api/v2/dashboard`.
#[derive(Clone, Debug, Default)]
pub struct Dashboard {
    pub uptime_seconds: i64,
    pub shutdown: bool,
    pub board_error: c_int,
    pub overheat_temp: u16,
    /// fan channel 1's shutdown temperature (the VR)
    pub vr_overheat_temp: u16,
    pub hashrate_timestamp: u64,
    pub hashrate: f32,
    pub hashrate_1m: f64,
    pub hashrate_10m: f64,
    pub hashrate_1h: f64,
    pub hashrate_1d: f64,
    pub best_diff: u64,
    pub best_session_diff: u64,
    pub shares_accepted: u64,
    pub shares_rejected: u64,
    pub frequency: c_int,
    pub asic_count: c_int,
    pub small_core_count: c_int,
    /// `asic_count` entries each
    pub chip_hashrate: Vec<f32>,
    pub chip_error_rate: Vec<f32>,
    pub asic_temps: Vec<f32>,
    pub watts: f32,
    pub min_power: f32,
    pub max_power: f32,
    /// mV
    pub voltage: f32,
    pub min_voltage: f32,
    pub max_voltage: f32,
    /// mA
    pub current: f32,
    pub min_current_a: f32,
    pub max_current_a: f32,
    pub vout: f32,
    pub asic_temp: f32,
    pub vr_temp: f32,
    pub vr_temp_int: f32,
    pub fan_count: c_int,
    pub fan_speed: [u16; 2],
    pub fan_rpm: [u16; 2],
    pub manager: ManagerInfo,
    pub pool_host: [Option<CString>; 2],
    pub pool_port: [c_int; 2],
    pub pool_user: [Option<CString>; 2],
    pub has_can_extension: bool,
    pub can_enabled: bool,
    pub coinbase: [DashCoinbase; 2],
    pub history_requested: bool,
    pub history_start: u64,
    pub history_span_ms: u64,
    pub history_current: u64,
    pub history_limit: u32,
    pub now_ms: u64,
}

/// The first `n` floats of `v` (`n` <= 0: none).
fn floats(v: &[f32], n: c_int) -> impl Iterator<Item = f32> + '_ {
    v.iter().copied().take(n.max(0) as usize)
}

/// `GET /api/v2/dashboard`
pub fn dashboard(s: &Dashboard) -> Json {
    let on = |v: f64| if s.shutdown { 0.0 } else { v };
    let mut d = Obj::new();

    let mut sys = Obj::new();
    sys.int("uptime", s.uptime_seconds)
        .bool("shutdown", s.shutdown)
        .int("boardError", s.board_error as i64)
        .uint("overheatTemp", s.overheat_temp as u64)
        // the VR shutdown runs on fan channel 1: boards with two channels
        .uint(
            "vrOverheatTemp",
            if s.fan_count > 1 {
                s.vr_overheat_temp as u64
            } else {
                0
            },
        );
    d.obj("system", sys);

    let mut perf = Obj::new();
    perf.uint("hashRateTimestamp", s.hashrate_timestamp)
        .double("hashRate", on(s.hashrate as f64))
        .double("hashRate1m", on(s.hashrate_1m))
        .double("hashRate10m", on(s.hashrate_10m))
        .double("hashRate1h", on(s.hashrate_1h))
        .double("hashRate1d", on(s.hashrate_1d))
        .uint("bestDiff", s.best_diff)
        .uint("bestSessionDiff", s.best_session_diff)
        .uint("sharesAccepted", s.shares_accepted)
        .uint("sharesRejected", s.shares_rejected)
        .int("frequency", s.frequency as i64)
        .int("asicCount", s.asic_count as i64)
        .int("smallCoreCount", s.small_core_count as i64);
    let chips = floats(&s.chip_hashrate, s.asic_count)
        .zip(floats(&s.chip_error_rate, s.asic_count))
        .map(|(h, e)| {
            let mut c = Obj::new();
            c.double("hashRate", on(h as f64))
                .double("errorHashRate", on(e as f64));
            c.into()
        })
        .collect();
    perf.arr("chips", chips);
    d.obj("performance", perf);

    let mut pwr = Obj::new();
    pwr.float("watts", s.watts)
        .float("min", s.min_power)
        .float("max", s.max_power)
        .float("voltage", s.voltage / 1000.0)
        .float("voltageMin", s.min_voltage)
        .float("voltageMax", s.max_voltage)
        .float("currentA", s.current / 1000.0)
        .float("currentAMin", s.min_current_a)
        .float("currentAMax", s.max_current_a)
        .float("coreVoltageActual", s.vout);
    d.obj("power", pwr);

    let mut therm = Obj::new();
    therm
        .float("asicTemp", s.asic_temp)
        .float("vrTemp", s.vr_temp)
        .float("vrTempInt", s.vr_temp_int);
    therm.arr(
        "asicTemps",
        floats(&s.asic_temps, s.asic_count).map(float).collect(),
    );
    let fans = (0..s.fan_count.clamp(0, 2) as usize)
        .map(|ch| {
            let mut f = Obj::new();
            f.uint("speed", s.fan_speed[ch] as u64)
                .uint("rpm", s.fan_rpm[ch] as u64);
            f.into()
        })
        .collect();
    therm.arr("fans", fans);
    d.obj("thermal", therm);

    let mut stratum = manager_info(&s.manager, true);
    if let Some((_, Json::Arr(pools))) = stratum.0.iter_mut().find(|(k, _)| k == b"pools") {
        for (i, pool) in pools.iter_mut().take(2).enumerate() {
            if let Json::Obj(members) = pool {
                members.push((b"host".to_vec(), cstr(s.pool_host[i].as_deref())));
                members.push((b"port".to_vec(), int(s.pool_port[i] as i64)));
                members.push((b"user".to_vec(), cstr(s.pool_user[i].as_deref())));
            }
        }
    }
    d.obj("stratum", stratum);

    let mut can = Obj::new();
    can.bool("hasExtension", s.has_can_extension)
        .bool("enabled", s.can_enabled);
    d.obj("can", can);

    let mut headers = Vec::new();
    for (p, cb) in s.coinbase.iter().enumerate() {
        if cb.block_height == 0 {
            continue;
        }
        let mut bh = Obj::new();
        bh.int("pool", p as i64)
            .uint("blockHeight", cb.block_height as u64)
            .double("networkDifficulty", cb.network_difficulty)
            .cstr("scriptSig", Some(&cb.scriptsig));
        if cb.verify_mode > 0 {
            bh.uint("coinbaseValueTotalSatoshis", cb.total_value_satoshis)
                .uint("coinbaseValueUserSatoshis", cb.user_value_satoshis)
                .bool("verificationOk", cb.verification_ok)
                .uint("verificationFailCount", cb.fail_count as u64)
                .uint("verificationCheckCount", cb.check_count as u64);
        }
        headers.push(bh.into());
    }
    let cb_pools = s
        .coinbase
        .iter()
        .map(|cb| {
            let mut cp = Obj::new();
            cp.uint("mode", cb.verify_mode as u64)
                .float("maxFee", cb.max_fee as f32 / 10.0)
                .bool("force", cb.force);
            cp.into()
        })
        .collect();
    let mut coinbase = Obj::new();
    coinbase.arr("blockHeaders", headers).arr("pools", cb_pools);
    d.obj("coinbase", coinbase);

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
    d.into()
}

/// `v2_system_t`: the values of `GET /api/v2/system`.
#[derive(Clone, Debug, Default)]
pub struct V2System {
    pub device_model: Option<CString>,
    pub asic_model: Option<CString>,
    pub version: Option<CString>,
    pub uptime_seconds: i64,
    pub last_reset_reason: Option<CString>,
    pub hostname: Option<CString>,
    pub ssid: Option<CString>,
    pub mac_addr: Option<CString>,
    pub ip_addr: Option<CString>,
    pub wifi_status: Option<CString>,
    pub wifi_rssi: c_int,
    pub free_heap: u64,
    pub free_heap_int: u64,
}

/// `GET /api/v2/system`
pub fn v2_system(s: &V2System) -> Json {
    let mut d = Obj::new();
    d.cstr("deviceModel", s.device_model.as_deref())
        .cstr("asicModel", s.asic_model.as_deref())
        .cstr("version", s.version.as_deref())
        .int("uptimeSeconds", s.uptime_seconds)
        .cstr("lastResetReason", s.last_reset_reason.as_deref());
    let mut net = Obj::new();
    net.cstr("hostname", s.hostname.as_deref())
        .cstr("ssid", s.ssid.as_deref())
        .cstr("macAddr", s.mac_addr.as_deref())
        .cstr("ipAddr", s.ip_addr.as_deref())
        .cstr("wifiStatus", s.wifi_status.as_deref())
        .int("wifiRSSI", s.wifi_rssi as i64);
    d.obj("network", net);
    let mut mem = Obj::new();
    mem.uint("freeHeap", s.free_heap)
        .uint("freeHeapInt", s.free_heap_int);
    d.obj("memory", mem);
    d.into()
}

/// `v2_identify_t`: the values of `GET /api/v2/identify`.
#[derive(Clone, Debug, Default)]
pub struct V2Identify {
    pub device_model: Option<CString>,
    pub default_theme: Option<CString>,
    pub otp: bool,
    pub ap_active: bool,
    pub core_dump_download: bool,
    pub can_enabled: bool,
}

/// `GET /api/v2/identify`
pub fn v2_identify(s: &V2Identify) -> Json {
    let mut d = Obj::new();
    d.cstr("deviceModel", s.device_model.as_deref())
        .cstr("defaultTheme", s.default_theme.as_deref())
        .bool("otp", s.otp)
        .bool("apActive", s.ap_active)
        .bool("coreDumpDownload", s.core_dump_download);
    let mut can = Obj::new();
    can.bool("enabled", s.can_enabled);
    d.obj("can", can);
    d.into()
}

/// `alert_info_t`: `GET /api/alert/info`.
#[derive(Clone, Debug, Default)]
pub struct AlertInfo {
    pub has_webhook: bool,
    pub watchdog: bool,
    pub block_found: bool,
    pub best_diff: bool,
    pub coinbase_verify: bool,
    pub show_block_found_screen: bool,
}

pub fn alert_info(a: &AlertInfo) -> Json {
    let mut d = Obj::new();
    d.bool("hasWebhook", a.has_webhook)
        .int("watchdogEnable", a.watchdog as i64)
        .int("blockFoundEnable", a.block_found as i64)
        .int("bestDiffEnable", a.best_diff as i64)
        .int("coinbaseVerifyEnable", a.coinbase_verify as i64)
        .int("showBlockFoundScreen", a.show_block_found_screen as i64);
    d.into()
}

/// `influx_info_t`: `GET /api/influx/info`.
#[derive(Clone, Debug, Default)]
pub struct InfluxInfo {
    pub url: Option<CString>,
    pub port: u16,
    pub bucket: Option<CString>,
    pub org: Option<CString>,
    pub prefix: Option<CString>,
    pub enabled: bool,
}

pub fn influx_info(i: &InfluxInfo) -> Json {
    let mut d = Obj::new();
    d.cstr("url", i.url.as_deref())
        .uint("port", i.port as u64)
        .cstr("bucket", i.bucket.as_deref())
        .cstr("org", i.org.as_deref())
        .cstr("prefix", i.prefix.as_deref())
        .int("enabled", i.enabled as i64);
    d.into()
}

/// `GET /api/otp/status`
pub fn otp_status(enabled: bool) -> Json {
    let mut d = Obj::new();
    d.bool("enabled", enabled);
    d.into()
}

/// The OTP session response: the token, its lifetime and expiry.
pub fn otp_session(token: &[u8], ttl_seconds: u32, expires_ms: u64) -> Json {
    let mut d = Obj::new();
    d.set("token", Json::Str(token.to_vec()))
        .uint("ttlMs", ttl_seconds.wrapping_mul(1000) as u64)
        .double("expiresAt", expires_ms as f64);
    d.into()
}

/// `wifi_network_t`: one scanned access point.
#[derive(Clone, Debug, Default)]
pub struct WifiNetwork {
    pub ssid: CString,
    pub rssi: i8,
    pub authmode: c_int,
}

/// `GET /api/wifi/scan`
pub fn wifi_scan(nets: &[WifiNetwork]) -> Json {
    let list = nets
        .iter()
        .map(|n| {
            let mut o = Obj::new();
            o.cstr("ssid", Some(&n.ssid))
                .int("rssi", n.rssi as i64)
                .int("authmode", n.authmode as i64);
            o.into()
        })
        .collect();
    let mut d = Obj::new();
    d.arr("networks", list);
    d.into()
}

/// One pool of `GET /api/v2/settings`.
#[derive(Clone, Debug, Default)]
pub struct SettingsPool {
    pub url: Option<CString>,
    pub port: u16,
    pub user: Option<CString>,
    pub enonce_subscribe: bool,
    pub tls: bool,
    pub protocol: u16,
    pub sv2_authority_pubkey: Option<CString>,
    pub sv2_channel_type: u16,
    pub sv2_require_auth: bool,
    pub coinbase_verify_mode: u16,
    /// tenths of a percent
    pub coinbase_max_fee: u16,
    pub coinbase_verify_force: bool,
}

/// `v2_settings_t`: the values of `GET /api/v2/settings`.
#[derive(Clone, Debug, Default)]
pub struct V2Settings {
    pub asic_model: Option<CString>,
    pub device_model: Option<CString>,
    pub version: Option<CString>,
    pub otp: bool,
    pub ap_active: bool,
    pub has_can_extension: bool,
    pub can_enabled: bool,
    pub frequency: c_int,
    pub core_voltage: c_int,
    pub vr_frequency: u32,
    pub default_frequency: c_int,
    pub default_core_voltage: c_int,
    pub default_vr_frequency: u32,
    pub eco_frequency: c_int,
    pub eco_core_voltage: c_int,
    pub abs_min_core_voltage: c_int,
    pub abs_max_core_voltage: c_int,
    pub frequency_options: Vec<u32>,
    pub voltage_options: Vec<u32>,
    pub pool_mode: u16,
    pub pool_balance: u16,
    pub stratum_keep: bool,
    pub job_interval: c_int,
    pub stratum_difficulty: u32,
    pub pools: [SettingsPool; 2],
    pub pid_available: bool,
    pub fan_count: c_int,
    pub fans: [FanInfo; 2],
    pub invert_fan_polarity: bool,
    /// NerdAxe/NerdAxeGamma builds report `pidUseMax`
    pub has_pid_use_max: bool,
    pub pid_use_max: bool,
    pub hostname: Option<CString>,
    pub ssid: Option<CString>,
    pub mempool_custom: bool,
    pub mempool_url: Option<CString>,
    pub flip_screen: bool,
    pub invert_screen: bool,
    pub auto_screen_off: bool,
}

/// `GET /api/v2/settings`
pub fn v2_settings(s: &V2Settings) -> Json {
    let mut d = Obj::new();
    d.cstr("asicModel", s.asic_model.as_deref())
        .cstr("deviceModel", s.device_model.as_deref())
        .cstr("version", s.version.as_deref())
        .bool("otp", s.otp)
        .bool("apActive", s.ap_active);
    let mut can = Obj::new();
    can.bool("hasExtension", s.has_can_extension)
        .bool("enabled", s.can_enabled);
    d.obj("can", can)
        .int("frequency", s.frequency as i64)
        .int("coreVoltage", s.core_voltage as i64)
        .uint("vrFrequency", s.vr_frequency as u64)
        .int("defaultFrequency", s.default_frequency as i64)
        .int("defaultCoreVoltage", s.default_core_voltage as i64)
        .uint("defaultVrFrequency", s.default_vr_frequency as u64)
        .int("ecoFrequency", s.eco_frequency as i64)
        .int("ecoCoreVoltage", s.eco_core_voltage as i64)
        .int("absMinCoreVoltage", s.abs_min_core_voltage as i64)
        .int("absMaxCoreVoltage", s.abs_max_core_voltage as i64);
    d.arr("frequencyOptions", u32_list(&s.frequency_options))
        .arr("voltageOptions", u32_list(&s.voltage_options))
        .uint("poolMode", s.pool_mode as u64)
        .uint("poolBalance", s.pool_balance as u64)
        .int("stratumKeep", s.stratum_keep as i64)
        .int("jobInterval", s.job_interval as i64)
        .uint("stratumDifficulty", s.stratum_difficulty as u64);
    let pools = s
        .pools
        .iter()
        .map(|p| {
            let mut o = Obj::new();
            o.cstr("url", p.url.as_deref())
                .uint("port", p.port as u64)
                .cstr("user", p.user.as_deref())
                .bool("enonceSubscribe", p.enonce_subscribe)
                .bool("tls", p.tls)
                .uint("protocol", p.protocol as u64)
                .cstr("sv2AuthorityPubkey", p.sv2_authority_pubkey.as_deref())
                .uint("sv2ChannelType", p.sv2_channel_type as u64)
                .bool("sv2RequireAuth", p.sv2_require_auth)
                .uint("coinbaseVerifyMode", p.coinbase_verify_mode as u64)
                .float("coinbaseMaxFee", p.coinbase_max_fee as f32 / 10.0)
                .bool("coinbaseVerifyForce", p.coinbase_verify_force);
            o.into()
        })
        .collect();
    d.arr("pools", pools);
    let target = |t: u16| if s.pid_available { t as i64 } else { -1 };
    let fans = s.fans[..s.fan_count.clamp(0, 2) as usize]
        .iter()
        .map(|f| {
            let mut o = Obj::new();
            o.cstr("label", f.label.as_deref())
                .int("mode", f.mode as i64)
                .uint("manualSpeed", f.manual_speed as u64)
                .uint("overheatTemp", f.overheat_temp as u64)
                .obj("pid", pid_obj(target(f.target_temp), f.p, f.i, f.d));
            o.into()
        })
        .collect();
    d.arr("fans", fans)
        .int("invertFanPolarity", s.invert_fan_polarity as i64);
    if s.has_pid_use_max {
        d.bool("pidUseMax", s.pid_use_max);
    }
    d.cstr("hostname", s.hostname.as_deref())
        .cstr("ssid", s.ssid.as_deref())
        .bool("mempoolCustom", s.mempool_custom)
        .cstr("mempoolUrl", s.mempool_url.as_deref())
        .int("flipScreen", s.flip_screen as i64)
        .int("invertScreen", s.invert_screen as i64)
        .int("autoScreenOff", s.auto_screen_off as i64);
    d.into()
}

/// `can_master_node_t`: the master board's node of `GET /api/v2/can/swarm`.
#[derive(Clone, Debug, Default)]
pub struct CanMasterNode {
    pub mac: Option<CString>,
    pub device_model: Option<CString>,
    pub version: Option<CString>,
    pub shutdown: bool,
    /// `getCurrentHashrate()` (0 in a shutdown)
    pub hashrate: f32,
    pub power: f32,
    pub temp: f32,
    pub vr_temp: f32,
    /// mA
    pub current: f32,
    pub vout: f32,
    pub board_error: c_int,
    pub free_heap_int: c_int,
    pub frequency: c_int,
    pub core_voltage: c_int,
    pub flip_screen: bool,
    pub auto_screen_off: bool,
    pub asic_temps: Vec<f32>,
    pub fan_count: c_int,
    /// label/p/i/d unused
    pub fans: [FanInfo; 2],
}

/// `GET /api/v2/can/swarm`: the fleet totals, the master and the CAN slaves
/// (from the CAN master's registry).
pub fn can_swarm(m: &CanMasterNode) -> Json {
    use crate::can_master::api as can;
    use crate::can_master::{slave_config, slave_mac, slave_telemetry};
    use crate::can_master::{CONFIG_LEN, SLAVE_MAX, TELEMETRY_LEN};

    let f32_at = |b: &[u8], at: usize| f32::from_le_bytes(b[at..at + 4].try_into().unwrap());
    let u16_at = |b: &[u8], at: usize| u16::from_le_bytes([b[at], b[at + 1]]);
    let u32_at = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());

    let master_hashrate = if m.shutdown { 0.0f32 } else { m.hashrate };
    let mut slave_hashrate = 0.0f32;
    for i in 1..SLAVE_MAX as u8 {
        if !can::can_master_is_slave_known(i) {
            continue;
        }
        if let Some(t) = slave_telemetry(i).filter(|t| t[43] == 0) {
            slave_hashrate += f32_at(&t, 39);
        }
    }
    let mut fleet = Obj::new();
    fleet
        .float("hashRate", master_hashrate + slave_hashrate)
        .float("power", m.power + can::can_master_get_slave_fleet_power());

    let mut nodes = Vec::new();
    let fan_count = m.fan_count.clamp(0, 2) as usize;
    let mut master = Obj::new();
    master
        .int("id", 0)
        .cstr("mac", m.mac.as_deref())
        .bool("active", true)
        .bool("foreign", false)
        .bool("isMaster", true)
        .cstr("deviceModel", m.device_model.as_deref())
        .cstr("version", m.version.as_deref())
        .float("hashRate", master_hashrate)
        .float("temp", m.temp)
        .float("vrTemp", m.vr_temp)
        .float("power", m.power)
        .int("current", m.current as i32 as i64)
        .int("coreVoltageActual", (m.vout * 1000.0) as i32 as i64)
        .uint("fanRpm", m.fans[0].rpm as u64)
        .uint(
            "fanRpm2",
            if m.fan_count > 1 { m.fans[1].rpm } else { 0 } as u64,
        )
        .uint("fanSpeed", m.fans[0].speed_perc as u64)
        .uint(
            "fanSpeed2",
            if m.fan_count > 1 {
                m.fans[1].speed_perc
            } else {
                0
            } as u64,
        )
        .bool("shutdown", m.shutdown)
        .int("boardError", m.board_error as i64)
        .int("freeHeapInt", m.free_heap_int as i64)
        .int("frequency", m.frequency as i64)
        .int("coreVoltage", m.core_voltage as i64)
        .bool("flipScreen", m.flip_screen)
        .bool("autoScreenOff", m.auto_screen_off);
    master.arr(
        "asicTemps",
        m.asic_temps.iter().copied().map(float).collect(),
    );
    let fans = m.fans[..fan_count]
        .iter()
        .map(|f| {
            let mut o = Obj::new();
            o.int("mode", f.mode as i64)
                .uint("manualSpeed", f.manual_speed as u64)
                .uint("overheatTemp", f.overheat_temp as u64)
                .uint("targetTemp", f.target_temp as u64)
                .uint("rpm", f.rpm as u64)
                .uint("speedPerc", f.speed_perc as u64);
            o.into()
        })
        .collect();
    master.arr("fans", fans);
    nodes.push(master.into());

    for i in 1..SLAVE_MAX as u8 {
        if !can::can_master_is_slave_known(i) {
            continue;
        }
        let mac = slave_mac(i).unwrap_or_default();
        let (t, c) = (slave_telemetry(i), slave_config(i));
        let (has_telem, has_config) = (t.is_some(), c.is_some());
        let t = t.unwrap_or([0; TELEMETRY_LEN]);
        let c = c.unwrap_or([0; CONFIG_LEN]);
        let text = |b: &[u8]| {
            let b = if has_config { b } else { &[][..] };
            Json::Str(b[..b.iter().position(|&x| x == 0).unwrap_or(b.len())].to_vec())
        };
        let mac_str = mac
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        let tf = |at: usize| if has_telem { f32_at(&t, at) } else { 0.0 };
        let tu16 = |at: usize| if has_telem { u16_at(&t, at) as u64 } else { 0 };
        let tu8 = |at: usize| if has_telem { t[at] as u64 } else { 0 };
        let cu16 = |at: usize| if has_config { u16_at(&c, at) as u64 } else { 0 };
        let cu8 = |at: usize| if has_config { c[at] as u64 } else { 0 };
        let mut node = Obj::new();
        node.int("id", i as i64)
            .set("mac", Json::Str(mac_str.into_bytes()))
            .bool("active", can::can_master_is_slave_active(i))
            .bool("foreign", can::can_master_is_slave_foreign(i))
            .set("deviceModel", text(&c[0..16]))
            .set("version", text(&c[16..32]))
            .float("hashRate", tf(39))
            .float("temp", tf(1))
            .float("vrTemp", tf(5))
            .float("power", tf(31))
            .uint("current", tu16(35))
            .uint("coreVoltageActual", tu16(37))
            .uint("fanRpm", tu16(25))
            .uint("fanRpm2", tu16(27))
            .uint("fanSpeed", tu8(29))
            .uint("fanSpeed2", tu8(30))
            .bool("shutdown", has_telem && t[43] != 0)
            .uint("boardError", tu8(44))
            .uint(
                "freeHeapInt",
                if has_telem { u32_at(&t, 45) as u64 } else { 0 },
            )
            .uint("frequency", cu16(32))
            .uint("coreVoltage", cu16(34))
            .bool("flipScreen", has_config && c[44] != 0)
            .bool("autoScreenOff", has_config && c[45] != 0);
        node.arr("asicTemps", (0..4).map(|j| float(tf(9 + 4 * j))).collect());
        let fans = (0..2)
            .map(|ch| {
                let b = 36 + 4 * ch;
                let mut f = Obj::new();
                f.uint("mode", cu8(b))
                    .uint("manualSpeed", cu8(b + 1))
                    .uint("overheatTemp", cu8(b + 3))
                    .uint("targetTemp", cu8(b + 2))
                    .uint("rpm", tu16(if ch == 0 { 25 } else { 27 }))
                    .uint("speedPerc", tu8(if ch == 0 { 29 } else { 30 }));
                f.into()
            })
            .collect();
        node.arr("fans", fans);
        nodes.push(node.into());
    }

    let mut d = Obj::new();
    d.obj("fleet", fleet).arr("nodes", nodes);
    d.into()
}

mod ffi_v2 {}

mod ffi_c3c {}

mod ffi_c3d {}
