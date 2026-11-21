//! The network (port of `main/network/connect.cpp` and
//! `network_manager.cpp`): the WiFi station and access point, the WiFi
//! scan, and the choice between WiFi and the W5500 Ethernet. The decisions
//! were Rust already (`wifi.rs` for the station, `netmgr.rs` for the
//! interfaces); this module carries out their actions on the IDF. The
//! WiFi structs come from the generated bindings; `wifi_init_config_t` is
//! `WIFI_INIT_CONFIG_DEFAULT()` built from the header's constants.
//!
//! The FreeRTOS event group that `wifi_connect()` / `wifi_wait_connected_ms()`
//! waited on is gone with them (nothing called them); waiting for any IP is
//! a condition variable.

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use core::ptr::NonNull;
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use crate::idf::nvs::err_name;
use crate::idf::{EspErr, ESP_OK};
use crate::log::log;
use crate::netmgr::{act as nact, ffi as netmgr};
use crate::wifi::{act, ffi as wifi_state};

/// A netif that lives for the rest of the firmware (the default WiFi ones,
/// the ETH one once its setup succeeded): only these reach the netif calls.
#[derive(Clone, Copy)]
pub struct NetifRef(NonNull<c_void>);

// SAFETY: esp_netif objects may be used from any task (esp_netif locks)
unsafe impl Send for NetifRef {}
// SAFETY: as Send
unsafe impl Sync for NetifRef {}

impl NetifRef {
    /// # Safety
    /// `p` is NULL or a live netif that is never destroyed.
    pub unsafe fn permanent(p: *mut c_void) -> Option<Self> {
        NonNull::new(p).map(Self)
    }

    fn as_ptr(self) -> *mut c_void {
        self.0.as_ptr()
    }
}

/// A netif slot, set once.
struct NetifSlot(AtomicPtr<c_void>);

impl NetifSlot {
    const fn new() -> Self {
        Self(AtomicPtr::new(core::ptr::null_mut()))
    }
    fn get(&self) -> Option<NetifRef> {
        // only NetifRefs are stored (set())
        NonNull::new(self.0.load(Ordering::Acquire)).map(NetifRef)
    }
    fn set(&self, n: NetifRef) {
        self.0.store(n.as_ptr(), Ordering::Release);
    }
}

/// An esp_timer that is never deleted.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
pub struct Timer(NonNull<c_void>);

// SAFETY: the esp_timer API may be called from any task
unsafe impl Send for Timer {}
// SAFETY: as Send
unsafe impl Sync for Timer {}

const TAG: &CStr = c"wifi";
const TAG_NET: &CStr = c"netmgr";

idf_const!(MODE_STA: u32 = wifi_mode_t_WIFI_MODE_STA, 1);
idf_const!(MODE_APSTA: u32 = wifi_mode_t_WIFI_MODE_APSTA, 3);
idf_const!(IF_STA: u32 = wifi_interface_t_WIFI_IF_STA, 0);
idf_const!(IF_AP: u32 = wifi_interface_t_WIFI_IF_AP, 1);
idf_const!(WIFI_EVENT_SCAN_DONE: i32 = wifi_event_t_WIFI_EVENT_SCAN_DONE, 1);
idf_const!(WIFI_EVENT_STA_START: i32 = wifi_event_t_WIFI_EVENT_STA_START, 2);
idf_const!(WIFI_EVENT_STA_DISCONNECTED: i32 = wifi_event_t_WIFI_EVENT_STA_DISCONNECTED, 5);
idf_const!(IP_EVENT_STA_GOT_IP: i32 = ip_event_t_IP_EVENT_STA_GOT_IP, 0);
idf_const!(ESP_EVENT_ANY_ID: i32 = ESP_EVENT_ANY_ID, -1);
idf_const!(ESP_ERR_INVALID_STATE: EspErr = ESP_ERR_INVALID_STATE, 0x103);
idf_const!(ESP_ERR_TIMEOUT: EspErr = ESP_ERR_TIMEOUT, 0x107);
idf_const!(ESP_ERR_WIFI_CONN: EspErr = ESP_ERR_WIFI_CONN, 0x3007);
idf_const!(ESP_ERR_WIFI_STOP_STATE: EspErr = ESP_ERR_WIFI_STOP_STATE, 0x3014);

/// `WIFI_SCAN_MAX_APS`
pub const SCAN_MAX_APS: usize = 20;

type EventHandler = unsafe extern "C" fn(*mut c_void, *const c_char, i32, *mut c_void);

