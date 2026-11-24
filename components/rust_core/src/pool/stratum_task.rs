//! The stratum task loop shared by v1 and v2 (port of
//! `StratumTaskBase::task()`): load the config, wait for a host, the network
//! and the stop flag, resolve, connect, run the protocol loop, tear down,
//! wait before the next attempt. The task, its transport and the protocol
//! loop are reached through [`Hooks`].
//!
//! The reconnect flag is cleared *before* the config is loaded, so a
//! reconnect requested in between (settings saved with a new pool) is kept.
//!
//! IPv6 is preferred for hostnames with an AAAA record when the device has a
//! routable IPv6 address; if that connect fails, IPv4 is tried and IPv6 is
//! skipped for this host and port for a while.

use crate::log::log;
use std::net::IpAddr;

/// Pause before the next attempt (no host, stopped, no network, DNS or
/// connect failure, connection lost).
pub const RETRY_DELAY_MS: u32 = 10_000;
/// How long IPv6 is skipped for a host and port after its connect failed.
pub const IPV6_SKIP_US: u64 = 5 * 60 * 1_000_000;

/// The address family of a DNS lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    /// IP literals (IPv4 or IPv6)
    Any,
    V4,
    V6,
}

/// What the task loop needs from the C++ task.
pub trait Hooks {
    /// Suspends the task while the device is shut down (returns after resume).
    fn suspend_if_shutdown(&mut self);
    fn clear_reconnect(&mut self);
    fn reconnect_requested(&mut self) -> bool;
    /// Loads the current config; the host (empty if none) and port.
    fn load_config(&mut self) -> (String, u16);
    /// The task is stopped (e.g. the secondary pool in failover mode).
    fn stopped(&mut self) -> bool;
    fn network_up(&mut self) -> bool;
    /// DNS lookup; the address as text.
    fn resolve(&mut self, host: &str, family: Family) -> Option<String>;
    /// The device has a preferred global or unique-local IPv6 address.
    fn has_routable_ipv6(&mut self) -> bool;
    fn now_us(&mut self) -> u64;
    /// Selects the transport (TCP, TLS, Noise) and connects.
    fn connect(&mut self, host: &str, ip: &str, port: u16) -> bool;
    fn protocol_loop(&mut self);
    fn count_pool_error(&mut self);
    /// Marks the task disconnected, closes the transport, tells the manager.
    fn disconnect(&mut self);
    fn delay_ms(&mut self, ms: u32);
}

/// What one round of the loop did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Round {
    NoHost,
    Stopped,
    NoNetwork,
    ResolveFailed,
    ConnectFailed,
    /// The protocol loop ended; `reconnect`: on request (no error, no delay).
    Disconnected {
        reconnect: bool,
    },
}

/// IPv6 skipped for the host and port whose IPv6 connect failed, so
/// switching pools tries IPv6 right away.
#[derive(Default)]
pub struct Ipv6Skip {
    host: String,
    port: u16,
    until_us: u64,
}

impl Ipv6Skip {
    fn active(&self, host: &str, port: u16, now_us: u64) -> bool {
        now_us < self.until_us && port == self.port && host == self.host
    }

    fn set(&mut self, host: &str, port: u16, now_us: u64) {
        self.host = host.into();
        self.port = port;
        self.until_us = now_us + IPV6_SKIP_US;
    }
}

fn retry_later(h: &mut impl Hooks, round: Round) -> Round {
    h.delay_ms(RETRY_DELAY_MS);
    round
}

