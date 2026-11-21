//! Settings request bodies of the HTTP API (port of the JSON handling in
//! `PATCH /api/system` with `StratumManager::saveSettings()`, and
//! `PATCH /api/v2/settings`): the body is parsed into typed writes, which
//! `settings_apply.rs` applies to the config.
//!
//! The JSON is read with ArduinoJson's rules ([`crate::ajson`]), and
//! `is<T>()` only accepts values of that type (integers in range for integer
//! types, any number for `float`), as before.
//!
//! Behaviour notes:
//! * PID factors and the coinbase fee are converted to the register value
//!   with saturation (out of range: 0 or 65535, NaN: 0),

use crate::ajson::Json;

/// `setting_t`: one config setter each; `index` is the fan channel or pool.
#[repr(u16)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Setting {
    WifiSsid = 0,
    WifiPass = 1,
    Hostname = 2,
    AsicVoltage = 3,
    AsicFrequency = 4,
    AsicJobInterval = 5,
    StratumDifficulty = 6,
    FlipScreen = 7,
    OverheatTemp = 8,
    InvertScreen = 9,
    FanPolarity = 10,
    TempControlMode = 11,
    FanSpeed = 12,
    AutoScreenOff = 13,
    StratumKeepalive = 14,
    PidTargetTemp = 15,
    PidP = 16,
    PidI = 17,
    PidD = 18,
    VrFrequency = 19,
    FanMode = 20,
    FanManualSpeed = 21,
    FanOverheatTemp = 22,
    FanPidTargetTemp = 23,
    FanPidP = 24,
    FanPidI = 25,
    FanPidD = 26,
    PoolMode = 27,
    StratumUrl = 28,
    StratumUser = 29,
    StratumPass = 30,
    StratumPort = 31,
    StratumEnonceSubscribe = 32,
    StratumTls = 33,
    StratumProtocol = 34,
    Sv2AuthorityPubkey = 35,
    Sv2ChannelType = 36,
    MempoolCustom = 37,
    MempoolUrl = 38,
    FanPidUseMax = 39,
    CanEnabled = 40,
    PoolBalance = 41,
    Sv2RequireAuth = 42,
    CoinbaseVerifyMode = 43,
    CoinbaseMaxFee = 44,
    CoinbaseVerifyForce = 45,
    InfluxEnabled = 46,
    InfluxUrl = 47,
    InfluxPort = 48,
    InfluxToken = 49,
    InfluxBucket = 50,
    InfluxOrg = 51,
    InfluxPrefix = 52,
    DiscordWebhook = 53,
    DiscordWatchdog = 54,
    DiscordBlockFound = 55,
    DiscordBestDiff = 56,
    DiscordCoinbaseVerify = 57,
    ShowBlockFound = 58,
    /// CAN node commands (`PATCH /api/v2/can/nodes/{id}`), not config:
    CanFrequency = 60,
    CanVoltage = 61,
    /// index: fan channel; num: mode | speed << 8 | target temp << 16 | overheat << 24
    CanFan = 62,
    /// num: flip screen | auto screen off << 1
    CanDisplay = 63,
}

/// A value for a setter: numbers and bools as `num`, strings as `text`
/// (without NUL: C sees the string up to the first one).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Write {
    pub setting: Setting,
    pub index: i32,
    pub num: u32,
    pub text: Option<Vec<u8>>,
}

// ---- ArduinoJson's is<T>() / as<T>() on the parsed value ----

const NULL: Json = Json::Null;

fn field<'a>(v: &'a Json, key: &str) -> &'a Json {
    v.get(key)
}

/// `is<uintN_t>()` with the given maximum.
fn uint(v: &Json, max: u64) -> Option<u32> {
    v.as_uint(max).map(|n| n as u32)
}

fn boolean(v: &Json) -> Option<bool> {
    v.as_bool()
}

/// `is<const char *>()`, as C sees it.
fn string(v: &Json) -> Option<Vec<u8>> {
    v.as_cstr().map(<[u8]>::to_vec)
}

/// `is<float>()` (any number), `as<float>()`.
fn float(v: &Json) -> Option<f32> {
    v.as_f32()
}

/// `is<bool>() || is<int>()`, then `as<int>() != 0`.
fn flag(v: &Json) -> Option<bool> {
    v.as_bool().or_else(|| v.as_int().map(|i| i != 0))
}

/// A PID factor as stored: hundredths, saturated.
fn hundredths(f: f32) -> u32 {
    (f * 100.0) as u16 as u32
}

/// The coinbase fee as stored: tenths of a percent, rounded, saturated.
fn tenths(f: f32) -> u32 {
    (f * 10.0).round() as u16 as u32
}

struct Out(Vec<Write>);

impl Out {
    fn num(&mut self, setting: Setting, index: i32, num: Option<u32>) {
        if let Some(num) = num {
            self.0.push(Write {
                setting,
                index,
                num,
                text: None,
            });
        }
    }
    fn bool(&mut self, setting: Setting, index: i32, b: Option<bool>) {
        self.num(setting, index, b.map(u32::from));
    }
    fn text(&mut self, setting: Setting, index: i32, text: Option<Vec<u8>>) {
        if let Some(text) = text {
            self.0.push(Write {
                setting,
                index,
                num: 0,
                text: Some(text),
            });
        }
    }
}

use Setting as S;