/// The IDF calls of this module. In the firmware on the generated bindings
/// (`WIFI_INIT_CONFIG_DEFAULT()` from the header's constants, the configs
/// written by field); on the host, stubs.
#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    use crate::idf::raw;

    pub fn wifi_event() -> *const c_char {
        // SAFETY: WIFI_EVENT is an immutable `const char *` set at link time by esp_wifi
        unsafe { raw::WIFI_EVENT }
    }
    pub fn ip_event() -> *const c_char {
        // SAFETY: IP_EVENT is an immutable `const char *` set at link time by esp_netif
        unsafe { raw::IP_EVENT }
    }
    pub fn netif_init() -> EspErr {
        // SAFETY: no arguments; esp_netif_init only sets up the TCP/IP stack once
        unsafe { raw::esp_netif_init() }
    }
    pub fn event_loop_create_default() -> EspErr {
        // SAFETY: no arguments; returns ESP_ERR_INVALID_STATE if the default loop exists
        unsafe { raw::esp_event_loop_create_default() }
    }
    pub fn register_handler(base: *const c_char, id: i32, handler: EventHandler) -> EspErr {
        let mut instance = core::ptr::null_mut();
        // SAFETY: `base` is a static event base, `handler` a valid extern "C" fn and
        // `instance` a live local the call writes the instance handle to
        unsafe {
            raw::esp_event_handler_instance_register(
                base,
                id,
                Some(handler),
                core::ptr::null_mut(),
                &mut instance,
            )
        }
    }
    /// A one-shot timer in the timer task (skipping missed events).
    pub fn timer_create(
        cb: unsafe extern "C" fn(*mut c_void),
        name: &'static CStr,
    ) -> Result<Timer, EspErr> {
        let args = raw::esp_timer_create_args_t {
            callback: Some(cb),
            arg: core::ptr::null_mut(),
            dispatch_method: raw::esp_timer_dispatch_t_ESP_TIMER_TASK,
            name: name.as_ptr(),
            skip_unhandled_events: true,
        };
        let mut t = core::ptr::null_mut();
        // SAFETY: `args` (with the static `name`) and `t` are live locals; esp_timer copies
        // the args, `cb` is a valid extern "C" fn and the arg pointer is unused
        match unsafe { raw::esp_timer_create(&args, &mut t) } {
            ESP_OK => NonNull::new(t.cast())
                .map(Timer)
                .ok_or(crate::idf::ESP_FAIL),
            e => Err(e),
        }
    }
    pub fn timer_start_once(t: Timer, us: u64) -> EspErr {
        // SAFETY: a timer from esp_timer_create, never deleted (Timer)
        unsafe { raw::esp_timer_start_once(t.0.as_ptr().cast(), us) }
    }
    /// `esp_wifi_init(WIFI_INIT_CONFIG_DEFAULT())`
    pub fn wifi_init_default() -> EspErr {
        // SAFETY: only reads extern statics/takes the address of `g_wifi_osi_funcs`, as
        // WIFI_INIT_CONFIG_DEFAULT() does; nothing is dereferenced here
        let cfg = unsafe {
            raw::wifi_init_config_t {
                osi_funcs: core::ptr::addr_of_mut!(raw::g_wifi_osi_funcs),
                wpa_crypto_funcs: raw::g_wifi_default_wpa_crypto_funcs,
                static_rx_buf_num: raw::CONFIG_ESP_WIFI_STATIC_RX_BUFFER_NUM as c_int,
                dynamic_rx_buf_num: raw::CONFIG_ESP_WIFI_DYNAMIC_RX_BUFFER_NUM as c_int,
                tx_buf_type: raw::CONFIG_ESP_WIFI_TX_BUFFER_TYPE as c_int,
                static_tx_buf_num: raw::WIFI_STATIC_TX_BUFFER_NUM as c_int,
                dynamic_tx_buf_num: raw::WIFI_DYNAMIC_TX_BUFFER_NUM as c_int,
                rx_mgmt_buf_type: raw::CONFIG_ESP_WIFI_DYNAMIC_RX_MGMT_BUF as c_int,
                rx_mgmt_buf_num: raw::WIFI_RX_MGMT_BUF_NUM_DEF as c_int,
                cache_tx_buf_num: raw::WIFI_CACHE_TX_BUFFER_NUM as c_int,
                csi_enable: raw::WIFI_CSI_ENABLED as c_int,
                ampdu_rx_enable: raw::WIFI_AMPDU_RX_ENABLED as c_int,
                ampdu_tx_enable: raw::WIFI_AMPDU_TX_ENABLED as c_int,
                amsdu_tx_enable: raw::WIFI_AMSDU_TX_ENABLED as c_int,
                nvs_enable: raw::WIFI_NVS_ENABLED as c_int,
                nano_enable: raw::WIFI_NANO_FORMAT_ENABLED as c_int,
                rx_ba_win: raw::WIFI_DEFAULT_RX_BA_WIN as c_int,
                wifi_task_core_id: raw::WIFI_TASK_CORE_ID as c_int,
                beacon_max_len: raw::WIFI_SOFTAP_BEACON_MAX_LEN as c_int,
                mgmt_sbuf_num: raw::WIFI_MGMT_SBUF_NUM as c_int,
                feature_caps: raw::WIFI_FEATURE_CAPS as u64,
                sta_disconnected_pm: raw::WIFI_STA_DISCONNECTED_PM_ENABLED != 0,
                espnow_max_encrypt_num: raw::CONFIG_ESP_WIFI_ESPNOW_MAX_ENCRYPT_NUM as c_int,
                tx_hetb_queue_num: raw::WIFI_TX_HETB_QUEUE_NUM as c_int,
                dump_hesigb_enable: raw::WIFI_DUMP_HESIGB_ENABLED != 0,
                magic: raw::WIFI_INIT_CONFIG_MAGIC as c_int,
            }
        };
        // SAFETY: `cfg` is a fully initialised local; esp_wifi_init only reads it
        unsafe { raw::esp_wifi_init(&cfg) }
    }
    pub fn set_mode(mode: u32) -> EspErr {
        // SAFETY: plain value argument
        unsafe { raw::esp_wifi_set_mode(mode) }
    }
    /// The access point: open, channel 1, up to 30 stations.
    pub fn set_ap_config(ssid: &[u8]) -> EspErr {
        let mut c = raw::wifi_config_t::default();
        // SAFETY: the `ap` union field is the one written and read by IF_AP;
        // `c` is a live local that esp_wifi_set_config copies
        unsafe {
            let ap = &mut c.ap;
            let n = ssid.len().min(ap.ssid.len());
            ap.ssid[..n].copy_from_slice(&ssid[..n]);
            ap.ssid_len = ssid.len() as u8;
            ap.channel = 1;
            ap.max_connection = 30;
            ap.authmode = raw::wifi_auth_mode_t_WIFI_AUTH_OPEN;
            raw::esp_wifi_set_config(IF_AP, &mut c)
        }
    }
    /// The station: all channels, the strongest AP, PMF capable, 802.11k/v,
    /// open networks only without a password.
    pub fn set_sta_config(ssid: &[u8], pass: &[u8]) -> EspErr {
        let mut c = raw::wifi_config_t::default();
        // SAFETY: the `sta` union field is the one written and read by IF_STA; ssid and
        // password keep their last byte 0; `c` is a live local that is copied
        unsafe {
            let sta = &mut c.sta;
            sta.scan_method = raw::wifi_scan_method_t_WIFI_ALL_CHANNEL_SCAN;
            sta.sort_method = raw::wifi_sort_method_t_WIFI_CONNECT_AP_BY_SIGNAL;
            sta.pmf_cfg.capable = true;
            sta.pmf_cfg.required = false;
            sta.set_rm_enabled(1);
            sta.set_btm_enabled(1);
            // NUL-terminated within the fields
            let n = ssid.len().min(sta.ssid.len() - 1);
            sta.ssid[..n].copy_from_slice(&ssid[..n]);
            let n = pass.len().min(sta.password.len() - 1);
            sta.password[..n].copy_from_slice(&pass[..n]);
            sta.threshold.authmode = if pass.is_empty() {
                raw::wifi_auth_mode_t_WIFI_AUTH_OPEN
            } else {
                raw::wifi_auth_mode_t_WIFI_AUTH_WPA2_PSK
            };
            raw::esp_wifi_set_config(IF_STA, &mut c)
        }
    }
    pub fn wifi_start() -> EspErr {
        // SAFETY: no arguments; called after esp_wifi_init
        unsafe { raw::esp_wifi_start() }
    }
    pub fn wifi_stop() -> EspErr {
        // SAFETY: no arguments; fails with an error code if WiFi is not initialised
        unsafe { raw::esp_wifi_stop() }
    }
    pub fn wifi_connect() -> EspErr {
        // SAFETY: no arguments; fails with an error code if WiFi is not started
        unsafe { raw::esp_wifi_connect() }
    }
    pub fn wifi_disconnect() -> EspErr {
        // SAFETY: no arguments; fails with an error code if WiFi is not started
        unsafe { raw::esp_wifi_disconnect() }
    }
    pub fn set_ps_none() -> EspErr {
        // SAFETY: plain value argument
        unsafe { raw::esp_wifi_set_ps(raw::wifi_ps_type_t_WIFI_PS_NONE) }
    }
    pub fn mac(iface: u32) -> (EspErr, [u8; 6]) {
        let mut mac = [0u8; 6];
        // SAFETY: `mac` is a 6-byte local, the size esp_wifi_get_mac writes
        let e = unsafe { raw::esp_wifi_get_mac(iface, mac.as_mut_ptr()) };
        (e, mac)
    }
    /// All channels, no hidden networks (`wifi_scan_config_t` zeroed).
    pub fn scan_start() -> EspErr {
        let cfg = raw::wifi_scan_config_t::default();
        // SAFETY: `cfg` is a zeroed local that esp_wifi_scan_start only reads (blocking: false)
        unsafe { raw::esp_wifi_scan_start(&cfg, false) }
    }
    /// The records of the finished scan (up to `SCAN_MAX_APS`).
    pub fn scan_records() -> Option<Vec<ScanResult>> {
        let mut records = vec![raw::wifi_ap_record_t::default(); SCAN_MAX_APS];
        let mut n = SCAN_MAX_APS as u16;
        // SAFETY: `records` holds `n` = SCAN_MAX_APS entries; the call writes at most `n`
        // records and stores the count it wrote in `n`
        if unsafe { raw::esp_wifi_scan_get_ap_records(&mut n, records.as_mut_ptr()) } != ESP_OK {
            return None;
        }
        records.truncate(n as usize);
        Some(
            records
                .iter()
                .map(|r| ScanResult {
                    ssid: CString::new(crate::cutil::until_nul(&r.ssid)).unwrap_or_default(),
                    rssi: r.rssi,
                    authmode: r.authmode as c_int,
                })
                .collect(),
        )
    }
    /// The access point's RSSI (dBm) while connected.
    pub fn sta_rssi() -> Option<i8> {
        let mut r = raw::wifi_ap_record_t::default();
        // SAFETY: `r` is a live local the call fills
        (unsafe { raw::esp_wifi_sta_get_ap_info(&mut r) } == ESP_OK).then_some(r.rssi)
    }
    pub fn netif_create_ap() -> Option<NetifRef> {
        // SAFETY: no arguments; requires esp_netif_init/esp_wifi_init (done in wifi_init);
        // the default netifs are never destroyed
        unsafe { NetifRef::permanent(raw::esp_netif_create_default_wifi_ap().cast()) }
    }
    pub fn netif_create_sta() -> Option<NetifRef> {
        // SAFETY: as netif_create_ap
        unsafe { NetifRef::permanent(raw::esp_netif_create_default_wifi_sta().cast()) }
    }
    pub fn set_hostname(netif: NetifRef, name: &CStr) -> EspErr {
        // SAFETY: a live netif (NetifRef) and a NUL-terminated string esp_netif copies
        unsafe { raw::esp_netif_set_hostname(netif.as_ptr().cast(), name.as_ptr()) }
    }
    pub fn set_default_netif(netif: NetifRef) -> EspErr {
        // SAFETY: a live netif (NetifRef)
        unsafe { raw::esp_netif_set_default_netif(netif.as_ptr().cast()) }
    }
    /// Makes the DNS servers of `netif` (from its DHCP lease) the global
    /// resolver's: with WiFi and Ethernet registered, lwIP does not always
    /// take the Ethernet lease's servers when WiFi never had one, and pool
    /// host lookups fail. `public_fallback`: 8.8.8.8 / 1.1.1.1 for a slot
    /// the lease left empty (Ethernet).
    pub fn apply_dns(netif: NetifRef, public_fallback: bool) {
        let netif = netif.as_ptr();
        let slots = [
            (raw::esp_netif_dns_type_t_ESP_NETIF_DNS_MAIN, [8u8, 8, 8, 8]),
            (raw::esp_netif_dns_type_t_ESP_NETIF_DNS_BACKUP, [1, 1, 1, 1]),
        ];
        for (ty, fallback) in slots {
            let mut info = raw::esp_netif_dns_info_t::default();
            // SAFETY: an IPv4 address is read only when the type says so
            let current = unsafe {
                raw::esp_netif_get_dns_info(netif.cast(), ty, &mut info) == ESP_OK
                    && info.ip.type_ as u32 == raw::ESP_IPADDR_TYPE_V4
                    && info.ip.u_addr.ip4.addr != 0
            };
            if current {
                // SAFETY: the condition above checked that `info.ip` is of type IPv4
                let a = unsafe { info.ip.u_addr.ip4.addr }.to_ne_bytes();
                log!(
                    Info,
                    TAG_NET,
                    "DNS[{ty}] from the active interface: {}.{}.{}.{}",
                    a[0],
                    a[1],
                    a[2],
                    a[3]
                );
            } else if public_fallback {
                info = raw::esp_netif_dns_info_t::default();
                info.ip.type_ = raw::ESP_IPADDR_TYPE_V4 as u8;
                info.ip.u_addr.ip4.addr = u32::from_ne_bytes(fallback);
                let a = fallback;
                log!(
                    Warn,
                    TAG_NET,
                    "ETH lease without DNS[{ty}], using {}.{}.{}.{}",
                    a[0],
                    a[1],
                    a[2],
                    a[3]
                );
            } else {
                continue;
            }
            // without CONFIG_ESP_NETIF_SET_DNS_PER_DEFAULT_NETIF this sets
            // the global lwIP DNS server of the slot
            // SAFETY: `netif` is the live, non-null preferred netif and `info` a local that is only read
            let e = unsafe { raw::esp_netif_set_dns_info(netif.cast(), ty, &mut info) };
            if e != ESP_OK {
                log!(
                    Warn,
                    TAG_NET,
                    "esp_netif_set_dns_info failed: {}",
                    err_name(e)
                );
            }
        }
    }
    /// The address of an `IP_EVENT_STA_GOT_IP`.
    ///
    /// # Safety
    /// `data` the event's `ip_event_got_ip_t`.
    pub unsafe fn got_ip(data: *mut c_void) -> [u8; 4] {
        (*(data as *const raw::ip_event_got_ip_t))
            .ip_info
            .ip
            .addr
            .to_ne_bytes()
    }
}

