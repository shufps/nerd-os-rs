//! Settings request bodies of the HTTP API: the typed writes
//! (`settings_apply.rs` applies them to the config) and the field readers
//! shared by the parsers next to the handlers in `v1/` and `v2/`.
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
    /// seconds without a key press before auto screen off
    AutoScreenOffTimeout = 64,
    /// CAN node commands (`PATCH /api/v2/can/nodes/{id}`), not config:
    CanFrequency = 60,
    CanVoltage = 61,
    /// index: fan channel; num: mode | speed << 8 | target temp << 16 | overheat << 24
    CanFan = 62,
    /// num: flip screen | auto screen off << 1
    CanDisplay = 63,
    HeartbeatHours = 65,
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

pub(crate) const NULL: Json = Json::Null;

pub(crate) fn field<'a>(v: &'a Json, key: &str) -> &'a Json {
    v.get(key)
}

/// `is<uintN_t>()` with the given maximum.
pub(crate) fn uint(v: &Json, max: u64) -> Option<u32> {
    v.as_uint(max).map(|n| n as u32)
}

/// The auto screen off time: 5 s to 1 h, else ignored.
pub(crate) fn screen_off_secs(v: &Json) -> Option<u32> {
    uint(v, 0xffff)
        .filter(|s| (crate::cfg::SCREEN_OFF_MIN_S..=crate::cfg::SCREEN_OFF_MAX_S).contains(s))
}

pub(crate) fn boolean(v: &Json) -> Option<bool> {
    v.as_bool()
}

/// `is<const char *>()`, as C sees it.
pub(crate) fn string(v: &Json) -> Option<Vec<u8>> {
    v.as_cstr().map(<[u8]>::to_vec)
}

/// `is<float>()` (any number), `as<float>()`.
pub(crate) fn num_f32(v: &Json) -> Option<f32> {
    v.as_f32()
}

/// `is<bool>() || is<int>()`, then `as<int>() != 0`.
pub(crate) fn flag(v: &Json) -> Option<bool> {
    v.as_bool().or_else(|| v.as_int().map(|i| i != 0))
}

/// A PID factor as stored: hundredths, rounded, saturated.
pub(crate) fn hundredths(f: f32) -> u32 {
    (f * 100.0).round() as u16 as u32
}

/// The coinbase fee as stored: tenths of a percent, rounded, saturated.
pub(crate) fn tenths(f: f32) -> u32 {
    (f * 10.0).round() as u16 as u32
}

pub(crate) struct Out(pub(crate) Vec<Write>);

impl Out {
    pub(crate) fn num(&mut self, setting: Setting, index: i32, num: Option<u32>) {
        if let Some(num) = num {
            self.0.push(Write {
                setting,
                index,
                num,
                text: None,
            });
        }
    }
    pub(crate) fn bool(&mut self, setting: Setting, index: i32, b: Option<bool>) {
        self.num(setting, index, b.map(u32::from));
    }
    pub(crate) fn text(&mut self, setting: Setting, index: i32, text: Option<Vec<u8>>) {
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

pub(crate) use Setting as S;

/// The `fans` array: at most two entries (channels 0 and 1); entries that
/// are not objects still take a channel.
pub(crate) fn fans(doc: &Json, out: &mut Out) {
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
            out.num(S::FanPidP, ch, num_f32(field(pid, "p")).map(hundredths));
            out.num(S::FanPidI, ch, num_f32(field(pid, "i")).map(hundredths));
            out.num(S::FanPidD, ch, num_f32(field(pid, "d")).map(hundredths));
        }
    }
}

/// ASIC values the handlers only take when not 0.
pub(crate) fn nonzero(v: &Json) -> Option<u32> {
    uint(v, 0xffff).filter(|&n| n > 0)
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

    #[test]
    fn pid_factors_keep_their_hundredths() {
        // 0.53 * 100.0 is 52.999996 in f32
        for h in 0..1000u32 {
            assert_eq!(hundredths(h as f32 / 100.0), h);
        }
    }
}
