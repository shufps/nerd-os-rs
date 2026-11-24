//! The CAN master's receive side (port of `can_master_task.cpp`): the slave
//! registry (HELLO → ASSIGN, persisted as a blob), the reassembly of the
//! multi-frame messages from the slaves, the decoded telemetry and config
//! per slave, the online timeouts and the slaves' total hashrate. The TWAI
//! driver, NVS and sending: `can.rs`.
//!
//! Behaviour notes:
//! - a frame's payload is at most the 8 data bytes: the TWAI HAL reports
//!   data length codes up to 15 but copies only 8 bytes, so the length is
//!   limited to the data field;
//! - everything is behind one lock: the HTTP handlers read the registry and
//!   telemetry (and delete slaves) while the master task writes them.

use core::ffi::CStr;
use std::sync::Mutex;

use crate::log::log;
use crate::util::cutil::fixed;

const TAG: &CStr = c"can_master";

pub const SLAVE_MAX: usize = 32;
pub const ID_HELLO: u32 = 0x100;
pub const ID_NONCE_BASE: u32 = 0x300;
pub const ID_TELEMETRY_BASE: u32 = 0x400;
pub const ID_CONFIG_BASE: u32 = 0x700;
pub const SEQ_LAST: u8 = 0xff;
/// `can_slave_telemetry_t` (packed)
pub const TELEMETRY_LEN: usize = 49;
/// `can_slave_config_t` (packed)
pub const CONFIG_LEN: usize = 46;
pub const NONCE_LEN: usize = 9;
/// Registry blob: per slot MAC[6] + used[1]; slot 0 is the master.
pub const REGISTRY_LEN: usize = SLAVE_MAX * 7;
const TIMEOUT_US: i64 = 10_000_000;

/// Field offsets in `can_slave_telemetry_t`.
mod tel {
    pub const TEMP: usize = 1;
    pub const VR_TEMP: usize = 5;
    pub const FAN_RPM: usize = 25;
    pub const FAN_RPM2: usize = 27;
    pub const FAN_SPEED: usize = 29;
    pub const FAN_SPEED2: usize = 30;
    pub const POWER: usize = 31;
    pub const CURRENT: usize = 35;
    pub const VOUT: usize = 37;
    pub const HASHRATE: usize = 39;
    pub const SHUTDOWN: usize = 43;
    #[cfg(test)]
    pub const BOARD_ERROR: usize = 44;
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

#[derive(Clone, Copy)]
struct Slot {
    mac: [u8; 6],
    used: bool,
    online: bool,
    last_telemetry_us: i64,
    telemetry: [u8; TELEMETRY_LEN],
    config: [u8; CONFIG_LEN],
}

impl Slot {
    const EMPTY: Slot = Slot {
        mac: [0; 6],
        used: false,
        online: false,
        last_telemetry_us: 0,
        telemetry: [0; TELEMETRY_LEN],
        config: [0; CONFIG_LEN],
    };
}

/// A message being reassembled.
#[derive(Clone, Copy)]
struct Rx {
    buf: [u8; TELEMETRY_LEN],
    len: usize,
    in_frame: bool,
}

impl Rx {
    const EMPTY: Rx = Rx {
        buf: [0; TELEMETRY_LEN],
        len: 0,
        in_frame: false,
    };
}

pub struct Master {
    slots: [Slot; SLAVE_MAX],
    /// per slave: nonce, telemetry, config
    rx: [[Rx; 3]; SLAVE_MAX],
}

/// What the C++ side has to do after a frame.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Event {
    /// `CAN_EVENT_*`
    pub kind: u8,
    pub slave_id: u8,
    pub mac: [u8; 6],
    /// ASSIGN: also request the slave's config (a known slave)
    pub request_config: bool,
    /// ASSIGN: the registry changed, save it
    pub save_registry: bool,
    /// NONCE
    pub nonce: u32,
    pub rolled_version: u32,
    pub job_id: u8,
}

pub const EVENT_NONE: u8 = 0;
pub const EVENT_ASSIGN: u8 = 1;
pub const EVENT_NONCE: u8 = 2;

impl Master {
    const fn new() -> Self {
        Self {
            slots: [Slot::EMPTY; SLAVE_MAX],
            rx: [[Rx::EMPTY; 3]; SLAVE_MAX],
        }
    }