#[cfg(not(target_os = "espidf"))]
#[allow(clippy::missing_safety_doc)]
mod sys {
    use super::*;
    use crate::idf::ESP_FAIL;

    pub fn wifi_event() -> *const c_char {
        c"WIFI_EVENT".as_ptr()
    }
    pub fn ip_event() -> *const c_char {
        c"IP_EVENT".as_ptr()
    }
    pub fn netif_init() -> EspErr {
        ESP_OK
    }
    pub fn event_loop_create_default() -> EspErr {
        ESP_OK
    }
    pub fn register_handler(_: *const c_char, _: i32, _: EventHandler) -> EspErr {
        ESP_OK
    }
    pub fn timer_create(
        _: unsafe extern "C" fn(*mut c_void),
        _: &'static CStr,
    ) -> Result<Timer, EspErr> {
        Ok(Timer(NonNull::dangling()))
    }
    pub fn timer_start_once(_: Timer, _: u64) -> EspErr {
        ESP_OK
    }
    pub fn wifi_init_default() -> EspErr {
        ESP_OK
    }
    pub fn set_mode(_: u32) -> EspErr {
        ESP_OK
    }
    pub fn set_ap_config(_: &[u8]) -> EspErr {
        ESP_OK
    }
    pub fn set_sta_config(_: &[u8], _: &[u8]) -> EspErr {
        ESP_OK
    }
    pub fn wifi_start() -> EspErr {
        ESP_OK
    }
    pub fn wifi_stop() -> EspErr {
        ESP_OK
    }
    pub fn wifi_connect() -> EspErr {
        ESP_OK
    }
    pub fn wifi_disconnect() -> EspErr {
        ESP_OK
    }
    pub fn set_ps_none() -> EspErr {
        ESP_OK
    }
    pub fn mac(_: u32) -> (EspErr, [u8; 6]) {
        (ESP_OK, [0; 6])
    }
    pub fn scan_start() -> EspErr {
        ESP_FAIL
    }
    pub fn scan_records() -> Option<Vec<ScanResult>> {
        None
    }
    pub fn sta_rssi() -> Option<i8> {
        None
    }
    pub fn netif_create_ap() -> Option<NetifRef> {
        None
    }
    pub fn netif_create_sta() -> Option<NetifRef> {
        None
    }
    pub fn set_hostname(_: NetifRef, _: &CStr) -> EspErr {
        ESP_OK
    }
    pub fn set_default_netif(_: NetifRef) -> EspErr {
        ESP_OK
    }
    pub fn apply_dns(_: NetifRef, _: bool) {}
    pub unsafe fn got_ip(_: *mut c_void) -> [u8; 4] {
        [0; 4]
    }
}

