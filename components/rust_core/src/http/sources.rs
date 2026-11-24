//! The data sources of the GET documents (board, pool manager, history,
//! fans) and the reload after a settings change, shared by `v1/` and `v2/`.

use crate::idf::idf_const;
use core::ffi::{c_int, CStr};

use crate::board::api::Info as BoardInfo;
use crate::config::cfg;
use crate::http::json::{self, FanInfo};
use crate::idf::httpd::Request;

idf_const!(pub(crate) CAP_SPIRAM: u32 = MALLOC_CAP_SPIRAM, 1 << 10);
idf_const!(pub(crate) CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

pub(crate) fn board() -> BoardInfo {
    crate::board::api::info()
}

pub(crate) fn small_core_count() -> c_int {
    crate::board::get().map_or(0, |b| b.small_core_count() as c_int)
}

pub(crate) fn stats() -> crate::pool::stats::Summary {
    crate::pool::manager::get().map_or_else(Default::default, |m| m.summary())
}

pub(crate) fn pool_manager_info() -> json::ManagerInfo {
    crate::pool::manager::get().map_or_else(Default::default, |m| m.info())
}

pub(crate) fn coinbase_info(pool: c_int) -> Option<crate::pool::coinbase_verify::Info> {
    crate::pool::manager::get().map(|m| m.coinbase_info(pool))
}

pub(crate) fn uptime_seconds() -> i64 {
    (crate::util::cutil::now_us() - crate::system::start_us()) / 1_000_000
}

pub(crate) fn now_ms() -> u64 {
    (crate::util::cutil::now_us() / 1000) as u64
}

/// The history query of the request (`?ts=..&limit=..`).
pub(crate) fn history_query(req: &mut Request, v2: bool) -> crate::http::query::HistoryQuery {
    let q = req.query(128);
    crate::http::query::history_query(q.as_deref().map(CStr::to_bytes), v2)
}

/// The fan settings of channel `ch` (as shown by the settings pages).
pub(crate) fn fan_settings(b: &BoardInfo, ch: usize) -> FanInfo {
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

/// Reloads what a settings change touches: board, fans, display, pools.
pub(crate) fn reload_settings() {
    crate::board::api::board_load_settings();
    crate::power::mgmt::fan_load_settings();
    crate::display::load_settings();
    crate::pool::manager::api::pool_mgr_load_settings();
}