    /// A slave announced itself: a known MAC gets its ID again, a new one
    /// the next free slot (1..).
    fn hello(&mut self, mac: [u8; 6]) -> Event {
        let mac_str = mac
            .iter()
            .map(|b| format!("{b:02X}"))
            .collect::<Vec<_>>()
            .join(":");
        if let Some(i) = (1..SLAVE_MAX).find(|&i| self.slots[i].used && self.slots[i].mac == mac) {
            log!(Info, TAG, "HELLO known slave MAC={mac_str} → id={i}");
            return Event {
                kind: EVENT_ASSIGN,
                slave_id: i as u8,
                mac,
                // the slave sends its config after ASSIGN; ask anyway in case
                // it is a re-negotiation and the slave skipped it
                request_config: true,
                ..Default::default()
            };
        }
        if let Some(i) = (1..SLAVE_MAX).find(|&i| !self.slots[i].used) {
            let s = &mut self.slots[i];
            s.mac = mac;
            s.used = true;
            log!(Info, TAG, "HELLO new slave MAC={mac_str} → id={i}");
            return Event {
                kind: EVENT_ASSIGN,
                slave_id: i as u8,
                mac,
                save_registry: true,
                ..Default::default()
            };
        }
        log!(
            Warn,
            TAG,
            "HELLO from slave but registry full (max {SLAVE_MAX})"
        );
        Event::default()
    }

    /// The slaves' total hashrate (GH/s) of the online ones.
    fn total_hashrate(&self) -> f32 {
        self.slots
            .iter()
            .filter(|s| s.used && s.online)
            .map(|s| f32_at(&s.telemetry, tel::HASHRATE))
            .sum()
    }

    /// A received frame; `data` is the frame's data field, `dlc` its data
    /// length code as the driver reports it (can exceed 8).
    pub fn frame(&mut self, id: u32, dlc: u8, data: &[u8; 8], now_us: i64) -> Event {
        if id == ID_HELLO && dlc == 6 {
            let mut mac = [0; 6];
            mac.copy_from_slice(&data[..6]);
            return self.hello(mac);
        }
        if dlc < 1 {
            return Event::default();
        }
        let base = id & 0xf80;
        let slave = (id & 0x7f) as usize;
        if slave >= SLAVE_MAX {
            return Event::default();
        }
        let (kind, max) = match base {
            ID_NONCE_BASE => (0, NONCE_LEN),
            ID_TELEMETRY_BASE => (1, TELEMETRY_LEN),
            ID_CONFIG_BASE => (2, CONFIG_LEN),
            _ => return Event::default(),
        };

        let seq = data[0];
        // at most the 7 payload bytes the frame has
        let payload = &data[1..(dlc as usize).min(8)];
        let rx = &mut self.rx[slave][kind];
        if seq == 0 {
            if rx.in_frame {
                log!(
                    Warn,
                    TAG,
                    "slave {slave} new seq=0 while in_frame (base=0x{base:03X}), discarding {} bytes",
                    rx.len
                );
            }
            rx.len = 0;
            rx.in_frame = true;
        } else if !rx.in_frame {
            return Event::default();
        }
        if rx.len + payload.len() > max {
            log!(
                Warn,
                TAG,
                "slave {slave} overflow (base=0x{base:03X}), discarding"
            );
            rx.in_frame = false;
            return Event::default();
        }
        rx.buf[rx.len..rx.len + payload.len()].copy_from_slice(payload);
        rx.len += payload.len();
        if seq != SEQ_LAST {
            return Event::default();
        }
        rx.in_frame = false;
        let msg = rx.buf;
        let len = rx.len;

        match kind {
            0 => {
                if len != NONCE_LEN {
                    log!(Warn, TAG, "slave {slave} unexpected nonce len {len}");
                    return Event::default();
                }
                Event {
                    kind: EVENT_NONCE,
                    slave_id: slave as u8,
                    nonce: u32::from_le_bytes(msg[0..4].try_into().unwrap()),
                    rolled_version: u32::from_le_bytes(msg[4..8].try_into().unwrap()),
                    job_id: msg[8],
                    ..Default::default()
                }
            }
            1 => {
                if len < TELEMETRY_LEN {
                    log!(
                        Warn,
                        TAG,
                        "slave {slave} telemetry too short {len} (expected {TELEMETRY_LEN})"
                    );
                    return Event::default();
                }
                let s = &mut self.slots[slave];
                s.telemetry.copy_from_slice(&msg[..TELEMETRY_LEN]);
                s.last_telemetry_us = now_us;
                s.online = true;
                let t = &s.telemetry;
                log!(
                    Info,
                    TAG,
                    "slave {slave} | hr={}GH/s | temp={}°C vr={}°C | fan0={}RPM({}%) fan1={}RPM({}%) | pwr={}W cur={}mA vout={}mV | {}",
                    fixed(f32_at(t, tel::HASHRATE) as f64, 1),
                    fixed(f32_at(t, tel::TEMP) as f64, 1),
                    fixed(f32_at(t, tel::VR_TEMP) as f64, 1),
                    u16_at(t, tel::FAN_RPM),
                    t[tel::FAN_SPEED],
                    u16_at(t, tel::FAN_RPM2),
                    t[tel::FAN_SPEED2],
                    fixed(f32_at(t, tel::POWER) as f64, 1),
                    u16_at(t, tel::CURRENT),
                    u16_at(t, tel::VOUT),
                    if t[tel::SHUTDOWN] != 0 { "SHUTDOWN" } else { "ok" }
                );
                crate::mining::hashrate::set_external(self.total_hashrate());
                Event::default()
            }
            _ => {
                if len < CONFIG_LEN {
                    log!(
                        Warn,
                        TAG,
                        "slave {slave} config too short {len} (expected {CONFIG_LEN})"
                    );
                    return Event::default();
                }
                let s = &mut self.slots[slave];
                s.config.copy_from_slice(&msg[..CONFIG_LEN]);
                let c = &s.config;
                let text = |b: &[u8]| {
                    String::from_utf8_lossy(&b[..b.iter().position(|&x| x == 0).unwrap_or(b.len())])
                        .into_owned()
                };
                log!(
                    Info,
                    TAG,
                    "slave {slave} config: model={} fw={} freq={}MHz volt={}mV",
                    text(&c[0..16]),
                    text(&c[16..32]),
                    u16_at(c, 32),
                    u16_at(c, 34)
                );
                Event::default()
            }
        }
    }

