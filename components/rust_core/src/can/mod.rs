//! The CAN bus glue (port of `main/tasks/can_task.cpp`, `can_sender.cpp`,
//! `can_master_task.cpp` and `can_slave_task.cpp`): the TWAI driver, the
//! frames sent, the master's receive task and the slave's three tasks. The
//! protocol state (master registry, slave negotiation, reassembly) was
//! Rust already (`can/master.rs`, `can/slave.rs`).
//!
//! The TX mutex is a static, and a bus-off recovery waits for the stopped
//! state the driver enters after recovery and restarts it.

pub mod master;
pub mod slave;

use crate::idf::idf_const;
use core::ffi::{c_int, c_void, CStr};
use std::sync::Mutex;
use std::time::Duration;

use crate::can::master::{api as master_api, SLAVE_MAX};
use crate::can::slave::api as slave_api;
use crate::idf::nvs::{err_name, Nvs};
use crate::idf::twai::{self, Message};
use crate::idf::{ms_to_ticks, EspErr, ESP_OK};
use crate::log::log;
use crate::mining::jobs::JOB_TABLES;
use crate::mining::nonce_difficulty;

const TAG: &CStr = c"can";
const TAG_SENDER: &CStr = c"can_sender";
const TAG_MASTER: &CStr = c"can_master";
const TAG_SLAVE: &CStr = c"can_slave";

const ID_HELLO: u32 = master::ID_HELLO;
const ID_ASSIGN: u32 = slave::ID_ASSIGN;
const ID_MASTER_BOOT: u32 = slave::ID_MASTER_BOOT;
const ID_JOB_BASE: u32 = slave::ID_JOB_BASE;
const ID_NONCE_BASE: u32 = master::ID_NONCE_BASE;
const ID_TELEMETRY_BASE: u32 = master::ID_TELEMETRY_BASE;
const ID_SETTINGS_BASE: u32 = slave::ID_SETTINGS_BASE;
const ID_CONFIG_BASE: u32 = master::ID_CONFIG_BASE;
const SEQ_LAST: u8 = 0xff;

/// The settings commands (master -> slave, one frame).
pub const CMD_SET_FREQ: u8 = 0x01;
pub const CMD_SET_VOLTAGE: u8 = 0x02;
pub const CMD_SET_FAN: u8 = 0x03;
pub const CMD_SET_DISPLAY: u8 = 0x04;
pub const CMD_GET_CONFIG: u8 = 0x05;
pub const CMD_SHUTDOWN: u8 = 0xFD;
pub const CMD_IDENTIFY: u8 = 0xFE;
pub const CMD_RESTART: u8 = 0xFF;

const TELEMETRY_VERSION: u8 = 5;

idf_const!(ESP_ERR_INVALID_STATE: EspErr = ESP_ERR_INVALID_STATE, 0x103);
idf_const!(ESP_ERR_TIMEOUT: EspErr = ESP_ERR_TIMEOUT, 0x107);
idf_const!(MAC_WIFI_STA: u32 = esp_mac_type_t_ESP_MAC_WIFI_STA, 0);
idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::esp_read_mac;
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn esp_read_mac(_: *mut u8, _: u32) -> EspErr {
        crate::idf::ESP_FAIL
    }
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

