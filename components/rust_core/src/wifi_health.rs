//! The network health log (port of `main/tasks/wifi_health.cpp`): every 5
//! minutes the lwIP protocol counters (reset after the first 5 minutes)
//! and the WiFi signal.

use core::ffi::{c_void, CStr};
use std::time::Duration;

use crate::log::log;

const TAG: &CStr = c"wifi_health";
const PERIOD: Duration = Duration::from_secs(300);

/// The protocol blocks of `lwip_stats`, in the order they are logged.
#[derive(Clone, Copy)]
enum Block {
    Ip,
    Tcp,
    Udp,
    Icmp,
    IpFrag,
    EthArp,
    Link,
}

const BLOCKS: [(&str, Block); 7] = [
    ("IP", Block::Ip),
    ("TCP", Block::Tcp),
    ("UDP", Block::Udp),
    ("ICMP", Block::Icmp),
    ("IPFRAG", Block::IpFrag),
    ("ETHARP", Block::EthArp),
    ("LINK", Block::Link),
];

/// The counters the log shows.
#[derive(Default)]
struct Counters {
    xmit: u16,
    recv: u16,
    drop: u16,
    err: u16,
    chkerr: u16,
    lenerr: u16,
    memerr: u16,
}

#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    use crate::idf::raw;

    /// The block (plain counters written by the lwIP task, read without a
    /// lock as before).
    fn proto(b: Block) -> *mut raw::stats_proto {
        unsafe {
            let s = core::ptr::addr_of_mut!(raw::lwip_stats);
            match b {
                Block::Ip => core::ptr::addr_of_mut!((*s).ip),
                Block::Tcp => core::ptr::addr_of_mut!((*s).tcp),
                Block::Udp => core::ptr::addr_of_mut!((*s).udp),
                Block::Icmp => core::ptr::addr_of_mut!((*s).icmp),
                Block::IpFrag => core::ptr::addr_of_mut!((*s).ip_frag),
                Block::EthArp => core::ptr::addr_of_mut!((*s).etharp),
                Block::Link => core::ptr::addr_of_mut!((*s).link),
            }
        }
    }
    pub fn counters(b: Block) -> Counters {
        let p = unsafe { proto(b).read_volatile() };
        Counters {
            xmit: p.xmit,
            recv: p.recv,
            drop: p.drop,
            err: p.err,
            chkerr: p.chkerr,
            lenerr: p.lenerr,
            memerr: p.memerr,
        }
    }
    pub fn reset(b: Block) {
        unsafe { proto(b).write_volatile(raw::stats_proto::default()) };
    }
    /// RSSI, SSID and channel of the access point while connected.
    pub fn ap_info() -> Option<(i8, Vec<u8>, u8)> {
        let mut r = raw::wifi_ap_record_t::default();
        (unsafe { raw::esp_wifi_sta_get_ap_info(&mut r) } == 0)
            .then(|| (r.rssi, crate::cutil::until_nul(&r.ssid).to_vec(), r.primary))
    }
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub fn counters(_: Block) -> Counters {
        Counters::default()
    }
    pub fn reset(_: Block) {}
    pub fn ap_info() -> Option<(i8, Vec<u8>, u8)> {
        None
    }
}

fn reset_stats() {
    for (_, b) in BLOCKS {
        sys::reset(b);
    }
}

fn log_health() {
    log!(Info, TAG, "------ WiFi / lwIP Health ------");
    for (name, b) in BLOCKS {
        let c = sys::counters(b);
        log!(
            Info,
            TAG,
            "{name}: xmit={}, recv={}, drop={}, err={}, chkerr={}, lenerr={}, memerr={}",
            c.xmit,
            c.recv,
            c.drop,
            c.err,
            c.chkerr,
            c.lenerr,
            c.memerr
        );
    }
    match sys::ap_info() {
        Some((rssi, ssid, channel)) => log!(
            Info,
            TAG,
            "RSSI: {rssi} dBm, SSID: {}, Channel: {channel}",
            String::from_utf8_lossy(&ssid)
        ),
        None => log!(Warn, TAG, "WiFi not connected (RSSI/SSID unavailable)"),
    }
    log!(Info, TAG, "-------------------------------");
}

pub(crate) mod ffi {
    use super::*;

    /// The task (`wifi_monitor_task`); never returns.
    ///
    /// # Safety
    /// A FreeRTOS task entry.
    #[no_mangle]
    pub unsafe extern "C" fn wifi_monitor_task(_: *mut c_void) {
        std::thread::sleep(PERIOD);
        reset_stats();
        loop {
            if crate::power_mgmt::is_shutdown() {
                log!(Warn, TAG, "suspended");
                crate::idf::task::suspend_self();
            }
            log_health();
            std::thread::sleep(PERIOD);
        }
    }
}