/// `ESP_ERROR_CHECK()`
fn check(e: EspErr, what: &str) {
    if e != ESP_OK {
        log!(Error, TAG, "{what} failed: {}", err_name(e));
        panic!("ESP_ERROR_CHECK failed: {what}: {}", err_name(e));
    }
}

fn delay_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

// ---------------------------------------------------------------------------
// WiFi (connect.cpp)

static WIFI_INITED: AtomicBool = AtomicBool::new(false);
static NETIF_AP: NetifSlot = NetifSlot::new();
static NETIF_STA: NetifSlot = NetifSlot::new();
/// the reconnect backoff (a timer: a delay in the handler blocked the
/// default event loop)
static BACKOFF_TIMER: OnceLock<Timer> = OnceLock::new();
/// the station MAC as text
static MAC: OnceLock<CString> = OnceLock::new();
/// the last scan's records
static SCAN: Mutex<Vec<ScanResult>> = Mutex::new(Vec::new());

fn set_wifi_status(text: &str) {
    crate::system::set_wifi_status(text.as_bytes());
}

/// Carries out the actions of a `wifi.rs` call, in bit order.
fn run_actions(a: u32) {
    // the connected/fail bits had no waiters left
    if a & act::HOOK_DISCONNECTED != 0 {
        on_event(Event::WifiDisconnected);
    }
    if a & act::STATUS_FAILED != 0 {
        log!(Warn, TAG, "Could not connect to WiFi (initial).");
        set_wifi_status("Connect Failed!");
    }
    if a & act::CONNECT != 0 {
        sys::wifi_connect();
    }
    if a & act::STATUS_RETRYING != 0 {
        log!(Info, TAG, "Retrying WiFi connection...");
        set_wifi_status(&format!("Retrying: {}", wifi_state::wifi_state_retries()));
    }
    if a & act::STATUS_CONNECTED != 0 {
        set_wifi_status("Connected!");
    }
    if a & act::HOOK_GOT_IP != 0 {
        on_event(Event::WifiGotIp);
    }
    if a & act::STATUS_PAUSED != 0 {
        set_wifi_status("LAN Connected!");
    }
    if a & act::BACKOFF != 0 {
        if let Some(&t) = BACKOFF_TIMER.get() {
            sys::timer_start_once(t, 1_000_000);
        }
    }
}

