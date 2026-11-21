//! The stratum task loop shared by v1 and v2 (port of
//! `StratumTaskBase::task()`): load the config, wait for a host, the network
//! and the stop flag, resolve, connect, run the protocol loop, tear down,
//! wait before the next attempt. The task, its transport and the protocol
//! loop are reached through [`Hooks`].
//!
//! The reconnect flag is cleared *before* the config is loaded, so a
//! reconnect requested in between (settings saved with a new pool) is kept.

use crate::log::log;

/// Pause before the next attempt (no host, stopped, no network, DNS or
/// connect failure, connection lost).
pub const RETRY_DELAY_MS: u32 = 10_000;

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
    /// DNS lookup (IPv4); the address as text.
    fn resolve(&mut self, host: &str) -> Option<String>;
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

fn retry_later(h: &mut impl Hooks, round: Round) -> Round {
    h.delay_ms(RETRY_DELAY_MS);
    round
}

/// One round of the task loop, including its delay.
pub fn round(h: &mut impl Hooks, tag: &core::ffi::CStr) -> Round {
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
    let Some(ip) = h.resolve(&host) else {
        log!(Error, tag, "{host} couldn't be resolved!");
        return retry_later(h, Round::ResolveFailed);
    };

    log!(
        Info,
        tag,
        "Connecting to: stratum+tcp://{host}:{port} ({ip})"
    );
    if !h.connect(&host, &ip, port) {
        return retry_later(h, Round::ConnectFailed);
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

mod ffi {

    // SAFETY (all): the C++ side provides valid callbacks for its context.
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Fake {
        host: String,
        stopped: bool,
        network: bool,
        dns: bool,
        connects: bool,
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
            (self.host.clone(), 3333)
        }
        fn stopped(&mut self) -> bool {
            self.stopped
        }
        fn network_up(&mut self) -> bool {
            self.network
        }
        fn resolve(&mut self, _host: &str) -> Option<String> {
            self.dns.then(|| "10.0.0.1".into())
        }
        fn connect(&mut self, host: &str, ip: &str, port: u16) -> bool {
            self.log.push(format!("connect {host} {ip} {port}"));
            self.connects
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
            network: true,
            dns: true,
            connects: true,
            ..Default::default()
        }
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
            (Fake { dns: false, ..up() }, Round::ResolveFailed),
            (
                Fake {
                    connects: false,
                    ..up()
                },
                Round::ConnectFailed,
            ),
        ] {
            assert_eq!(round(&mut f, tag), expected);
            assert_eq!(f.log.last().unwrap(), "delay 10000");
            assert!(!f.log.iter().any(|l| l == "protocol"));
        }
    }

    #[test]
    fn connection_lost_counts_an_error_and_waits() {
        let mut f = up();
        assert_eq!(
            round(&mut f, c"t"),
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
        assert_eq!(round(&mut f, c"t"), Round::Disconnected { reconnect: true });
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
}
