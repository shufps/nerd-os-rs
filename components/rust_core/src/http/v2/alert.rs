//! `GET`/`PATCH /api/v2/alert` and `POST /api/v2/alert/test` (port of
//! `handler_alert.cpp`).

use crate::ajson::Json;
use crate::config;
use crate::config::sdkconfig as k;
use crate::config::settings_apply::Applied;
use crate::http::json::Obj;
use crate::http::reply::{
    apply_settings, cfg_flag, cfg_str, end_chunks, send_doc, send_err, send_str, set_json,
};
use crate::http::settings::{boolean, field, string, uint, Out, Setting as S, Write};
use crate::idf::httpd::{err, Request};
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};

/// `alert_info_t`: `GET /api/alert/info`.
#[derive(Clone, Debug, Default)]
pub struct AlertInfo {
    pub has_webhook: bool,
    pub watchdog: bool,
    pub block_found: bool,
    pub best_diff: bool,
    pub coinbase_verify: bool,
    pub show_block_found_screen: bool,
    pub heartbeat_hours: u16,
}

pub fn alert_info(a: &AlertInfo) -> Json {
    let mut d = Obj::new();
    d.bool("hasWebhook", a.has_webhook)
        .int("watchdogEnable", a.watchdog as i64)
        .int("blockFoundEnable", a.block_found as i64)
        .int("bestDiffEnable", a.best_diff as i64)
        .int("coinbaseVerifyEnable", a.coinbase_verify as i64)
        .int("showBlockFoundScreen", a.show_block_found_screen as i64)
        .uint("heartbeatHours", a.heartbeat_hours as u64);
    d.into()
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
    out.num(
        S::HeartbeatHours,
        0,
        uint(f("heartbeatHours"), crate::heartbeat::MAX_HOURS as u64),
    );
    out.0
}

pub(crate) fn get(req: &mut Request) -> EspErr {
    set_json(req);
    let webhook = cfg_str(b"alrt_disc_url", k::ALERT_DISCORD_URL);
    let a = AlertInfo {
        has_webhook: !webhook.is_empty(),
        watchdog: cfg_flag(b"alrt_disc_en", k::ALERT_DISCORD_WATCHDOG_ENABLE_VALUE),
        block_found: cfg_flag(
            b"alrt_disc_bf_en",
            k::ALERT_DISCORD_BLOCK_FOUND_ENABLE_VALUE,
        ),
        best_diff: cfg_flag(b"alrt_disc_bd_en", k::ALERT_DISCORD_BEST_DIFF_ENABLE_VALUE),
        coinbase_verify: cfg_flag(b"alrt_disc_cb_en", 0),
        show_block_found_screen: cfg_flag(b"block_found_en", k::SHOW_BLOCK_FOUND_ENABLE_VALUE),
        heartbeat_hours: config::get_u16(b"alrt_hb_intv", 0).min(crate::heartbeat::MAX_HOURS),
    };
    send_doc(req, alert_info(&a))
}

pub(crate) fn patch(req: &mut Request) -> EspErr {
    let mut a = Applied::default();
    if apply_settings(req, c"http_utils", settings_alert, &mut a) != ESP_OK {
        return ESP_FAIL;
    }
    crate::config::nvs::request_flush();
    end_chunks(req);
    // the alerter's and the display's settings
    crate::discord::load_config();
    crate::display::load_settings();
    ESP_OK
}

pub(crate) fn test(req: &mut Request) -> EspErr {
    if crate::discord::api::discord_send_test() {
        send_str(req, c"ok");
    } else {
        send_err(req, err::E500_INTERNAL_SERVER_ERROR, c"Send failed");
    }
    ESP_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_hours() {
        let parse = |b: &str| settings_alert(&crate::ajson::deserialize(b.as_bytes()).unwrap());
        let w = parse(r#"{"heartbeatHours":6}"#);
        assert_eq!((w.len(), w[0].setting, w[0].num), (1, S::HeartbeatHours, 6));
        assert_eq!(parse(r#"{"heartbeatHours":0}"#)[0].num, 0);
        // out of range or not a number: ignored
        assert!(parse(r#"{"heartbeatHours":25}"#).is_empty());
        assert!(parse(r#"{"heartbeatHours":-1}"#).is_empty());
        assert!(parse(r#"{"heartbeatHours":"6"}"#).is_empty());
    }
}
