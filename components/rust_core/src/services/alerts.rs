//! Discord alerts (port of `DiscordAlerter` in `main/discord.cpp`): the
//! alert texts, a queue of 4 messages (a full queue drops the alert, as
//! before) and the sender loop that posts them to the webhook. The HTTP
//! request (`esp_http_client`) is a hook.
//!
//! Behaviour notes:
//! - the payload is built as JSON: the message (with the user-set hostname)
//!   is escaped, so any hostname gives valid JSON;
//! - the configuration is a snapshot under a lock, so the settings handler
//!   can replace it while the sender or an alerting task uses the old one.

use core::ffi::{c_int, CStr};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Mutex, OnceLock};

use crate::log::log;

const TAG: &CStr = c"discord";
/// `ALERTER_MESSAGE_LEN` - 1: a queued message is cut to this many bytes.
const MESSAGE_MAX: usize = 255;
/// A message without the device block (the metrics) is cut to this many
/// bytes (Discord takes up to 2000 characters).
const PLAIN_MESSAGE_MAX: usize = 1000;
const QUEUE_LEN: usize = 4;

#[derive(Clone, Default, Debug)]
pub struct Config {
    pub webhook: String,
    pub host: String,
    pub watchdog: bool,
    pub block_found: bool,
    pub best_diff: bool,
    pub coinbase_verify: bool,
    /// the metrics message interval in hours, 0: off
    pub heartbeat_hours: u16,
}

static CONFIG: Mutex<Option<Config>> = Mutex::new(None);

pub(crate) fn config() -> Config {
    CONFIG
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
        .unwrap_or_default()
}

pub fn set_config(c: Config) {
    *CONFIG.lock().unwrap_or_else(|e| e.into_inner()) = Some(c);
}

/// The texts of the alerts. Line breaks are real newlines; `payload()`
/// escapes them to `\n`, the same JSON the C++ code sent with its literal
/// `\n` sequences.
pub mod text {
    pub fn watchdog() -> String {
        "Device rebooted because there was no share for more than 1h!".into()
    }

    pub fn test() -> String {
        "This is a test message!".into()
    }

    pub fn block_found(diff: &str, network: &str) -> String {
        format!(":tada: Block found!\nDiff: {diff} (network: {network})")
    }

    pub fn best_diff(diff: &str, network: &str) -> String {
        format!(":chart_with_upwards_trend: New *best difficulty* found!\nDiff: {diff} (network: {network})")
    }

    /// `mode` 2 checks the fee as well.
    pub fn coinbase_verify_failed(pool: i32, user_sats: u64, total_sats: u64, mode: i32) -> String {
        let head = format!(
            ":warning: Coinbase verification failed for pool {}!",
            pool + 1
        );
        if user_sats == 0 {
            format!("{head}\nYour address was **not found** in the coinbase transaction.")
        } else if mode >= 2 && total_sats > 0 {
            let fee_x100 =
                (total_sats.saturating_sub(user_sats) as u128 * 10_000 / total_sats as u128) as u64;
            format!(
                "{head}\nPool fee: **{}.{:02}%** exceeds configured limit.",
                fee_x100 / 100,
                fee_x100 % 100
            )
        } else {
            head
        }
    }

    /// The message with the device block appended.
    pub fn with_device(message: &str, host: &str, ip: &str, mac: &str) -> String {
        format!("{message}\n```\nHostname: {host}\nIP:       {ip}\nMAC:      {mac}\n```")
    }
}

/// Cuts `s` to `max` bytes at a character boundary.
fn cut(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

/// `{"content": "<message>"}` with the message escaped as a JSON string.
pub fn payload(message: &str) -> String {
    format!(
        "{{\"content\": \"{}\"}}",
        crate::util::cutil::json_escape(message)
    )
}

struct Queue {
    tx: SyncSender<String>,
    rx: Mutex<Option<Receiver<String>>>,
}

fn queue() -> &'static Queue {
    static Q: OnceLock<Queue> = OnceLock::new();
    Q.get_or_init(|| {
        let (tx, rx) = sync_channel(QUEUE_LEN);
        Queue {
            tx,
            rx: Mutex::new(Some(rx)),
        }
    })
}

/// Queues `message` with the device block; false if the queue is full.
pub fn enqueue(message: &str, ip: &str, mac: &str) -> bool {
    let host = config().host;
    let host = if host.is_empty() { "unknown" } else { &host };
    let msg = cut(text::with_device(message, host, ip, mac), MESSAGE_MAX);
    log!(Warn, TAG, "enqueued: {msg}");
    queue().tx.try_send(msg).is_ok()
}

/// Queues `message` as it is (no device block); false if the queue is full.
pub fn enqueue_plain(message: &str) -> bool {
    let msg = cut(message.to_string(), PLAIN_MESSAGE_MAX);
    log!(Info, TAG, "enqueued: {msg}");
    queue().tx.try_send(msg).is_ok()
}

/// Which alert, for the enable flags.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Test,
    Watchdog,
    BlockFound,
    BestDiff,
    CoinbaseVerify,
}

pub fn enabled(kind: Kind) -> bool {
    let c = config();
    match kind {
        Kind::Test => true,
        Kind::Watchdog => c.watchdog,
        Kind::BlockFound => c.block_found,
        Kind::BestDiff => c.best_diff,
        Kind::CoinbaseVerify => c.coinbase_verify,
    }
}

/// The `ALERT_*` codes.
pub mod kind {
    use core::ffi::c_int;
    pub const TEST: c_int = 0;
    pub const WATCHDOG: c_int = 1;
    pub const BLOCK_FOUND: c_int = 2;
    pub const BEST_DIFF: c_int = 3;
    pub const COINBASE_VERIFY: c_int = 4;
}