unsafe extern "C" fn backoff_done(_: *mut c_void) {
    run_actions(wifi_state::wifi_state_event(wifi_state::EV_BACKOFF_DONE));
}

/// WiFi stopped on purpose (Ethernet has an IP): a disconnect then is no
/// connection loss.
pub fn pause(paused: bool) {
    run_actions(wifi_state::wifi_state_pause(paused));
}

unsafe extern "C" fn wifi_event(_: *mut c_void, base: *const c_char, id: i32, data: *mut c_void) {
    if base == sys::wifi_event() {
        match id {
            WIFI_EVENT_STA_START => {
                run_actions(wifi_state::wifi_state_event(wifi_state::EV_STA_START))
            }
            WIFI_EVENT_SCAN_DONE => {
                let records = sys::scan_records().unwrap_or_else(|| {
                    log!(Warn, TAG, "esp_wifi_scan_get_ap_records failed");
                    Vec::new()
                });
                let n = records.len();
                *SCAN.lock().unwrap_or_else(|e| e.into_inner()) = records;
                log!(Info, TAG, "WiFi scan done: {n} APs");
                run_actions(wifi_state::wifi_state_event(wifi_state::EV_SCAN_DONE));
            }
            WIFI_EVENT_STA_DISCONNECTED => {
                run_actions(wifi_state::wifi_state_event(wifi_state::EV_DISCONNECTED))
            }
            _ => {}
        }
    } else if base == sys::ip_event() && id == IP_EVENT_STA_GOT_IP && !data.is_null() {
        let ip = sys::got_ip(data);
        let [a, b, c, d] = ip;
        log!(Info, TAG, "WiFi got IP: {a}.{b}.{c}.{d}");
        run_actions(wifi_state::wifi_state_got_ip(ip.as_ptr()));
    }
}

