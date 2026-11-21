//! The network manager's decisions (port of `main/network/network_manager.cpp`):
//! which interface is preferred (Ethernet over WiFi), the access point
//! switched off for good with the first IP, WiFi stopped while Ethernet has
//! an IP and restarted when its link goes down, and the hooks on the first
//! IP and on a change of the preferred interface. The events come from the
//! WiFi/Ethernet drivers; the C++ side carries out the returned actions
//! (`esp_wifi_*`, `esp_netif_set_default_netif()`, event bits, hooks) in
//! their bit order, the order of the former code.
//!
//! The state is behind a lock: it is written by the event loop and read by
//! the HTTP and display tasks.

use core::ffi::c_int;
use std::sync::Mutex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    WifiGotIp,
    WifiDisconnected,
    EthGotIp,
    EthLinkDown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Iface {
    #[default]
    None = 0,
    Wifi = 1,
    Eth = 2,
}

/// Actions, carried out in this bit order.
pub mod act {
    pub const SET_WIFI_BIT: u32 = 1 << 0;
    pub const CLEAR_WIFI_BIT: u32 = 1 << 1;
    pub const SET_ETH_BIT: u32 = 1 << 2;
    pub const CLEAR_ETH_BIT: u32 = 1 << 3;
    /// the access point off (once)
    pub const AP_OFF: u32 = 1 << 4;
    /// `esp_wifi_disconnect()` + `esp_wifi_stop()`
    pub const STOP_WIFI: u32 = 1 << 5;
    /// `esp_wifi_start()`
    pub const START_WIFI: u32 = 1 << 6;
    /// `esp_netif_set_default_netif()` with the preferred interface
    pub const SET_DEFAULT: u32 = 1 << 7;
    /// the hook on the first IP
    pub const HOOK_ANY_IP: u32 = 1 << 8;
    /// the hook on a change of the preferred interface
    pub const HOOK_PREFERRED: u32 = 1 << 9;
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    pub wifi_ip: bool,
    pub eth_ip: bool,
    any_ip: bool,
    pub preferred: Iface,
    pub ap_off: bool,
    wifi_off_for_eth: bool,
}

impl State {
    /// The actions of an event; `eth_netif`/`wifi_netif`: the interfaces
    /// exist.
    pub fn event(&mut self, ev: Event, eth_netif: bool, wifi_netif: bool) -> u32 {
        let mut a = 0;
        match ev {
            Event::WifiGotIp => {
                self.wifi_ip = true;
                a |= act::SET_WIFI_BIT;
                a |= self.ap_off_once();
            }
            Event::WifiDisconnected => {
                self.wifi_ip = false;
                a |= act::CLEAR_WIFI_BIT;
            }
            Event::EthGotIp => {
                self.eth_ip = true;
                a |= act::SET_ETH_BIT;
                a |= self.ap_off_once();
                // Ethernet has an IP: WiFi is not needed
                if !self.wifi_off_for_eth {
                    a |= act::STOP_WIFI;
                    self.wifi_ip = false;
                    self.wifi_off_for_eth = true;
                }
            }
            Event::EthLinkDown => {
                self.eth_ip = false;
                a |= act::CLEAR_ETH_BIT;
                // WiFi as the fallback again
                if self.wifi_off_for_eth {
                    a |= act::START_WIFI;
                    self.wifi_off_for_eth = false;
                }
            }
        }
        a | self.update_route(eth_netif, wifi_netif)
    }

    fn ap_off_once(&mut self) -> u32 {
        if self.ap_off {
            return 0;
        }
        self.ap_off = true;
        act::AP_OFF
    }

    fn update_route(&mut self, eth_netif: bool, wifi_netif: bool) -> u32 {
        let preferred = if self.eth_ip && eth_netif {
            Iface::Eth
        } else if self.wifi_ip && wifi_netif {
            Iface::Wifi
        } else {
            Iface::None
        };
        let mut a = 0;
        let changed = preferred != self.preferred;
        self.preferred = preferred;
        if preferred != Iface::None {
            a |= act::SET_DEFAULT;
        }
        let any = self.wifi_ip || self.eth_ip;
        if any && !self.any_ip {
            a |= act::HOOK_ANY_IP;
        }
        self.any_ip = any;
        if changed {
            a |= act::HOOK_PREFERRED;
        }
        a
    }
}

static STATE: Mutex<State> = Mutex::new(State {
    wifi_ip: false,
    eth_ip: false,
    any_ip: false,
    preferred: Iface::None,
    ap_off: false,
    wifi_off_for_eth: false,
});