fn kind_of(k: c_int) -> Option<Kind> {
    Some(match k {
        0 => Kind::Test,
        1 => Kind::Watchdog,
        2 => Kind::BlockFound,
        3 => Kind::BestDiff,
        4 => Kind::CoinbaseVerify,
        _ => return None,
    })
}

/// Whether alerts of code `k` are enabled.
pub fn kind_enabled(k: c_int) -> bool {
    kind_of(k).is_some_and(enabled)
}

/// Queues an alert of code `k` (`kind::*`): the difficulty texts for block
/// found / best difficulty, the coinbase values for the verification
/// alert. False if disabled or the queue is full.
#[allow(clippy::too_many_arguments)]
pub fn send(
    k: c_int,
    diff: &str,
    network_diff: &str,
    pool: c_int,
    user_sats: u64,
    total_sats: u64,
    mode: c_int,
    ip: &str,
    mac: &str,
) -> bool {
    let Some(kind) = kind_of(k) else {
        return false;
    };
    if !enabled(kind) {
        if matches!(kind, Kind::Watchdog | Kind::BlockFound) {
            let what = if kind == Kind::Watchdog {
                "watchdog"
            } else {
                "block found"
            };
            log!(Info, TAG, "discord {what} alert not enabled");
        }
        return false;
    }
    let message = match kind {
        Kind::Test => text::test(),
        Kind::Watchdog => text::watchdog(),
        Kind::BlockFound => text::block_found(diff, network_diff),
        Kind::BestDiff => text::best_diff(diff, network_diff),
        Kind::CoinbaseVerify => text::coinbase_verify_failed(pool, user_sats, total_sats, mode),
    };
    enqueue(&message, ip, if mac.is_empty() { "unknown" } else { mac })
}

/// The sender: posts the queued messages; never returns.
pub fn run(post: &mut impl FnMut(&str, &str) -> c_int) -> ! {
    log!(Info, TAG, "Discord alerter started");
    let Some(rx) = queue().rx.lock().unwrap_or_else(|e| e.into_inner()).take() else {
        // a second sender: nothing to do
        loop {
            std::thread::park();
        }
    };
    loop {
        let Ok(msg) = rx.recv() else {
            loop {
                std::thread::park();
            }
        };
        post_message(&msg, post);
    }
}

/// Posts one message; the HTTP status (or -1).
fn post_message(message: &str, post: &mut impl FnMut(&str, &str) -> c_int) -> bool {
    let url = config().webhook;
    if url.is_empty() {
        log!(Error, TAG, "Webhook URL or message is null");
        return false;
    }
    log!(Info, TAG, "discord message: {message}");
    let status = post(&url, &payload(message));
    if (200..300).contains(&status) {
        log!(
            Info,
            TAG,
            "Discord message sent successfully (HTTP {status})"
        );
        true
    } else {
        log!(Error, TAG, "Discord responded with HTTP {status}");
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_as_before() {
        assert_eq!(
            text::block_found("1.23T", "135T"),
            ":tada: Block found!\nDiff: 1.23T (network: 135T)"
        );
        assert_eq!(
            text::coinbase_verify_failed(0, 0, 100, 2),
            ":warning: Coinbase verification failed for pool 1!\nYour address was **not found** in the coinbase transaction."
        );
        // 2 % fee
        assert_eq!(
            text::coinbase_verify_failed(1, 98, 100, 2),
            ":warning: Coinbase verification failed for pool 2!\nPool fee: **2.00%** exceeds configured limit."
        );
        assert_eq!(
            text::coinbase_verify_failed(1, 98, 100, 1),
            ":warning: Coinbase verification failed for pool 2!"
        );
        // large values are handled
        assert!(text::coinbase_verify_failed(0, 1, u64::MAX, 2).contains("99.99%"));
    }

    #[test]
    fn payload_is_escaped_json() {
        let msg = text::with_device("hi", "rig\"1\\x", "10.0.0.2", "AA:BB");
        let p = payload(&msg);
        assert_eq!(
            p,
            "{\"content\": \"hi\\n```\\nHostname: rig\\\"1\\\\x\\nIP:       10.0.0.2\\nMAC:      AA:BB\\n```\"}"
        );
        // it parses, and the content is the message
        let v = crate::util::ajson::deserialize(p.as_bytes()).unwrap();
        assert_eq!(v.get("content").as_cstr().unwrap(), msg.as_bytes());
    }

    #[test]
    fn messages_are_cut_at_255_bytes() {
        let m = cut("ä".repeat(200), MESSAGE_MAX);
        assert!(m.len() <= 255 && m.len() >= 254);
    }

    #[test]
    fn flags_and_queue() {
        set_config(Config {
            webhook: "https://example".into(),
            host: "rig".into(),
            watchdog: false,
            block_found: true,
            ..Default::default()
        });
        assert!(!enabled(Kind::Watchdog));
        assert!(enabled(Kind::BlockFound));
        assert!(enabled(Kind::Test));
        // 4 fit, the 5th is dropped
        let mut sent = 0;
        while enqueue("x", "1.2.3.4", "m") {
            sent += 1;
            assert!(sent <= QUEUE_LEN);
        }
        assert_eq!(sent, QUEUE_LEN);
        let rx = queue().rx.lock().unwrap();
        let first = rx.as_ref().unwrap().try_recv().unwrap();
        assert!(first.starts_with("x\n```\nHostname: rig\n"));
        drop(rx);
        let mut posted = Vec::new();
        assert!(post_message(&first, &mut |url: &str, body: &str| {
            posted.push((url.to_string(), body.to_string()));
            204
        }));
        assert_eq!(posted[0].0, "https://example");
        assert!(!post_message(&first, &mut |_: &str, _: &str| 500));
    }
}
