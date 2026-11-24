//! The body of `PATCH /api/system` (v1, with `StratumManager::saveSettings()`).

use crate::http::settings::{
    boolean, fans, field, flag, hundredths, nonzero, num_f32, screen_off_secs, string, uint, Out,
    Setting as S, Write,
};
use crate::util::ajson::Json;

/// `PATCH /api/system` (v1): the flat fields, then `fans`, then the flat
/// stratum fields of `StratumManager::saveSettings()`; in dual pool mode
/// `poolBalance` before them (`StratumManagerDualPool::saveSettings()`).
pub fn settings_v1(doc: &Json, dual_pool: bool) -> Vec<Write> {
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
    out.bool(S::FlipScreen, 0, boolean(f("flipscreen")));
    out.num(S::OverheatTemp, 0, uint(f("overheat_temp"), 0xffff));
    out.bool(S::InvertScreen, 0, boolean(f("invertscreen")));
    out.bool(S::FanPolarity, 0, boolean(f("invertfanpolarity")));
    out.num(S::TempControlMode, 0, uint(f("autofanspeed"), 0xffff));
    out.num(S::FanSpeed, 0, uint(f("manualFanSpeed"), 0xffff));
    out.bool(S::AutoScreenOff, 0, boolean(f("autoscreenoff")));
    out.num(
        S::AutoScreenOffTimeout,
        0,
        screen_off_secs(f("autoscreenofftimeout")),
    );
    out.bool(S::StratumKeepalive, 0, flag(f("stratum_keep")));
    out.num(S::PidTargetTemp, 0, uint(f("pidTargetTemp"), 0xffff));
    out.num(S::PidP, 0, num_f32(f("pidP")).map(hundredths));
    out.num(S::PidI, 0, num_f32(f("pidI")).map(hundredths));
    out.num(S::PidD, 0, num_f32(f("pidD")).map(hundredths));
    out.num(S::VrFrequency, 0, uint(f("vrFrequency"), 0xffff_ffff));
    fans(doc, &mut out);

    // StratumManager::saveSettings()
    if dual_pool {
        out.num(S::PoolBalance, 0, uint(f("poolBalance"), 0xffff));
    }
    out.num(S::PoolMode, 0, uint(f("poolMode"), 1));
    for (pool, prefix) in [(0, ""), (1, "fallback")] {
        let key = |name: &str| {
            if prefix.is_empty() {
                format!("stratum{name}")
            } else {
                format!("fallbackStratum{name}")
            }
        };
        let f = |name: &str| field(doc, &key(name));
        out.text(S::StratumUrl, pool, string(f("URL")));
        out.text(S::StratumUser, pool, string(f("User")));
        out.text(S::StratumPass, pool, string(f("Password")));
        out.num(S::StratumPort, pool, uint(f("Port"), 0xffff));
        out.bool(
            S::StratumEnonceSubscribe,
            pool,
            boolean(f("EnonceSubscribe")),
        );
        out.bool(S::StratumTls, pool, boolean(f("TLS")));
    }
    out.num(S::StratumProtocol, 0, uint(f("stratumProtocol"), 0xffff));
    out.num(
        S::StratumProtocol,
        1,
        uint(f("fallbackStratumProtocol"), 0xffff),
    );
    out.text(S::Sv2AuthorityPubkey, 0, string(f("sv2AuthorityPubkey")));
    out.text(
        S::Sv2AuthorityPubkey,
        1,
        string(f("fallbackSv2AuthorityPubkey")),
    );
    out.num(S::Sv2ChannelType, 0, uint(f("sv2ChannelType"), 0xffff));
    out.num(
        S::Sv2ChannelType,
        1,
        uint(f("fallbackSv2ChannelType"), 0xffff),
    );
    out.0
}