/// One round of the task loop, including its delay; `skip` is kept across
/// rounds.
pub fn round(h: &mut impl Hooks, skip: &mut Ipv6Skip, tag: &core::ffi::CStr) -> Round {
    h.suspend_if_shutdown();
    // Clear first, so a reconnect requested while the config is
    // being loaded stays requested.
    h.clear_reconnect();
    let (host, port) = h.load_config();

    if host.is_empty() {
        return retry_later(h, Round::NoHost);
    }
    if h.stopped() {
        return retry_later(h, Round::Stopped);
    }
    if !h.network_up() {
        log!(Info, tag, "No network connection, waiting...");
        return retry_later(h, Round::NoNetwork);
    }
    // an IPv6 literal may be written in brackets, as in URLs
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .filter(|h| !h.is_empty())
        .unwrap_or(host.as_str());
    let literal = host.parse::<IpAddr>().is_ok();
    let mut connected = false;

    // Prefer IPv6 for hostnames with an AAAA record when the device has a
    // routable IPv6 address. If IPv6 can't connect, try IPv4 for the same
    // host and skip IPv6 for this host for a while. TLS still verifies the
    // certificate against the hostname.
    if !literal && !skip.active(host, port, h.now_us()) && h.has_routable_ipv6() {
        if let Some(ip) = h.resolve(host, Family::V6) {
            log!(
                Info,
                tag,
                "Connecting to: stratum+tcp://{host}:{port} ([{ip}])"
            );
            connected = h.connect(host, &ip, port);
            if !connected {
                log!(Warn, tag, "IPv6 connection to {host} failed, trying IPv4");
                let now = h.now_us();
                skip.set(host, port, now);
            }
        }
    }

    if !connected {
        // IP literals may be IPv4 or IPv6; hostnames resolve to IPv4 as
        // before. A hostname without an A record (IPv6-only pool) still uses
        // its AAAA.
        let ip = if literal {
            h.resolve(host, Family::Any)
        } else {
            h.resolve(host, Family::V4)
                .or_else(|| h.resolve(host, Family::V6))
        };
        let Some(ip) = ip else {
            log!(Error, tag, "{host} couldn't be resolved!");
            return retry_later(h, Round::ResolveFailed);
        };

        // IPv6 addresses are shown in brackets, as in URLs
        let v6 = ip.contains(':');
        let url_host = if literal && v6 {
            format!("[{host}]")
        } else {
            host.to_string()
        };
        let shown_ip = if v6 { format!("[{ip}]") } else { ip.clone() };
        log!(
            Info,
            tag,
            "Connecting to: stratum+tcp://{url_host}:{port} ({shown_ip})"
        );
        if !h.connect(host, &ip, port) {
            return retry_later(h, Round::ConnectFailed);
        }
    }

    h.protocol_loop();

    // a requested reconnect is not a pool error and skips the delay
    let reconnect = h.reconnect_requested();
    if reconnect {
        log!(Info, tag, "Shutdown socket ...");
    } else {
        h.count_pool_error();
        log!(Error, tag, "Shutdown socket ...");
    }
    h.disconnect();
    if !reconnect {
        h.delay_ms(RETRY_DELAY_MS);
    }
    Round::Disconnected { reconnect }
}

mod api {