    /// Marks slaves without telemetry for 10 s offline.
    pub fn tick(&mut self, now_us: i64) {
        let mut changed = false;
        for (i, s) in self.slots.iter_mut().enumerate().skip(1) {
            if !s.used || !s.online {
                continue;
            }
            let age = now_us - s.last_telemetry_us;
            if age > TIMEOUT_US {
                log!(
                    Warn,
                    TAG,
                    "slave {i} timeout after {}ms — marking offline",
                    age / 1000
                );
                s.online = false;
                s.telemetry = [0; TELEMETRY_LEN];
                changed = true;
            }
        }
        if changed {
            crate::mining::hashrate::set_external(self.total_hashrate());
        }
    }

    /// The registry from its blob (slot 0 is ignored); a blob of another
    /// size is ignored.
    pub fn load_registry(&mut self, blob: &[u8]) {
        if blob.len() != REGISTRY_LEN {
            return;
        }
        for i in 1..SLAVE_MAX {
            let slot = &blob[i * 7..i * 7 + 7];
            self.slots[i].used = slot[6] != 0;
            if self.slots[i].used {
                self.slots[i].mac.copy_from_slice(&slot[..6]);
                log!(
                    Info,
                    TAG,
                    "NVS restored id={i} MAC={}",
                    slot[..6]
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(":")
                );
            }
        }
    }

    pub fn registry(&self) -> [u8; REGISTRY_LEN] {
        let mut blob = [0; REGISTRY_LEN];
        for i in 1..SLAVE_MAX {
            blob[i * 7..i * 7 + 6].copy_from_slice(&self.slots[i].mac);
            blob[i * 7 + 6] = self.slots[i].used as u8;
        }
        blob
    }

    pub fn delete(&mut self, slave: usize) {
        if slave < SLAVE_MAX {
            self.slots[slave] = Slot::EMPTY;
        }
    }

    fn slot(&self, slave: usize) -> Option<&Slot> {
        self.slots.get(slave)
    }
}

static MASTER: Mutex<Master> = Mutex::new(Master::new());