fn mac_str(mac: &[u8]) -> String {
    mac.iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

fn sta_mac() -> [u8; 6] {
    let mut mac = [0u8; 6];
    // SAFETY: mac is a 6-byte buffer, the size esp_read_mac writes for a station MAC
    unsafe { sys::esp_read_mac(mac.as_mut_ptr(), MAC_WIFI_STA) };
    mac
}

fn status_line(tag: &CStr) {
    if let Some(st) = twai::status() {
        log!(
            Info,
            tag,
            "TWAI state={} tx_err={} rx_err={} msgs_to_tx={} msgs_to_rx={} tx_fail={}",
            st.state,
            st.tx_error_counter,
            st.rx_error_counter,
            st.msgs_to_tx,
            st.msgs_to_rx,
            st.tx_failed_count
        );
    }
}

// ---------------------------------------------------------------------------
// driver

/// The TWAI peripheral at 500 kbit/s (`can_init()`).
pub fn init(tx: c_int, rx: c_int) {
    // 500 kbit/s, a 32-frame RX queue (telemetry bursts), shared interrupt
    let e = twai::install_500k(tx, rx);
    if e != ESP_OK {
        log!(Error, TAG, "twai_driver_install failed: {}", err_name(e));
        return;
    }
    let e = twai::start();
    if e != ESP_OK {
        log!(Error, TAG, "twai_start failed: {}", err_name(e));
        twai::driver_uninstall();
        return;
    }
    log!(
        Info,
        TAG,
        "TWAI ready at 500 kbit/s (TX=GPIO{tx} RX=GPIO{rx})"
    );

    // reading the status after the start seems to stabilise the controller
    // on the ESP32-S3 (the tx_err=128 issue)
    sleep_ms(100);
    if let Some(st) = twai::status() {
        log!(
            Info,
            TAG,
            "TWAI initial state={} tx_err={} rx_err={}",
            st.state,
            st.tx_error_counter,
            st.rx_error_counter
        );
    }
}

// ---------------------------------------------------------------------------
// sending

/// Multiframe messages go out whole (several tasks send).
static TX: Mutex<()> = Mutex::new(());

/// One frame; on a failure a bus-off recovery or a restart of a stopped
/// driver (the frame is dropped then).
fn transmit(frame: &Message) -> EspErr {
    let e = twai::transmit(frame, ms_to_ticks(50));
    if e == ESP_OK {
        return ESP_OK;
    }
    match (e, twai::status()) {
        (ESP_ERR_INVALID_STATE, Some(st)) => {
            if st.state == twai::STATE_BUS_OFF {
                log!(
                    Warn,
                    TAG_SENDER,
                    "BUS_OFF detected (tx_err={}), initiating recovery",
                    st.tx_error_counter
                );
                twai::initiate_recovery();
                // after the recovery the driver is stopped
                let recovered = (0..20).any(|_| {
                    sleep_ms(50);
                    twai::status().is_some_and(|s| s.state == twai::STATE_STOPPED)
                });
                if recovered {
                    log!(Info, TAG_SENDER, "CAN bus recovered");
                } else {
                    log!(
                        Warn,
                        TAG_SENDER,
                        "CAN bus recovery timed out, restarting TWAI"
                    );
                }
            }
            // stopped (after a recovery): start again
            if twai::status().is_some_and(|s| s.state == twai::STATE_STOPPED) {
                if twai::start() == ESP_OK {
                    log!(Info, TAG_SENDER, "TWAI restarted from STOPPED state");
                } else {
                    log!(Error, TAG_SENDER, "TWAI restart failed");
                }
            }
        }
        (ESP_ERR_INVALID_STATE, None) => {}
        (ESP_ERR_TIMEOUT, st) => {
            // queue full or no ACK: drop the stale frames
            if let Some(st) = st {
                log!(
                    Warn,
                    TAG_SENDER,
                    "TX timeout id=0x{:03X} (state={} tx_err={} rx_err={} q={})",
                    frame.identifier,
                    st.state,
                    st.tx_error_counter,
                    st.rx_error_counter,
                    st.msgs_to_tx
                );
                if st.msgs_to_tx > 0 {
                    twai::clear_transmit_queue();
                }
            }
        }
        _ => log!(
            Warn,
            TAG_SENDER,
            "TX failed id=0x{:03X}: {}",
            frame.identifier,
            err_name(e)
        ),
    }
    e
}

/// `data` in frames of a sequence byte and up to 7 bytes; the last one
/// has `SEQ_LAST`.
fn frames(id: u32, data: &[u8]) -> Vec<Message> {
    let n = data.len().div_ceil(7);
    data.chunks(7)
        .enumerate()
        .map(|(i, chunk)| {
            let mut b = [0u8; 8];
            b[0] = if i + 1 == n { SEQ_LAST } else { i as u8 };
            b[1..=chunk.len()].copy_from_slice(chunk);
            twai::message(id, &b[..=chunk.len()])
        })
        .collect()
}

fn send_multiframe(id: u32, data: &[u8]) {
    let _g = TX.lock().unwrap_or_else(|e| e.into_inner());
    for f in frames(id, data) {
        transmit(&f);
    }
}

fn send_single(id: u32, data: &[u8]) -> bool {
    transmit(&twai::message(id, data)) == ESP_OK
}

fn send_hello(mac: &[u8; 6]) {
    if send_single(ID_HELLO, mac) {
        log!(Info, TAG_SENDER, "TX HELLO MAC={}", mac_str(mac));
    }
}

fn send_assign(mac: &[u8; 6], id: u8) {
    let mut d = [0u8; 7];
    d[..6].copy_from_slice(mac);
    d[6] = id;
    if send_single(ID_ASSIGN, &d) {
        log!(Info, TAG_SENDER, "TX ASSIGN MAC={} → id={id}", mac_str(mac));
    }
}

fn send_master_boot(mac: &[u8; 6]) {
    if send_single(ID_MASTER_BOOT, mac) {
        log!(Info, TAG_SENDER, "TX MASTER_BOOT MAC={}", mac_str(mac));
    }
}

/// A settings command (`CMD_*` and its arguments) to a slave.
pub fn send_settings_cmd(slave_id: u8, payload: &[u8]) {
    send_multiframe(ID_SETTINGS_BASE | (slave_id & 0x7f) as u32, payload);
}

/// A job for a slave's ASICs: the `BM1368_job` bytes the chips get and the
/// pool difficulty (the slave filters its nonces with it).
pub fn send_raw_job(slave_id: u8, job_id: u8, job: &crate::mining::jobs::BmJob) {
    let mut p = [0u8; slave::JOB_LEN + 4];
    p[0] = job_id;
    p[1] = 0x01; // midstates
    p[2..6].copy_from_slice(&job.starting_nonce.to_le_bytes());
    p[6..10].copy_from_slice(&job.target.to_le_bytes());
    p[10..14].copy_from_slice(&job.ntime.to_le_bytes());
    p[14..46].copy_from_slice(&job.merkle_root_be);
    p[46..78].copy_from_slice(&job.prev_block_hash_be);
    p[78..82].copy_from_slice(&job.version.to_le_bytes());
    p[82..86].copy_from_slice(&job.pool_diff.to_le_bytes());
    send_multiframe(ID_JOB_BASE | (slave_id & 0x7f) as u32, &p);
}

// ---------------------------------------------------------------------------
// master

const REGISTRY_NAMESPACE: &CStr = c"can_master";
const REGISTRY_KEY: &CStr = c"reg";

pub fn set_slave_freq(slave: u8, mhz: u16) {
    let f = mhz.to_le_bytes();
    send_settings_cmd(slave, &[CMD_SET_FREQ, f[0], f[1]]);
}
pub fn set_slave_voltage(slave: u8, mv: u16) {
    let v = mv.to_le_bytes();
    send_settings_cmd(slave, &[CMD_SET_VOLTAGE, v[0], v[1]]);
}
pub fn set_slave_fan(slave: u8, ch: u8, mode: u8, speed: u8, target: u8, overheat: u8) {
    send_settings_cmd(slave, &[CMD_SET_FAN, ch, mode, speed, target, overheat]);
}
pub fn set_slave_display(slave: u8, flip: u8, auto_off: u8) {
    send_settings_cmd(slave, &[CMD_SET_DISPLAY, flip, auto_off]);
}
pub fn shutdown_slave(slave: u8) {
    send_settings_cmd(slave, &[CMD_SHUTDOWN]);
}
pub fn identify_slave(slave: u8) {
    send_settings_cmd(slave, &[CMD_IDENTIFY]);
}
pub fn restart_slave(slave: u8) {
    send_settings_cmd(slave, &[CMD_RESTART]);
}

pub fn delete_slave(slave: u8) {
    if slave as usize >= SLAVE_MAX {
        return;
    }
    master_api::can_master_delete(slave);
    save_registry();
    log!(Info, TAG_MASTER, "Deleted slave id={slave} from registry");
}

fn load_registry() {
    let Ok(nvs) = Nvs::open(REGISTRY_NAMESPACE, crate::idf::nvs::READONLY) else {
        return;
    };
    // a blob longer than the registry does not load (as nvs_get_blob()
    // into the registry buffer)
    if let Ok(blob) = nvs.get_blob(REGISTRY_KEY) {
        if blob.len() <= master::REGISTRY_LEN {
            master::load_registry(&blob);
        }
    }
}

fn save_registry() {
    let Ok(nvs) = Nvs::open(REGISTRY_NAMESPACE, crate::idf::nvs::READWRITE) else {
        log!(Warn, TAG_MASTER, "NVS open failed, registry not saved");
        return;
    };
    let blob = master::registry();
    nvs.set_blob(REGISTRY_KEY, &blob);
    nvs.commit();
}

/// A nonce of a slave: checked and submitted like the local ASICs' ones.
fn handle_nonce(slave_id: u8, nonce: u32, rolled_version: u32, job_id: u8) {
    log!(
        Info,
        TAG_MASTER,
        "slave {slave_id} nonce={nonce:08X} job_id={job_id:02X}"
    );
    let Some(j) = JOB_TABLES
        .get(1 + slave_id as usize)
        .and_then(|t| t.clone_job(job_id))
    else {
        log!(
            Warn,
            TAG_MASTER,
            "slave {slave_id} unknown job_id 0x{job_id:02X}"
        );
        return;
    };
    let version = rolled_version | j.version;
    let diff = nonce_difficulty(
        version,
        &j.prev_block_hash,
        &j.merkle_root,
        j.ntime,
        j.target,
        nonce,
    );
    let pool_str = if j.pool_id != 0 { "Sec" } else { "Pri" };
    let max = board_info().max_difficulty;
    log!(
        Info,
        TAG_MASTER,
        "({pool_str}) slave={slave_id} job={job_id:02X} nonce={nonce:08X} diff={}/pool={}/asic={max}",
        crate::util::cutil::fixed(diff, 1),
        j.pool_diff
    );

    // the share history needs the slave's chip number, which is not sent
    if diff >= j.pool_diff as f64 {
        if let Some(m) = crate::pool::manager::get() {
            m.submit_share(
                (j.pool_id & 1) as usize,
                j.job_id(),
                j.extranonce2(),
                j.ntime,
                nonce,
                version,
                j.version,
            );
        }
    }
    crate::pool::manager::api::pool_mgr_check_best_diff(j.pool_id, diff, j.target);
    crate::pool::manager::api::pool_mgr_check_found_block(j.pool_id, diff, j.target);
}

fn master_loop() -> ! {
    let mac = sta_mac();
    log!(Info, TAG_MASTER, "CAN master MAC={}", mac_str(&mac));
    load_registry();
    log!(Info, TAG_MASTER, "CAN master receiver started");
    // running slaves negotiate again
    send_master_boot(&mac);

    let mut timeouts = 0;
    loop {
        let mut msg = Message::default();
        let e = twai::receive(&mut msg, ms_to_ticks(100));
        if e == ESP_ERR_TIMEOUT {
            timeouts += 1;
            // about 1 s
            if timeouts >= 10 {
                timeouts = 0;
                master_api::can_master_tick(crate::util::cutil::now_us());
                status_line(TAG_MASTER);
            }
            continue;
        }
        if e != ESP_OK {
            log!(Warn, TAG_MASTER, "twai_receive error: {}", err_name(e));
            continue;
        }

        let ev = master::frame(
            msg.identifier,
            msg.data_length_code,
            &msg.data,
            crate::util::cutil::now_us(),
        );
        match ev.kind {
            master::EVENT_ASSIGN => {
                if ev.save_registry {
                    save_registry();
                }
                send_assign(&ev.mac, ev.slave_id);
                if ev.request_config {
                    send_settings_cmd(ev.slave_id, &[CMD_GET_CONFIG]);
                }
            }
            master::EVENT_NONCE => {
                handle_nonce(ev.slave_id, ev.nonce, ev.rolled_version, ev.job_id)
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// slave

/// The live telemetry (`can_slave_telemetry_t`, 49 bytes, offsets as read
/// by `can/master.rs`).
fn telemetry() -> [u8; master::TELEMETRY_LEN] {
    use crate::power::mgmt::{fan_get, get as pm};
    let mut t = [0u8; master::TELEMETRY_LEN];
    let mut put = |at: usize, b: &[u8]| t[at..at + b.len()].copy_from_slice(b);
    put(0, &[TELEMETRY_VERSION]);
    put(1, &pm(3).to_le_bytes());
    put(5, &pm(4).to_le_bytes());
    for i in 0..4 {
        let temp = crate::board::board_core::CHIP_TEMPS.get(i as c_int);
        put(9 + 4 * i, &temp.to_le_bytes());
    }
    put(25, &fan_get(0, 0).to_le_bytes());
    put(27, &fan_get(1, 0).to_le_bytes());
    put(29, &[fan_get(0, 1) as u8, fan_get(1, 1) as u8]);
    put(31, &pm(0).to_le_bytes());
    put(35, &(pm(2) as u16).to_le_bytes());
    let vout = crate::board::api::board_power_value(3);
    put(37, &((vout * 1000.0) as u16).to_le_bytes());
    put(
        39,
        &crate::mining::hashrate::api::hashrate_monitor_total().to_le_bytes(),
    );
    put(
        43,
        &[
            crate::power::mgmt::is_shutdown() as u8,
            crate::system::error().0 as u8,
        ],
    );
    let heap = crate::idf::task::free_heap(CAP_INTERNAL) as u32;
    put(45, &heap.to_le_bytes());
    t
}

/// The device info and settings (`can_slave_config_t`, 46 bytes).
fn slave_config() -> [u8; master::CONFIG_LEN] {
    use crate::config::get_u16;
    let b = board_info();
    let mut c = [0u8; master::CONFIG_LEN];
    // NUL-terminated within 16 bytes
    let text = |s: &[u8]| s[..s.len().min(15)].to_vec();
    let model = text(b.device_model.to_bytes());
    c[..model.len()].copy_from_slice(&model);
    let fw = text(crate::net::mdns::fw_version().as_bytes());
    c[16..16 + fw.len()].copy_from_slice(&fw);
    let freq = get_u16(b"asicfrequency", b.default_frequency as u16);
    let mv = get_u16(b"asicvoltage", b.default_voltage_mv as u16);
    c[32..34].copy_from_slice(&freq.to_le_bytes());
    c[34..36].copy_from_slice(&mv.to_le_bytes());
    for ch in 0..2 {
        let target_key: &[u8] = if ch == 0 {
            b"pid_temp"
        } else {
            b"fan1_pid_temp"
        };
        let at = 36 + 4 * ch;
        c[at] = b.fan_mode[ch] as u8;
        c[at + 1] = crate::config::cfg::fan_manual_speed(ch) as u8;
        c[at + 2] = get_u16(target_key, b.pid[ch].target_temp) as u8;
        c[at + 3] = crate::config::cfg::fan_overheat_temp(ch) as u8;
    }
    c[44] = (get_u16(b"flipscreen", 0) != 0) as u8;
    c[45] = crate::config::cfg::auto_screen_off() as u8;
    c
}

fn board_info() -> crate::board::api::Info {
    crate::board::api::info()
}

fn send_slave_config(slave_id: u8) {
    send_multiframe(ID_CONFIG_BASE | (slave_id & 0x7f) as u32, &slave_config());
}

fn set_u16(key: &[u8], v: u16) {
    crate::config::set(
        key,
        crate::util::ajson::Json::Num(crate::util::ajson::Num::U(v as u64)),
    );
}

/// A settings command of the master.
fn handle_settings_cmd(p: &[u8]) {
    let Some(&cmd) = p.first() else {
        return;
    };
    let u16_at = |i: usize| u16::from_le_bytes([p[i], p[i + 1]]);
    let mut send_config = false;
    match cmd {
        CMD_SET_FREQ if p.len() >= 3 => {
            let freq = u16_at(1);
            set_u16(b"asicfrequency", freq);
            crate::board::api::board_load_settings();
            log!(Info, TAG_SLAVE, "CMD SET_FREQ {freq}MHz → applied");
            send_config = true;
        }
        CMD_SET_VOLTAGE if p.len() >= 3 => {
            let mv = u16_at(1);
            set_u16(b"asicvoltage", mv);
            crate::board::api::board_load_settings();
            log!(Info, TAG_SLAVE, "CMD SET_VOLTAGE {mv}mV → applied");
            send_config = true;
        }
        CMD_SET_FAN if p.len() >= 6 => {
            let (ch, mode, speed, target, overheat) = (p[1], p[2], p[3], p[4], p[5]);
            // channel 0 or else channel 1 (as Config::setFan*())
            let k = |k0: &'static [u8], k1: &'static [u8]| if ch == 0 { k0 } else { k1 };
            set_u16(k(b"autofanspeed", b"fan1mode"), mode as u16);
            set_u16(k(b"fanspeed", b"fan1speed"), speed as u16);
            set_u16(k(b"pid_temp", b"fan1_pid_temp"), target as u16);
            set_u16(k(b"overheat_temp", b"fan1_overheat"), overheat as u16);
            crate::power::mgmt::fan_load_settings();
            log!(
                Info,
                TAG_SLAVE,
                "CMD SET_FAN ch{ch} mode={mode} overheat={overheat} → applied"
            );
            send_config = true;
        }
        CMD_SET_DISPLAY if p.len() >= 3 => {
            set_u16(b"flipscreen", (p[1] != 0) as u16);
            set_u16(b"autoscreenoff", (p[2] != 0) as u16);
            crate::board::api::board_load_settings();
            log!(
                Info,
                TAG_SLAVE,
                "CMD SET_DISPLAY flip={} autoOff={} → applied",
                p[1],
                p[2]
            );
            send_config = true;
        }
        CMD_SET_FREQ | CMD_SET_VOLTAGE | CMD_SET_FAN | CMD_SET_DISPLAY => {}
        CMD_GET_CONFIG => send_config = true,
        CMD_SHUTDOWN => {
            log!(Warn, TAG_SLAVE, "CMD SHUTDOWN → shutting down");
            crate::power::mgmt::shutdown();
        }
        CMD_IDENTIFY => {
            log!(Info, TAG_SLAVE, "CMD IDENTIFY → blink display 60s");
            // 0: the default 60 s
            crate::display::send_identify(0);
        }
        CMD_RESTART => {
            log!(Info, TAG_SLAVE, "CMD RESTART → restarting");
            sleep_ms(100);
            crate::idf::task::restart()
        }
        _ => log!(Warn, TAG_SLAVE, "Unknown settings cmd 0x{cmd:02X}"),
    }
    if send_config {
        crate::config::nvs::request_flush();
        let id = slave_api::can_slave_id();
        if id != slave::UNASSIGNED {
            send_slave_config(id);
        }
    }
}

fn now_ms() -> u64 {
    (crate::util::cutil::now_us() / 1000) as u64
}

fn slave_loop() -> ! {
    let mac = sta_mac();
    // negotiation, job reassembly and store: can_slave.rs
    slave_api::can_slave_init(mac);
    loop {
        // HELLO every second while unassigned
        if slave_api::can_slave_hello_due(now_ms()) {
            send_hello(&mac);
            status_line(TAG_SLAVE);
        }

        let mut msg = Message::default();
        let e = twai::receive(&mut msg, ms_to_ticks(100));
        if e == ESP_ERR_TIMEOUT {
            continue;
        }
        if e != ESP_OK {
            log!(Warn, TAG_SLAVE, "twai_receive error: {}", err_name(e));
            continue;
        }

        let ev =
            slave_api::can_slave_frame(msg.identifier, msg.data_length_code, &msg.data, now_ms());
        match ev.kind {
            slave::EVENT_ASSIGNED => send_slave_config(ev.id),
            slave::EVENT_COMMAND => {
                let n = (ev.id as usize).min(ev.cmd.len());
                handle_settings_cmd(&ev.cmd[..n]);
            }
            slave::EVENT_JOB => {
                if let Some(a) = crate::board::get().and_then(|b| b.asic_handle()) {
                    a.run(|d, u| d.send_raw_job(u, &ev.job));
                }
            }
            _ => {}
        }
    }
}

/// A nonce back to the master: nonce, rolled version, job ID (9 bytes, 2
/// frames, without the TX lock as before).
fn send_nonce(slave_id: u8, r: &crate::mining::asic::TaskResult) {
    let mut b = [0u8; master::NONCE_LEN];
    b[..4].copy_from_slice(&r.nonce.to_le_bytes());
    b[4..8].copy_from_slice(&r.rolled_version.to_le_bytes());
    b[8] = r.job_id;
    let id = ID_NONCE_BASE | (slave_id & 0x7f) as u32;
    for f in frames(id, &b) {
        twai::transmit(&f, ms_to_ticks(50));
    }
}

fn slave_result_loop() -> ! {
    log!(Info, TAG_SLAVE, "CAN slave result task started");
    loop {
        let id = slave_api::can_slave_id();
        if id == slave::UNASSIGNED {
            sleep_ms(100);
            continue;
        }
        let Some(a) = crate::board::get().and_then(|b| b.asic_handle()) else {
            sleep_ms(100);
            continue;
        };
        let Some(r) = a.run(|d, u| d.process_work(u)) else {
            continue;
        };
        if r.is_reg_resp != 0 {
            match r.reg {
                0xb4 => {
                    if let Some(t) = crate::mining::tasks::chip_temp(r.data) {
                        crate::board::board_core::CHIP_TEMPS.set(r.asic_nr, t);
                    }
                }
                0x90 => crate::mining::hashrate::on_counter(r.asic_nr, r.data),
                _ => {}
            }
            continue;
        }
        // below the stored job's pool difficulty: not sent (can_slave.rs)
        if slave_api::can_slave_check_nonce(r.job_id, r.nonce, r.rolled_version) {
            send_nonce(id, &r);
        }
    }
}

fn slave_telemetry_loop() -> ! {
    log!(Info, TAG_SLAVE, "CAN slave telemetry task started");
    loop {
        sleep_ms(1000);
        let id = slave_api::can_slave_id();
        if id == slave::UNASSIGNED {
            continue;
        }
        send_multiframe(ID_TELEMETRY_BASE | (id & 0x7f) as u32, &telemetry());
    }
}

pub(crate) mod api {
    use super::*;

    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn can_master_task(_: *mut c_void) {
        master_loop()
    }

    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn can_slave_task(_: *mut c_void) {
        slave_loop()
    }

    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn can_slave_result_task(_: *mut c_void) {
        slave_result_loop()
    }

    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn can_slave_telemetry_task(_: *mut c_void) {
        slave_telemetry_loop()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame_bytes(f: &Message) -> Vec<u8> {
        f.data[..f.data_length_code as usize].to_vec()
    }

    #[test]
    fn multiframe_split() {
        // 49 bytes: 7 full frames, the last one marked
        let data: Vec<u8> = (0..49).collect();
        let f = frames(0x405, &data);
        assert_eq!(f.len(), 7);
        for (i, fr) in f.iter().enumerate() {
            assert_eq!(fr.identifier, 0x405);
            let b = frame_bytes(fr);
            assert_eq!(b.len(), 8);
            assert_eq!(b[0], if i == 6 { SEQ_LAST } else { i as u8 });
            assert_eq!(&b[1..], &data[i * 7..i * 7 + 7]);
        }
        // 9 bytes (a nonce): 7 + 2
        let f = frames(0x303, &[1, 2, 3, 4, 5, 6, 7, 8, 9]);
        assert_eq!(frame_bytes(&f[0]), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(frame_bytes(&f[1]), [SEQ_LAST, 8, 9]);
        // a single-frame command
        let f = frames(0x501, &[CMD_IDENTIFY]);
        assert_eq!(f.len(), 1);
        assert_eq!(frame_bytes(&f[0]), [SEQ_LAST, CMD_IDENTIFY]);
    }
}
