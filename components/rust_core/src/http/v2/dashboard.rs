//! `GET /api/v2/dashboard` (port of `v2/handler_v2_dashboard.cpp`): the
//! handler and its document.

use core::ffi::c_int;
use std::ffi::CString;

use crate::ajson::Json;
use crate::cfg;
use crate::history::api::{history_current, history_current_timestamp};
use crate::http::json::{cstr, float, floats, history, int, manager_info, ManagerInfo, Obj};
use crate::http::reply::{send_doc, set_json};
use crate::http::sources::{
    board, coinbase_info, history_query, now_ms, pool_manager_info, small_core_count, stats,
    uptime_seconds,
};
use crate::idf::httpd::Request;
use crate::idf::EspErr;
use crate::power_mgmt;

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

// the snapshot is filled field by field (as the former handler)
#[allow(clippy::field_reassign_with_default)]
pub(crate) fn get(req: &mut Request) -> EspErr {
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
        d.asic_temps
            .push(crate::board::board_core::CHIP_TEMPS.get(i));
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

    d.manager = pool_manager_info();
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

    send_doc(req, dashboard(&d))
}