/// A received frame (the CAN task); what it means for the caller.
pub fn frame(id: u32, dlc: u8, data: &[u8; 8], now_us: i64) -> Event {
    with(|m| m.frame(id, dlc, data, now_us))
}

/// The periodic step (timeouts).
pub fn tick(now_us: i64) {
    with(|m| m.tick(now_us));
}

/// The slave registry from its saved form.
pub fn load_registry(blob: &[u8]) {
    with(|m| m.load_registry(blob));
}

/// Forgets a slave.
pub fn delete(slave: u8) {
    with(|m| m.delete(slave as usize));
}

/// The registry in its saved form.
pub fn registry() -> [u8; REGISTRY_LEN] {
    with(|m| m.registry())
}

/// What `f` reads from a known slave.
fn known<T>(slave: u8, f: impl FnOnce(&Slot) -> T) -> Option<T> {
    with(|m| m.slot(slave as usize).filter(|s| s.used).map(f))
}

/// The MAC of a known slave.
pub fn slave_mac(slave: u8) -> Option<[u8; 6]> {
    known(slave, |s| s.mac)
}

/// The last telemetry (`can_slave_telemetry_t`) of a known slave.
pub fn slave_telemetry(slave: u8) -> Option<[u8; TELEMETRY_LEN]> {
    known(slave, |s| s.telemetry)
}

/// The last config (`can_slave_config_t`) of a known slave.
pub fn slave_config(slave: u8) -> Option<[u8; CONFIG_LEN]> {
    known(slave, |s| s.config)
}

fn with<T>(f: impl FnOnce(&mut Master) -> T) -> T {
    f(&mut MASTER.lock().unwrap_or_else(|e| e.into_inner()))
}

pub(crate) mod api {
    use super::*;

    pub fn can_master_tick(now_us: i64) {
        tick(now_us);
    }

    pub fn can_master_delete(slave: u8) {
        delete(slave);
    }

    pub fn can_master_is_slave_active(slave: u8) -> bool {
        with(|m| m.slot(slave as usize).is_some_and(|s| s.used && s.online))
    }

    pub fn can_master_is_slave_known(slave: u8) -> bool {
        with(|m| m.slot(slave as usize).is_some_and(|s| s.used))
    }

    /// (never set: foreign slaves are not detected yet)
    pub fn can_master_is_slave_foreign(_slave: u8) -> bool {
        false
    }