fn with<T>(f: impl FnOnce(&mut State) -> T) -> T {
    f(&mut STATE.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The access point is on.
pub fn ap_active() -> bool {
    with(|s| !s.ap_off)
}

/// WiFi or Ethernet has an address.
pub fn has_ip() -> bool {
    with(|s| s.wifi_ip || s.eth_ip)
}

pub(crate) mod api {
    use super::*;

    /// `ev`: 0 WiFi got an IP, 1 WiFi disconnected, 2 Ethernet got an IP,
    /// 3 Ethernet link down. The `NETMGR_ACT_*` bits to carry out, in bit
    /// order; the preferred interface (`NETMGR_*`) in `*preferred`.
    ///
    pub fn netmgr_event(ev: c_int, eth_netif: bool, wifi_netif: bool) -> (u32, c_int) {
        let ev = match ev {
            0 => Event::WifiGotIp,
            1 => Event::WifiDisconnected,
            2 => Event::EthGotIp,
            _ => Event::EthLinkDown,
        };
        let (a, p) = with(|s| (s.event(ev, eth_netif, wifi_netif), s.preferred));
        (a, p as c_int)
    }

    /// The access point off outside an event (startup with a configured
    /// pool): true if it is to be switched off now (the first time).
    pub fn netmgr_ap_off_once() -> bool {
        with(|s| s.ap_off_once() != 0)
    }

    pub fn netmgr_wifi_has_ip() -> bool {
        with(|s| s.wifi_ip)
    }

    pub fn netmgr_eth_has_ip() -> bool {
        with(|s| s.eth_ip)
    }
}

#[cfg(test)]
mod tests {
    use super::act::*;
    use super::*;

    #[test]
    fn wifi_only() {
        let mut s = State::default();
        // the first IP: AP off, WiFi preferred, both hooks
        assert_eq!(
            s.event(Event::WifiGotIp, false, true),
            SET_WIFI_BIT | AP_OFF | SET_DEFAULT | HOOK_ANY_IP | HOOK_PREFERRED
        );
        assert!(s.ap_off && s.preferred == Iface::Wifi);
        // a renewed IP: only the bit and the default route
        assert_eq!(
            s.event(Event::WifiGotIp, false, true),
            SET_WIFI_BIT | SET_DEFAULT
        );
        assert_eq!(
            s.event(Event::WifiDisconnected, false, true),
            CLEAR_WIFI_BIT | HOOK_PREFERRED
        );
        assert_eq!(s.preferred, Iface::None);
        // reconnected: the AP stays off; the any-IP hook fires again (rising)
        assert_eq!(
            s.event(Event::WifiGotIp, false, true),
            SET_WIFI_BIT | SET_DEFAULT | HOOK_ANY_IP | HOOK_PREFERRED
        );
        // a WiFi IP without the netif: not preferred
        let mut s = State::default();
        assert_eq!(
            s.event(Event::WifiGotIp, false, false),
            SET_WIFI_BIT | AP_OFF | HOOK_ANY_IP
        );
    }

    #[test]
    fn ethernet_takes_over_and_hands_back() {
        let mut s = State::default();
        s.event(Event::WifiGotIp, true, true);
        // Ethernet: WiFi stopped, Ethernet preferred
        assert_eq!(
            s.event(Event::EthGotIp, true, true),
            SET_ETH_BIT | STOP_WIFI | SET_DEFAULT | HOOK_PREFERRED
        );
        assert_eq!((s.preferred, s.wifi_ip), (Iface::Eth, false));
        // the disconnect the stop causes
        assert_eq!(
            s.event(Event::WifiDisconnected, true, true),
            CLEAR_WIFI_BIT | SET_DEFAULT
        );
        // a second Ethernet IP: WiFi is already off
        assert_eq!(
            s.event(Event::EthGotIp, true, true),
            SET_ETH_BIT | SET_DEFAULT
        );
        // link down: WiFi restarted, nothing preferred until it has an IP
        assert_eq!(
            s.event(Event::EthLinkDown, true, true),
            CLEAR_ETH_BIT | START_WIFI | HOOK_PREFERRED
        );
        assert_eq!(s.preferred, Iface::None);
        // another link down: WiFi is already on
        assert_eq!(s.event(Event::EthLinkDown, true, true), CLEAR_ETH_BIT);
        assert_eq!(
            s.event(Event::WifiGotIp, true, true),
            SET_WIFI_BIT | SET_DEFAULT | HOOK_ANY_IP | HOOK_PREFERRED
        );
        // Ethernet first: AP off with it
        let mut s = State::default();
        assert_eq!(
            s.event(Event::EthGotIp, true, true),
            SET_ETH_BIT | AP_OFF | STOP_WIFI | SET_DEFAULT | HOOK_ANY_IP | HOOK_PREFERRED
        );
    }
}
