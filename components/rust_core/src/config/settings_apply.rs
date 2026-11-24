//! Applies the writes of a settings body (`http/settings.rs`) to the
//! config (port of `main/http_server/settings_apply.cpp`): each setting
//! goes to its NVS key, per pool or fan channel where it has an index. The
//! config is reached through [`Store`] (a recording one in the tests).

use crate::ajson::{Json, Num};
use crate::config;
use crate::http::settings::{Setting, Write};

/// What applying a body changed beyond the config.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// the coinbase verification settings of a pool (re-run it)
    pub verify_changed: [bool; 2],
}

/// The config as `nvs_config.h`'s `cfgSet*()`/`cfgGetU16()`.
pub trait Store {
    fn set_u16(&mut self, key: &[u8], v: u16);
    fn set_u64(&mut self, key: &[u8], v: u64);
    /// None stores null (a NULL string)
    fn set_str(&mut self, key: &[u8], v: Option<&[u8]>);
    fn get_u16(&mut self, key: &[u8], def: u16) -> u16;
}

/// The firmware's config.
pub struct Config;

impl Store for Config {
    fn set_u16(&mut self, key: &[u8], v: u16) {
        config::set(key, Json::Num(Num::U(v as u64)));
    }
    fn set_u64(&mut self, key: &[u8], v: u64) {
        config::set(key, Json::Num(Num::U(v)));
    }
    fn set_str(&mut self, key: &[u8], v: Option<&[u8]>) {
        config::set(key, v.map_or(Json::Null, |v| Json::Str(v.to_vec())));
    }
    fn get_u16(&mut self, key: &[u8], def: u16) -> u16 {
        config::get_u16(key, def)
    }
}

/// Whether the board takes `fanPidUseMax` (the NerdAxe and NerdAxeGamma
/// builds).
pub fn pid_use_max_board() -> bool {
    matches!(crate::config::sdkconfig::BOARD, "NERDAXE" | "NERDAXEGAMMA")
}

/// Applies one write; `pid_use_max`: the board takes `fanPidUseMax`.
pub fn apply(w: &Write, applied: &mut Applied, pid_use_max: bool, s: &mut impl Store) {
    use Setting::*;
    let u16v = w.num as u16;
    let b = (w.num != 0) as u16;
    let i = w.index;
    let primary = i == 0;
    // the C setters got a C string: one with a NUL inside came out empty
    let text: Option<&[u8]> = w
        .text
        .as_deref()
        .map(|t| if t.contains(&0) { &b""[..] } else { t });
    let pool = |p: &'static [u8], f: &'static [u8]| if primary { p } else { f };
    let fan = |c0: &'static [u8], c1: &'static [u8]| if i == 0 { c0 } else { c1 };

    match w.setting {
        WifiSsid => s.set_str(b"wifissid", text),
        WifiPass => s.set_str(b"wifipass", text),
        Hostname => s.set_str(b"hostname", text),
        AsicVoltage => s.set_u16(b"asicvoltage", u16v),
        AsicFrequency => s.set_u16(b"asicfrequency", u16v),
        AsicJobInterval => s.set_u16(b"asicjobinterval", u16v),
        StratumDifficulty => s.set_u64(b"stratumdiff", w.num as u64),
        FlipScreen => s.set_u16(b"flipscreen", b),
        OverheatTemp => s.set_u16(b"overheat_temp", u16v),
        InvertScreen => s.set_u16(b"invertscreen", b),
        FanPolarity => s.set_u16(b"pwmfanpol", b),
        TempControlMode => s.set_u16(b"autofanspeed", u16v),
        FanSpeed => s.set_u16(b"fanspeed", u16v),
        AutoScreenOff => s.set_u16(b"autoscreenoff", b),
        AutoScreenOffTimeout => s.set_u16(b"scroff_secs", u16v),
        StratumKeepalive => s.set_u16(b"stratum_keep", b),
        PidTargetTemp => s.set_u16(b"pid_temp", u16v),
        PidP => s.set_u16(b"pid_p", u16v),
        PidI => s.set_u16(b"pid_i", u16v),
        PidD => s.set_u16(b"pid_d", u16v),
        VrFrequency => s.set_u64(b"vr_frequency", w.num as u64),
        FanMode => s.set_u16(fan(b"autofanspeed", b"fan1mode"), u16v),
        FanManualSpeed => s.set_u16(fan(b"fanspeed", b"fan1speed"), u16v),
        FanOverheatTemp => s.set_u16(fan(b"overheat_temp", b"fan1_overheat"), u16v),
        FanPidTargetTemp => s.set_u16(fan(b"pid_temp", b"fan1_pid_temp"), u16v),
        FanPidP => s.set_u16(fan(b"pid_p", b"fan1_pid_p"), u16v),
        FanPidI => s.set_u16(fan(b"pid_i", b"fan1_pid_i"), u16v),
        FanPidD => s.set_u16(fan(b"pid_d", b"fan1_pid_d"), u16v),
        PoolMode => s.set_u16(b"pool_mode", u16v),
        PoolBalance => s.set_u16(b"pool_balance", u16v),
        StratumUrl => s.set_str(pool(b"stratumurl", b"fbstratumurl"), text),
        StratumUser => s.set_str(pool(b"stratumuser", b"fbstratumuser"), text),
        StratumPass => s.set_str(pool(b"stratumpass", b"fbstratumpass"), text),
        StratumPort => s.set_u16(pool(b"stratumport", b"fbstratumport"), u16v),
        StratumEnonceSubscribe => s.set_u16(pool(b"stratumesub", b"fbstratumesub"), b),
        StratumTls => s.set_u16(pool(b"stratumtls", b"fbstratumtls"), b),
        StratumProtocol => s.set_u16(pool(b"sv2_proto", b"fbsv2_proto"), u16v),
        Sv2AuthorityPubkey => s.set_str(pool(b"sv2_auth_pk", b"fbsv2_authpk"), text),
        Sv2ChannelType => s.set_u16(pool(b"sv2_chan_type", b"fbsv2_chtype"), u16v),
        Sv2RequireAuth => s.set_u16(pool(b"sv2_reqauth", b"fbsv2_reqauth"), b),
        MempoolCustom => s.set_u16(b"mempool_cust", b),
        MempoolUrl => s.set_str(b"mempool_url", text),
        FanPidUseMax => {
            if pid_use_max {
                s.set_u16(b"fan_pid_max", b);
            }
        }
        CanEnabled => s.set_u16(b"can_enabled", b),
        InfluxEnabled => s.set_u16(b"influx_enable", b),
        InfluxUrl => s.set_str(b"influx_url", text),
        InfluxPort => s.set_u16(b"influx_port", u16v),
        InfluxToken => s.set_str(b"influx_token", text),
        InfluxBucket => s.set_str(b"influx_bucket", text),
        InfluxOrg => s.set_str(b"influx_org", text),
        InfluxPrefix => s.set_str(b"influx_prefix", text),
        DiscordWebhook => s.set_str(b"alrt_disc_url", text),
        DiscordWatchdog => s.set_u16(b"alrt_disc_en", b),
        DiscordBlockFound => s.set_u16(b"alrt_disc_bf_en", b),
        DiscordBestDiff => s.set_u16(b"alrt_disc_bd_en", b),
        DiscordCoinbaseVerify => s.set_u16(b"alrt_disc_cb_en", b),
        ShowBlockFound => s.set_u16(b"block_found_en", b),
        HeartbeatHours => s.set_u16(b"alrt_hb_intv", u16v),
        // CAN node commands go to the node, not the config
        CanFrequency | CanVoltage | CanFan | CanDisplay => {}
        // coinbase verification: remember whether it changed, to re-run it
        CoinbaseVerifyMode => {
            let key = pool(b"cb_verify_mode", b"fb_cb_vfy_mode");
            applied.verify_changed[(i & 1) as usize] |= s.get_u16(key, 0) != u16v;
            s.set_u16(key, u16v);
        }
        CoinbaseMaxFee => {
            let key = pool(b"cb_max_fee", b"fb_cb_max_fee");
            applied.verify_changed[(i & 1) as usize] |= s.get_u16(key, 30) != u16v;
            s.set_u16(key, u16v);
        }
        CoinbaseVerifyForce => {
            let key = pool(b"cb_vfy_force", b"fb_cb_vfy_frc");
            applied.verify_changed[(i & 1) as usize] |= (s.get_u16(key, 0) != 0) != (b != 0);
            s.set_u16(key, b);
        }
    }
}