    /// Sum of the power (W) the known slaves report.
    pub fn can_master_get_slave_fleet_power() -> f32 {
        with(|m| {
            m.slots[1..]
                .iter()
                .filter(|s| s.used)
                .map(|s| f32_at(&s.telemetry, tel::POWER))
                .sum()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(m: &mut Master, base: u32, slave: u32, msg: &[u8], now: i64) -> Event {
        let chunks: Vec<&[u8]> = msg.chunks(7).collect();
        let mut ev = Event::default();
        for (i, c) in chunks.iter().enumerate() {
            let seq = if i == chunks.len() - 1 {
                SEQ_LAST
            } else {
                i as u8
            };
            let mut data = [0u8; 8];
            data[0] = seq;
            data[1..1 + c.len()].copy_from_slice(c);
            ev = m.frame(base | slave, 1 + c.len() as u8, &data, now);
        }
        ev
    }

    fn telemetry(hashrate: f32, power: f32, shutdown: u8, error: u8) -> [u8; TELEMETRY_LEN] {
        let mut t = [0u8; TELEMETRY_LEN];
        t[0] = 5;
        t[tel::HASHRATE..tel::HASHRATE + 4].copy_from_slice(&hashrate.to_le_bytes());
        t[tel::POWER..tel::POWER + 4].copy_from_slice(&power.to_le_bytes());
        t[tel::SHUTDOWN] = shutdown;
        t[tel::BOARD_ERROR] = error;
        t
    }

    #[test]
    fn hello_assigns_and_remembers() {
        let mut m = Master::new();
        let mac = [1, 2, 3, 4, 5, 6];
        let mut data = [0u8; 8];
        data[..6].copy_from_slice(&mac);
        let ev = m.frame(ID_HELLO, 6, &data, 0);
        assert_eq!(
            (ev.kind, ev.slave_id, ev.save_registry, ev.request_config),
            (EVENT_ASSIGN, 1, true, false)
        );
        // known again: same ID, config requested, nothing to save
        let ev = m.frame(ID_HELLO, 6, &data, 0);
        assert_eq!(
            (ev.kind, ev.slave_id, ev.save_registry, ev.request_config),
            (EVENT_ASSIGN, 1, false, true)
        );
        // registry blob round trip
        let blob = m.registry();
        assert_eq!(&blob[7..14], &[1, 2, 3, 4, 5, 6, 1]);
        let mut m2 = Master::new();
        m2.load_registry(&blob);
        assert!(m2.slots[1].used && m2.slots[1].mac == mac);
        // full registry
        for i in 2..SLAVE_MAX {
            data[0] = i as u8;
            assert_eq!(m.frame(ID_HELLO, 6, &data, 0).slave_id, i as u8);
        }
        data[0] = 0xee;
        assert_eq!(m.frame(ID_HELLO, 6, &data, 0).kind, EVENT_NONE);
    }

    #[test]
    fn nonce_reassembly() {
        let mut m = Master::new();
        let msg = [0x78, 0x56, 0x34, 0x12, 0x00, 0x20, 0x00, 0x00, 0x2a];
        let ev = frames(&mut m, ID_NONCE_BASE, 3, &msg, 0);
        assert_eq!(
            (ev.kind, ev.slave_id, ev.nonce, ev.rolled_version, ev.job_id),
            (EVENT_NONCE, 3, 0x1234_5678, 0x2000, 0x2a)
        );
        // continuation without a start: ignored
        let data = [SEQ_LAST, 1, 2, 3, 4, 5, 6, 7];
        assert_eq!(m.frame(ID_NONCE_BASE | 3, 8, &data, 0).kind, EVENT_NONE);
        // too long: discarded
        let ev = frames(&mut m, ID_NONCE_BASE, 3, &[0; 14], 0);
        assert_eq!(ev.kind, EVENT_NONE);
        // slave ID out of range, unknown base
        assert_eq!(frames(&mut m, ID_NONCE_BASE, 40, &msg, 0).kind, EVENT_NONE);
        assert_eq!(frames(&mut m, 0x600, 3, &msg, 0).kind, EVENT_NONE);
    }

    #[test]
    fn dlc_over_8_reads_only_the_data_field() {
        // DLC 15: only the 8-byte data field is read (7 payload bytes)
        let mut m = Master::new();
        let mut data = [0xaa; 8];
        data[0] = 0;
        m.frame(ID_TELEMETRY_BASE | 1, 15, &data, 0);
        assert_eq!(m.rx[1][1].len, 7);
    }

    #[test]
    fn telemetry_hashrate_timeout_and_fleet() {
        let _g = crate::mining::hashrate::tests_lock();
        let mut m = Master::new();
        m.slots[1].used = true;
        m.slots[2].used = true;
        frames(
            &mut m,
            ID_TELEMETRY_BASE,
            1,
            &telemetry(100.0, 20.0, 0, 0),
            1_000_000,
        );
        frames(
            &mut m,
            ID_TELEMETRY_BASE,
            2,
            &telemetry(50.5, 10.0, 1, 3),
            1_000_000,
        );
        assert_eq!(m.total_hashrate(), 150.5);
        assert_eq!(crate::mining::hashrate::external(), 150.5);
        assert!(
            m.slots[2].telemetry[tel::SHUTDOWN] != 0 && m.slots[1].telemetry[tel::SHUTDOWN] == 0
        );
        // too short: ignored
        frames(&mut m, ID_TELEMETRY_BASE, 1, &[0; 20], 2_000_000);
        assert_eq!(m.total_hashrate(), 150.5);
        // slave 2 times out
        m.slots[1].last_telemetry_us = 10_000_000;
        m.tick(11_500_000);
        assert!(!m.slots[2].online && m.slots[1].online);
        assert_eq!(crate::mining::hashrate::external(), 100.0);
        m.delete(1);
        assert!(!m.slots[1].used);
    }

    #[test]
    fn config_message() {
        let mut m = Master::new();
        let mut c = [0u8; CONFIG_LEN];
        c[..5].copy_from_slice(b"Q1373");
        c[32..34].copy_from_slice(&525u16.to_le_bytes());
        frames(&mut m, ID_CONFIG_BASE, 4, &c, 0);
        assert_eq!(u16_at(&m.slots[4].config, 32), 525);
    }
}