/// The access point's SSID ("Nerdaxe_" and the last two AP MAC bytes).
pub fn ap_ssid() -> String {
    let (_, mac) = sys::mac(IF_AP);
    format!("Nerdaxe_{:02X}{:02X}", mac[4], mac[5])
}

fn init_softap() -> Option<NetifRef> {
    let netif = sys::netif_create_ap();
    check(
        sys::set_ap_config(ap_ssid().as_bytes()),
        "esp_wifi_set_config(AP)",
    );
    netif
}

pub fn softap_off() {
    log!(Info, TAG, "WiFi AP off (STA only)");
    let e = sys::set_mode(MODE_STA);
    if e != ESP_ERR_WIFI_STOP_STATE {
        check(e, "esp_wifi_set_mode(STA)");
    }
    crate::system::set_ap_state(false);
}

pub fn softap_on() {
    log!(Info, TAG, "WiFi AP on (APSTA)");
    check(sys::set_mode(MODE_APSTA), "esp_wifi_set_mode(APSTA)");
    crate::system::set_ap_state(true);
}

fn init_sta(ssid: &[u8], pass: &[u8]) -> Option<NetifRef> {
    let netif = sys::netif_create_sta();
    wifi_state::wifi_state_set_has_ssid(!ssid.is_empty());
    if pass.is_empty() {
        log!(Info, TAG, "WiFi password empty -> allow open networks");
    }
    check(sys::set_sta_config(ssid, pass), "esp_wifi_set_config(STA)");
    netif
}

/// Starts WiFi as access point and station; the station's netif.
pub fn wifi_init(ssid: &[u8], pass: &[u8], hostname: &CStr) -> Option<NetifRef> {
    if !WIFI_INITED.load(Ordering::Acquire) {
        match sys::timer_create(backoff_done, c"wifi_backoff") {
            Ok(t) => {
                let _ = BACKOFF_TIMER.set(t);
            }
            Err(e) => check(e, "esp_timer_create"),
        }
        check(
            sys::register_handler(sys::wifi_event(), ESP_EVENT_ANY_ID, wifi_event),
            "esp_event_handler_instance_register(WIFI_EVENT)",
        );
        check(
            sys::register_handler(sys::ip_event(), IP_EVENT_STA_GOT_IP, wifi_event),
            "esp_event_handler_instance_register(IP_EVENT)",
        );
        let e = sys::wifi_init_default();
        if e != ESP_ERR_INVALID_STATE {
            check(e, "esp_wifi_init");
        }
        WIFI_INITED.store(true, Ordering::Release);
    }

    softap_on();

    // the netifs only once
    if NETIF_AP.get().is_none() {
        if let Some(n) = init_softap() {
            NETIF_AP.set(n);
        }
    }
    if NETIF_STA.get().is_none() {
        if let Some(n) = init_sta(ssid, pass) {
            NETIF_STA.set(n);
        }
    }

    let e = sys::wifi_start();
    if e != ESP_ERR_WIFI_CONN {
        check(e, "esp_wifi_start");
    }
    check(sys::set_ps_none(), "esp_wifi_set_ps");

    let sta = NETIF_STA.get();
    if let Some(sta) = sta {
        let e = sys::set_hostname(sta, hostname);
        if e != ESP_OK {
            log!(Warn, TAG, "esp_netif_set_hostname failed: {}", err_name(e));
        } else {
            log!(Info, TAG, "Hostname: {}", hostname.to_string_lossy());
        }
    }

    let (e, mac) = sys::mac(IF_STA);
    check(e, "esp_wifi_get_mac");
    let text = mac
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":");
    let _ = MAC.set(CString::new(text).unwrap_or_default());
    sta
}