/// The `fans` array: at most two entries (channels 0 and 1); entries that
/// are not objects still take a channel.
fn fans(doc: &Json, out: &mut Out) {
    let Some(fans) = field(doc, "fans").as_array() else {
        return;
    };
    for (ch, fan) in fans.iter().take(2).enumerate() {
        let ch = ch as i32;
        let fan = if fan.is_object() { fan } else { &NULL };
        out.num(S::FanMode, ch, uint(field(fan, "mode"), 0xffff));
        out.num(
            S::FanManualSpeed,
            ch,
            uint(field(fan, "manualSpeed"), 0xffff),
        );
        out.num(
            S::FanOverheatTemp,
            ch,
            uint(field(fan, "overheatTemp"), 0xffff),
        );
        let pid = field(fan, "pid");
        if pid.is_object() {
            out.num(
                S::FanPidTargetTemp,
                ch,
                uint(field(pid, "targetTemp"), 0xffff),
            );
            out.num(S::FanPidP, ch, float(field(pid, "p")).map(hundredths));
            out.num(S::FanPidI, ch, float(field(pid, "i")).map(hundredths));
            out.num(S::FanPidD, ch, float(field(pid, "d")).map(hundredths));
        }
    }
}

/// ASIC values the handlers only take when not 0.
fn nonzero(v: &Json) -> Option<u32> {
    uint(v, 0xffff).filter(|&n| n > 0)
}

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
    out.bool(S::StratumKeepalive, 0, flag(f("stratum_keep")));
    out.num(S::PidTargetTemp, 0, uint(f("pidTargetTemp"), 0xffff));
    out.num(S::PidP, 0, float(f("pidP")).map(hundredths));
    out.num(S::PidI, 0, float(f("pidI")).map(hundredths));
    out.num(S::PidD, 0, float(f("pidD")).map(hundredths));
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
            out.num(S::CoinbaseMaxFee, i, float(p("coinbaseMaxFee")).map(tenths));
            out.bool(S::CoinbaseVerifyForce, i, boolean(p("coinbaseVerifyForce")));
        }
    }
    out.0
}

/// `PATCH /api/v2/influx`.
pub fn settings_influx(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.bool(S::InfluxEnabled, 0, boolean(f("enabled")));
    out.text(S::InfluxUrl, 0, string(f("url")));
    out.num(S::InfluxPort, 0, uint(f("port"), 0xffff));
    out.text(S::InfluxToken, 0, string(f("token")));
    out.text(S::InfluxBucket, 0, string(f("bucket")));
    out.text(S::InfluxOrg, 0, string(f("org")));
    out.text(S::InfluxPrefix, 0, string(f("prefix")));
    out.0
}

/// `PATCH /api/v2/alert`.
pub fn settings_alert(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.text(S::DiscordWebhook, 0, string(f("webhookUrl")));
    out.bool(S::DiscordWatchdog, 0, boolean(f("watchdogEnable")));
    out.bool(S::DiscordBlockFound, 0, boolean(f("blockFoundEnable")));
    out.bool(S::DiscordBestDiff, 0, boolean(f("bestDiffEnable")));
    out.bool(
        S::DiscordCoinbaseVerify,
        0,
        boolean(f("coinbaseVerifyEnable")),
    );
    out.bool(S::ShowBlockFound, 0, boolean(f("showBlockFoundScreen")));
    out.0
}

/// `PATCH /api/v2/can/nodes/{id}`: frequency, voltage, a fan command per
/// entry of `fans` (at most two; values converted like `as<uint8_t>()`, so
/// missing ones are 0), then the display flags if either is a bool.
pub fn can_node(doc: &Json) -> Vec<Write> {
    let mut out = Out(Vec::new());
    let f = |k| field(doc, k);
    out.num(S::CanFrequency, 0, uint(f("frequency"), 0xffff));
    out.num(S::CanVoltage, 0, uint(f("coreVoltage"), 0xffff));
    if let Some(fans) = f("fans").as_array() {
        for (ch, fan) in fans.iter().take(2).enumerate() {
            let fan = if fan.is_object() { fan } else { &NULL };
            let b = |k| field(fan, k).to_u8() as u32;
            let num =
                b("mode") | b("manualSpeed") << 8 | b("targetTemp") << 16 | b("overheatTemp") << 24;
            out.num(S::CanFan, ch as i32, Some(num));
        }
    }
    let (flip, auto_off) = (f("flipScreen"), f("autoScreenOff"));
    if flip.as_bool().is_some() || auto_off.as_bool().is_some() {
        out.num(
            S::CanDisplay,
            0,
            Some(flip.truthy() as u32 | (auto_off.truthy() as u32) << 1),
        );
    }
    out.0
}

/// `PATCH /api/v2/otp`: the `enabled` flag.
pub enum OtpBody {
    Invalid,
    NoEnabled,
    Enabled(bool),
}

pub fn otp_body(body: &[u8]) -> OtpBody {
    match crate::ajson::deserialize(body) {
        Err(_) => OtpBody::Invalid,
        Ok(doc) => doc
            .get("enabled")
            .as_bool()
            .map_or(OtpBody::NoEnabled, OtpBody::Enabled),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pid_and_fee_saturate() {
        assert_eq!(hundredths(1.5), 150);
        assert_eq!(hundredths(-1.0), 0);
        assert_eq!(hundredths(1e9), 65535);
        assert_eq!(hundredths(f32::NAN), 0);
        assert_eq!(tenths(2.25), 23);
    }
}
