//! `GET`/`PATCH /api/v2/settings` (port of `v2/handler_v2_settings.cpp`):
//! the handlers, the document (also the source of the first settings
//! profile) and the body.

use core::ffi::c_int;
use std::ffi::CString;

use crate::ajson::Json;
use crate::cfg;
use crate::http::json::*;
use crate::http::reply::{send_doc, set_json};
use crate::http::settings::num_f32;
use crate::http::settings::{
    boolean, fans, field, flag, nonzero, screen_off_secs, string, tenths, uint, Out, Setting as S,
    Write,
};
use crate::http::sources::{board, fan_settings, reload_settings};
use crate::idf::httpd::Request;
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};

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
    /// seconds
    pub auto_screen_off_timeout: u32,
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
        .int("autoScreenOff", s.auto_screen_off as i64)
        .int("autoScreenOffTimeout", s.auto_screen_off_timeout as i64);
    d.into()
}

/// `PATCH /api/v2/settings`: the flat fields, `fans`, then `pools` (at most
/// two; entries that are not objects are skipped).
pub fn settings_v2(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.text(S::WifiSsid, 0, string(f("ssid")));
    out.text(S::WifiPass, 0, string(f("wifiPass")));
    out.text(S::Hostname, 0, string(f("hostname")));
    out.num(S::AsicVoltage, 0, nonzero(f("coreVoltage")));
    out.num(S::AsicFrequency, 0, nonzero(f("frequency")));
    out.num(S::AsicJobInterval, 0, nonzero(f("jobInterval")));
    out.num(
        S::StratumDifficulty,
        0,
        uint(f("stratumDifficulty"), 0xffff_ffff),
    );
    out.num(S::VrFrequency, 0, uint(f("vrFrequency"), 0xffff_ffff));
    out.bool(S::FlipScreen, 0, boolean(f("flipScreen")));
    out.bool(S::InvertScreen, 0, boolean(f("invertScreen")));
    out.bool(S::AutoScreenOff, 0, boolean(f("autoScreenOff")));
    out.num(
        S::AutoScreenOffTimeout,
        0,
        screen_off_secs(f("autoScreenOffTimeout")),
    );
    out.bool(S::MempoolCustom, 0, boolean(f("mempoolCustom")));
    out.text(S::MempoolUrl, 0, string(f("mempoolUrl")));
    out.bool(S::FanPolarity, 0, boolean(f("invertFanPolarity")));
    // only some boards apply it (settings_apply.cpp)
    out.bool(S::FanPidUseMax, 0, boolean(f("pidUseMax")));
    out.bool(S::StratumKeepalive, 0, flag(f("stratumKeep")));
    out.bool(S::CanEnabled, 0, flag(f("canMaster")));
    fans(doc, &mut out);

    out.num(S::PoolMode, 0, uint(f("poolMode"), 1));
    out.num(S::PoolBalance, 0, uint(f("poolBalance"), 0xffff));
    if let Some(pools) = f("pools").as_array() {
        for (i, pool) in pools.iter().take(2).enumerate() {
            let i = i as i32;
            let p = |k| field(pool, k);
            out.text(S::StratumUrl, i, string(p("url")));
            out.num(S::StratumPort, i, uint(p("port"), 0xffff));
            out.text(S::StratumUser, i, string(p("user")));
            out.text(S::StratumPass, i, string(p("password")));
            out.bool(S::StratumEnonceSubscribe, i, boolean(p("enonceSubscribe")));
            out.bool(S::StratumTls, i, boolean(p("tls")));
            out.num(S::StratumProtocol, i, uint(p("protocol"), 0xffff));
            out.text(S::Sv2AuthorityPubkey, i, string(p("sv2AuthorityPubkey")));
            out.num(S::Sv2ChannelType, i, uint(p("sv2ChannelType"), 0xffff));
            out.bool(S::Sv2RequireAuth, i, boolean(p("sv2RequireAuth")));
            out.num(
                S::CoinbaseVerifyMode,
                i,
                uint(p("coinbaseVerifyMode"), 0xffff),
            );
            out.num(
                S::CoinbaseMaxFee,
                i,
                num_f32(p("coinbaseMaxFee")).map(tenths),
            );
            out.bool(S::CoinbaseVerifyForce, i, boolean(p("coinbaseVerifyForce")));
        }
    }
    out.0
}

/// `Board::Error::COINBASE_VERIFY_FAULT`
const COINBASE_VERIFY_FAULT: c_int = 6;

pub(crate) fn get(req: &mut Request) -> EspErr {
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
    v.auto_screen_off_timeout = cfg::screen_off_secs();

    v2_settings(&v)
}

/// The same structure as `GET /api/v2/settings`, all fields optional; the
/// pools as a `pools[]` array.
pub(crate) fn patch(req: &mut Request) -> EspErr {
    let Ok(verify_changed) =
        crate::http::reply::apply_pool_settings(req, false, false, c"v2_settings")
    else {
        return ESP_FAIL;
    };

    // re-run the verification (unblocked and counted afresh if its settings
    // changed)
    if let Some(m) = crate::pool::manager::get() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_screen_off_timeout_range() {
        let get = |body: &str| {
            let doc = crate::ajson::deserialize(body.as_bytes()).unwrap();
            settings_v2(&doc)
                .into_iter()
                .find(|w| w.setting == S::AutoScreenOffTimeout)
                .map(|w| w.num)
        };
        assert_eq!(get(r#"{"autoScreenOffTimeout":300}"#), Some(300));
        assert_eq!(get(r#"{"autoScreenOffTimeout":5}"#), Some(5));
        assert_eq!(get(r#"{"autoScreenOffTimeout":3600}"#), Some(3600));
        assert_eq!(get(r#"{"autoScreenOffTimeout":4}"#), None);
        assert_eq!(get(r#"{"autoScreenOffTimeout":3601}"#), None);
        assert_eq!(get(r#"{"autoScreenOffTimeout":"60"}"#), None);
    }
}