/// A network of the scan.
pub struct ScanResult {
    pub ssid: CString,
    pub rssi: i8,
    pub authmode: c_int,
}

/// Scans for networks (only while the access point is up: the request
/// comes in over it, the station is disconnected meanwhile).
pub fn scan() -> Result<Vec<ScanResult>, EspErr> {
    if !wifi_state::wifi_state_scan_begin() {
        log!(Warn, TAG, "WiFi scan already in progress");
        return Err(ESP_ERR_INVALID_STATE);
    }
    log!(Info, TAG, "Starting WiFi scan");
    SCAN.lock().unwrap_or_else(|e| e.into_inner()).clear();

    // a connecting station blocks the scan
    sys::wifi_disconnect();
    delay_ms(100);

    let e = sys::scan_start();
    if e != ESP_OK {
        log!(Error, TAG, "esp_wifi_scan_start failed: {}", err_name(e));
        run_actions(wifi_state::wifi_state_scan_finish());
        return Err(e);
    }
    // about 10 s
    for _ in 0..20 {
        if !wifi_state::wifi_state_scanning() {
            break;
        }
        delay_ms(500);
    }
    if wifi_state::wifi_state_scanning() {
        log!(Error, TAG, "WiFi scan timeout");
        // the station tries again
        run_actions(wifi_state::wifi_state_scan_finish());
        return Err(ESP_ERR_TIMEOUT);
    }
    let list = core::mem::take(&mut *SCAN.lock().unwrap_or_else(|e| e.into_inner()));
    run_actions(wifi_state::wifi_state_scan_finish());
    Ok(list)
}

/// The station MAC as text ("" before WiFi is up).
pub fn mac_addr() -> &'static CStr {
    MAC.get().map_or(c"", |m| m.as_c_str())
}

/// The access point's RSSI (dBm), -90 while not connected.
pub fn wifi_rssi() -> c_int {
    sys::sta_rssi().map_or(-90, |r| r as c_int)
}

// ---------------------------------------------------------------------------
// interfaces (network_manager.cpp)

#[derive(Clone, Copy)]
enum Event {
    WifiGotIp = 0,
    WifiDisconnected = 1,
    EthGotIp = 2,
    EthLinkDown = 3,
}

const NET_WIFI_IP: u8 = 1 << 0;
const NET_ETH_IP: u8 = 1 << 1;

/// The "has an IP" bits and their waiters.
static NET_BITS: Mutex<u8> = Mutex::new(0);
static NET_CHANGED: Condvar = Condvar::new();
static NET_INITED: AtomicBool = AtomicBool::new(false);

type Hook = unsafe extern "C" fn();
/// main: reconnect the pools when the preferred interface changes
static HOOK_PREFERRED: Mutex<Option<Hook>> = Mutex::new(None);

fn net_bits(set: u8, clear: u8) {
    let mut b = NET_BITS.lock().unwrap_or_else(|e| e.into_inner());
    *b = (*b | set) & !clear;
    NET_CHANGED.notify_all();
}

unsafe extern "C" fn eth_got_ip() {
    on_event(Event::EthGotIp);
}

unsafe extern "C" fn eth_link_down() {
    on_event(Event::EthLinkDown);
}

/// netif and the default event loop (both may exist already) and the
/// Ethernet hooks.
fn net_init() -> EspErr {
    if NET_INITED.load(Ordering::Acquire) {
        return ESP_OK;
    }
    for e in [sys::netif_init(), sys::event_loop_create_default()] {
        if e != ESP_OK && e != ESP_ERR_INVALID_STATE {
            return e;
        }
    }
    crate::w5500::ffi::w5500_set_hooks(Some(eth_got_ip), Some(eth_link_down));
    NET_INITED.store(true, Ordering::Release);
    ESP_OK
}

/// WiFi (access point and station) and, on boards with one, Ethernet.
pub fn start(ssid: &[u8], pass: &[u8], hostname: &CStr, has_eth: bool) -> EspErr {
    let e = net_init();
    if e != ESP_OK {
        return e;
    }
    wifi_init(ssid, pass, hostname);
    if has_eth {
        let e = crate::w5500::start();
        if e != ESP_OK {
            log!(Warn, TAG_NET, "ETH init failed: {}", err_name(e));
        }
    }
    ESP_OK
}

