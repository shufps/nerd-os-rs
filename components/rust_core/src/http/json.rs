//! The JSON documents of the HTTP API: the object builder (the member order
//! and number types of the former ArduinoJson code: a `float` is written
//! with 6 decimal places, a `double` with 9 unless it is a lossless float,
//! so the output is byte-identical) and the parts shared by several
//! documents (pool manager, fans, history). The documents themselves are
//! next to their handlers in `v1/` and `v2/`. Verified against the former
//! handler code by `tests/http_json.rs`.

use core::ffi::{c_int, CStr};
use std::ffi::CString;

use crate::ajson::{Json, Num};

/// An object under construction (members in insertion order).
#[derive(Default)]
pub struct Obj(pub(crate) Vec<(Vec<u8>, Json)>);

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
pub(crate) fn cstr(v: Option<&CStr>) -> Json {
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
    let more = crate::mining::history::export(start, end, current, limit, now_ms, |r| {
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

pub(crate) fn pid_obj(target: i64, p: u16, i: u16, d: u16) -> Obj {
    let mut o = Obj::new();
    o.int("targetTemp", target)
        .float("p", p as f32 / 100.0)
        .float("i", i as f32 / 100.0)
        .float("d", d as f32 / 100.0);
    o
}

pub(crate) fn u32_list(v: &[u32]) -> Vec<Json> {
    v.iter().map(|&v| Json::Num(Num::U(v as u64))).collect()
}

/// The first `n` floats of `v` (`n` <= 0: none).
pub(crate) fn floats(v: &[f32], n: c_int) -> impl Iterator<Item = f32> + '_ {
    v.iter().copied().take(n.max(0) as usize)
}