    // SAFETY (all): the C++ side provides valid callbacks for its context.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        host: String,
        port: u16,
        stopped: bool,
        network: bool,
        /// the A and AAAA records
        a: Option<&'static str>,
        aaaa: Option<&'static str>,
        routable_ipv6: bool,
        connects: bool,
        ipv6_connects: bool,
        now_us: u64,
        lookups: Vec<Family>,
        reconnect: bool,
        /// a reconnect arrives while the config loads
        reconnect_during_load: bool,
        log: Vec<String>,
    }

    impl Hooks for Fake {
        fn suspend_if_shutdown(&mut self) {}
        fn clear_reconnect(&mut self) {
            self.reconnect = false;
            self.log.push("clear".into());
        }
        fn reconnect_requested(&mut self) -> bool {
            self.reconnect
        }
        fn load_config(&mut self) -> (String, u16) {
            if self.reconnect_during_load {
                self.reconnect = true;
            }
            (self.host.clone(), self.port)
        }
        fn stopped(&mut self) -> bool {
            self.stopped
        }
        fn network_up(&mut self) -> bool {
            self.network
        }
        fn resolve(&mut self, host: &str, family: Family) -> Option<String> {
            self.lookups.push(family);
            match family {
                Family::Any => host.parse::<IpAddr>().ok().map(|ip| ip.to_string()),
                Family::V4 => self.a.map(Into::into),
                Family::V6 => self.aaaa.map(Into::into),
            }
        }
        fn has_routable_ipv6(&mut self) -> bool {
            self.routable_ipv6
        }
        fn now_us(&mut self) -> u64 {
            self.now_us
        }
        fn connect(&mut self, host: &str, ip: &str, port: u16) -> bool {
            self.log.push(format!("connect {host} {ip} {port}"));
            self.connects && (self.ipv6_connects || !ip.contains(':'))
        }
        fn protocol_loop(&mut self) {
            self.log.push("protocol".into());
        }
        fn count_pool_error(&mut self) {
            self.log.push("error".into());
        }
        fn disconnect(&mut self) {
            self.log.push("disconnect".into());
        }
        fn delay_ms(&mut self, ms: u32) {
            self.log.push(format!("delay {ms}"));
        }
    }

    fn up() -> Fake {
        Fake {
            host: "pool".into(),
            port: 3333,
            network: true,
            a: Some("10.0.0.1"),
            connects: true,
            ipv6_connects: true,
            ..Default::default()
        }
    }

    /// up() with an AAAA record and a routable IPv6 address
    fn dual() -> Fake {
        Fake {
            aaaa: Some("2001:db8::1"),
            routable_ipv6: true,
            ..up()
        }
    }

    fn connects(f: &Fake) -> Vec<&str> {
        f.log
            .iter()
            .filter(|l| l.starts_with("connect"))
            .map(String::as_str)
            .collect()
    }

    #[test]
    fn waits_for_preconditions() {
        let tag = c"test";
        for (mut f, expected) in [
            (
                Fake {
                    host: "".into(),
                    ..up()
                },
                Round::NoHost,
            ),
            (
                Fake {
                    stopped: true,
                    ..up()
                },
                Round::Stopped,
            ),
            (
                Fake {
                    network: false,
                    ..up()
                },
                Round::NoNetwork,
            ),
            (Fake { a: None, ..up() }, Round::ResolveFailed),
            (
                Fake {
                    a: None,
                    routable_ipv6: true,
                    ..up()
                },
                Round::ResolveFailed,
            ),
            (
                Fake {
                    connects: false,
                    ..up()
                },
                Round::ConnectFailed,
            ),
        ] {
            assert_eq!(round(&mut f, &mut Ipv6Skip::default(), tag), expected);
            assert_eq!(f.log.last().unwrap(), "delay 10000");
            assert!(!f.log.iter().any(|l| l == "protocol"));
        }
    }

    #[test]
    fn connection_lost_counts_an_error_and_waits() {
        let mut f = up();
        assert_eq!(
            round(&mut f, &mut Ipv6Skip::default(), c"t"),
            Round::Disconnected { reconnect: false }
        );
        assert_eq!(
            f.log,
            [
                "clear",
                "connect pool 10.0.0.1 3333",
                "protocol",
                "error",
                "disconnect",
                "delay 10000"
            ]
        );
    }

    #[test]
    fn requested_reconnect_skips_error_and_delay() {
        let mut f = Fake {
            reconnect_during_load: true,
            ..up()
        };
        // the reconnect requested during the config load survives
        assert_eq!(
            round(&mut f, &mut Ipv6Skip::default(), c"t"),
            Round::Disconnected { reconnect: true }
        );
        assert_eq!(
            f.log,
            [
                "clear",
                "connect pool 10.0.0.1 3333",
                "protocol",
                "disconnect"
            ]
        );
    }

    #[test]
    fn prefers_ipv6_when_routable() {
        let mut f = dual();
        let mut skip = Ipv6Skip::default();
        assert_eq!(
            round(&mut f, &mut skip, c"t"),
            Round::Disconnected { reconnect: false }
        );
        assert_eq!(connects(&f), ["connect pool 2001:db8::1 3333"]);
        assert_eq!(f.lookups, [Family::V6]);
        // a connection lost later sets no skip: IPv6 again next round
        f.lookups.clear();
        round(&mut f, &mut skip, c"t");
        assert_eq!(f.lookups, [Family::V6]);
    }

    #[test]
    fn without_routable_ipv6_straight_to_ipv4() {
        let mut f = Fake {
            routable_ipv6: false,
            ..dual()
        };
        round(&mut f, &mut Ipv6Skip::default(), c"t");
        assert_eq!(connects(&f), ["connect pool 10.0.0.1 3333"]);
        assert_eq!(f.lookups, [Family::V4]);
    }

    #[test]
    fn ipv6_failure_falls_back_to_ipv4_and_skips_ipv6() {
        let mut f = Fake {
            ipv6_connects: false,
            ..dual()
        };
        let mut skip = Ipv6Skip::default();
        assert_eq!(
            round(&mut f, &mut skip, c"t"),
            Round::Disconnected { reconnect: false }
        );
        assert_eq!(
            connects(&f),
            [
                "connect pool 2001:db8::1 3333",
                "connect pool 10.0.0.1 3333"
            ]
        );
        // one pool error: the lost connection, not the IPv6 attempt
        assert_eq!(f.log.iter().filter(|l| *l == "error").count(), 1);

        // within the skip window: IPv4 only
        f.log.clear();
        f.lookups.clear();
        f.now_us += IPV6_SKIP_US - 1;
        round(&mut f, &mut skip, c"t");
        assert_eq!(connects(&f), ["connect pool 10.0.0.1 3333"]);
        assert_eq!(f.lookups, [Family::V4]);

        // the window expired: IPv6 again
        f.log.clear();
        f.now_us += 1;
        f.ipv6_connects = true;
        round(&mut f, &mut skip, c"t");
        assert_eq!(connects(&f), ["connect pool 2001:db8::1 3333"]);
    }

    #[test]
    fn ipv6_skip_only_for_the_same_host_and_port() {
        let mut f = Fake {
            ipv6_connects: false,
            ..dual()
        };
        let mut skip = Ipv6Skip::default();
        round(&mut f, &mut skip, c"t");
        for (host, port) in [("other", 3333), ("pool", 4444)] {
            f.host = host.into();
            f.port = port;
            f.log.clear();
            f.lookups.clear();
            f.ipv6_connects = true;
            round(&mut f, &mut skip, c"t");
            assert_eq!(f.lookups, [Family::V6]);
            assert_eq!(connects(&f), [format!("connect {host} 2001:db8::1 {port}")]);
            // the skip stays with pool:3333
            f.ipv6_connects = false;
        }
        f.host = "pool".into();
        f.port = 3333;
        f.lookups.clear();
        round(&mut f, &mut skip, c"t");
        assert_eq!(f.lookups, [Family::V4]);
    }

    #[test]
    fn ipv4_failure_after_ipv6_failure_retries_later() {
        let mut f = Fake {
            connects: false,
            ..dual()
        };
        assert_eq!(
            round(&mut f, &mut Ipv6Skip::default(), c"t"),
            Round::ConnectFailed
        );
        assert_eq!(
            connects(&f),
            [
                "connect pool 2001:db8::1 3333",
                "connect pool 10.0.0.1 3333"
            ]
        );
        assert_eq!(f.log.last().unwrap(), "delay 10000");
        assert!(!f.log.iter().any(|l| l == "error"));
    }

    #[test]
    fn ipv6_only_host_uses_its_aaaa() {
        for routable_ipv6 in [false, true] {
            let mut f = Fake {
                a: None,
                routable_ipv6,
                ..dual()
            };
            assert_eq!(
                round(&mut f, &mut Ipv6Skip::default(), c"t"),
                Round::Disconnected { reconnect: false }
            );
            assert_eq!(connects(&f), ["connect pool 2001:db8::1 3333"]);
        }
    }

    #[test]
    fn ip_literals_are_used_as_they_are() {
        for (host, ip) in [
            ("10.1.2.3", "10.1.2.3"),
            ("2001:db8::2", "2001:db8::2"),
            ("[2001:db8::2]", "2001:db8::2"),
        ] {
            let mut f = Fake {
                host: host.into(),
                ..dual()
            };
            assert_eq!(
                round(&mut f, &mut Ipv6Skip::default(), c"t"),
                Round::Disconnected { reconnect: false }
            );
            // brackets stripped, no DNS lookup of a record
            assert_eq!(connects(&f), [format!("connect {ip} {ip} 3333")]);
            assert_eq!(f.lookups, [Family::Any]);
        }
    }
}