/// Parses `body` with `parse` and applies its writes; false for invalid
/// JSON (nothing applied).
pub fn apply_body(
    body: &[u8],
    parse: impl Fn(&Json) -> Vec<Write>,
    applied: &mut Applied,
    s: &mut impl Store,
) -> bool {
    let Ok(doc) = crate::ajson::deserialize(body) else {
        return false;
    };
    let pid_use_max = pid_use_max_board();
    for w in parse(&doc) {
        apply(&w, applied, pid_use_max, s);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Rec(Vec<String>);

    impl Store for Rec {
        fn set_u16(&mut self, key: &[u8], v: u16) {
            self.0
                .push(format!("{}=u16:{v}", String::from_utf8_lossy(key)));
        }
        fn set_u64(&mut self, key: &[u8], v: u64) {
            self.0
                .push(format!("{}=u64:{v}", String::from_utf8_lossy(key)));
        }
        fn set_str(&mut self, key: &[u8], v: Option<&[u8]>) {
            let v = v.map_or("NULL".into(), String::from_utf8_lossy);
            self.0
                .push(format!("{}=s:{v}", String::from_utf8_lossy(key)));
        }
        fn get_u16(&mut self, _: &[u8], def: u16) -> u16 {
            def
        }
    }

    fn w(setting: Setting, index: i32, num: u32, text: Option<&[u8]>) -> Write {
        Write {
            setting,
            index,
            num,
            text: text.map(<[u8]>::to_vec),
        }
    }

    #[test]
    fn pools_fans_and_verification() {
        let mut r = Rec::default();
        let mut a = Applied::default();
        apply(
            &w(Setting::StratumUrl, 1, 0, Some(b"pool")),
            &mut a,
            false,
            &mut r,
        );
        apply(&w(Setting::FanPidP, 1, 7, None), &mut a, false, &mut r);
        apply(
            &w(Setting::CoinbaseMaxFee, 0, 30, None),
            &mut a,
            false,
            &mut r,
        );
        assert_eq!(a.verify_changed, [false, false]);
        apply(
            &w(Setting::CoinbaseVerifyForce, 1, 1, None),
            &mut a,
            false,
            &mut r,
        );
        assert_eq!(a.verify_changed, [false, true]);
        apply(&w(Setting::FanPidUseMax, 0, 1, None), &mut a, false, &mut r);
        apply(
            &w(Setting::Hostname, 0, 0, Some(b"a\0b")),
            &mut a,
            false,
            &mut r,
        );
        assert_eq!(
            r.0,
            [
                "fbstratumurl=s:pool",
                "fan1_pid_p=u16:7",
                "cb_max_fee=u16:30",
                "fb_cb_vfy_frc=u16:1",
                "hostname=s:"
            ]
        );
    }
}