/// The "has an IP" bits (1 WiFi, 2 Ethernet) once one is set, 0 after
/// `timeout`.
pub fn wait_any_ip(timeout: Duration) -> u8 {
    let b = NET_BITS.lock().unwrap_or_else(|e| e.into_inner());
    let (b, _) = NET_CHANGED
        .wait_timeout_while(b, timeout, |b| *b == 0)
        .unwrap_or_else(|e| e.into_inner());
    *b
}

pub fn shutdown_ap_once() {
    if netmgr::netmgr_ap_off_once() {
        log!(Info, TAG_NET, "First IP acquired -> AP permanently off");
        softap_off();
    }
}

/// An interface event: the decisions in `netmgr.rs`, the actions here.
fn on_event(ev: Event) {
    let eth = crate::w5500::netif();
    let sta = NETIF_STA.get();
    let mut preferred = 0;
    // SAFETY: `preferred` is a live local the call writes to
    let a =
        unsafe { netmgr::netmgr_event(ev as c_int, eth.is_some(), sta.is_some(), &mut preferred) };

    let set = if a & nact::SET_WIFI_BIT != 0 {
        NET_WIFI_IP
    } else {
        0
    } | if a & nact::SET_ETH_BIT != 0 {
        NET_ETH_IP
    } else {
        0
    };
    let clear = if a & nact::CLEAR_WIFI_BIT != 0 {
        NET_WIFI_IP
    } else {
        0
    } | if a & nact::CLEAR_ETH_BIT != 0 {
        NET_ETH_IP
    } else {
        0
    };
    if set | clear != 0 {
        net_bits(set, clear);
    }
    if a & nact::AP_OFF != 0 {
        log!(Info, TAG_NET, "First IP acquired -> AP permanently off");
        softap_off();
    }
    if a & nact::STOP_WIFI != 0 {
        log!(Info, TAG_NET, "ETH has IP -> stopping WiFi");
        pause(true);
        sys::wifi_disconnect();
        sys::wifi_stop();
    }
    if a & nact::START_WIFI != 0 {
        log!(Info, TAG_NET, "ETH link down -> restarting WiFi");
        pause(false);
        sys::wifi_start();
    }

    // NETMGR_ETH, NETMGR_WIFI
    let netif = match preferred {
        2 => eth,
        1 => sta,
        _ => None,
    };
    if let (true, Some(netif)) = (a & nact::SET_DEFAULT != 0, netif) {
        let e = sys::set_default_netif(netif);
        if e != ESP_OK {
            log!(
                Warn,
                TAG_NET,
                "esp_netif_set_default_netif failed: {}",
                err_name(e)
            );
        }
        // NETMGR_ETH: the public servers as the fallback
        sys::apply_dns(netif, preferred == 2);
    }
    // HOOK_ANY_IP: no one registers for it
    if a & nact::HOOK_PREFERRED != 0 {
        let hook = *HOOK_PREFERRED.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(h) = hook {
            // SAFETY: `h` was registered through network_set_hook_preferred_changed, whose
            // caller passes a valid function taking no arguments
            unsafe { h() };
        }
    }
}

/// The preferred interface's address (Ethernet first) and whether it is
/// Ethernet; `None` without one.
pub fn preferred_ip() -> Option<(String, bool)> {
    if netmgr::netmgr_eth_has_ip() && crate::w5500::has_ip() {
        return Some((crate::w5500::ip(), true));
    }
    if netmgr::netmgr_wifi_has_ip() {
        let mut buf = [0 as c_char; 20];
        // SAFETY: `buf` is a live local of `buf.len()` bytes; the call writes at most that
        // many including the NUL
        let ok = unsafe { wifi_state::wifi_state_ip(buf.as_mut_ptr(), buf.len()) };
        // SAFETY: wifi_state_ip always NUL-terminates `buf` (len > 0) and it was zeroed
        let ip = unsafe { CStr::from_ptr(buf.as_ptr()) }
            .to_string_lossy()
            .into_owned();
        if ok && !ip.is_empty() {
            return Some((ip, false));
        }
    }
    None
}

pub(crate) mod ffi {
    use super::*;

    pub extern "C" fn network_set_hook_preferred_changed(hook: Option<Hook>) {
        *HOOK_PREFERRED.lock().unwrap_or_else(|e| e.into_inner()) = hook;
    }

    /// The Ethernet setup before the board init; a failure is logged.
    pub extern "C" fn network_early_eth_init() {
        let e = crate::w5500::early_init();
        if e != ESP_OK {
            log!(
                Error,
                c"network",
                "ETH setup failed ({}), continuing without Ethernet",
                err_name(e)
            );
        }
    }

    #[no_mangle]
    pub extern "C" fn network_wifi_rssi() -> c_int {
        wifi_rssi()
    }

    /// The station MAC ("" before WiFi is up).
    #[no_mangle]
    pub extern "C" fn network_mac_addr() -> *const c_char {
        mac_addr().as_ptr()
    }
}
