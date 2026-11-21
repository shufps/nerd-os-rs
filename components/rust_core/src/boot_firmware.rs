//! The boot steps of the firmware (port of the hooks of `main/main.cpp`):
//! the IDF setup, the board, the tasks, the network setup, all reached
//! directly, with the display and its self test (formerly C++ objects
//! created in `main/main.cpp`).

use crate::idf::idf_const;
use core::ffi::{c_int, c_void, CStr};
use std::time::Duration;

use crate::boot::{Hooks, PoolMode};
use crate::idf::nvs::err_name;
use crate::idf::task::{create_internal, TaskFn};
use crate::idf::{EspErr, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"nerd*axe";

#[cfg(target_os = "espidf")]
const _: () = assert!(
    crate::sdkconfig::SPIRAM,
    "firmware will not work without psram"
);

/// the task watchdog: an hour (the stratum task feeds it)
const WATCHDOG_TIMEOUT_MS: u32 = 3600 * 1000;
idf_const!(RST_TASK_WDT: c_int = esp_reset_reason_t_ESP_RST_TASK_WDT, 6);
idf_const!(ESP_ERR_INVALID_STATE: EspErr = ESP_ERR_INVALID_STATE, 0x103);
idf_const!(CAP_PSRAM: u32 = MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT, (1 << 10) | (1 << 2));
idf_const!(CAP_DEFAULT: u32 = MALLOC_CAP_DEFAULT, 1 << 12);
idf_const!(CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_task_wdt_config_t as WdtConfig;
#[cfg(not(target_os = "espidf"))]
/// `esp_task_wdt_config_t`
pub struct WdtConfig {
    pub timeout_ms: u32,
    pub idle_core_mask: u32,
    pub trigger_panic: bool,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        esp_event_loop_create_default, esp_netif_init, esp_psram_get_size, esp_task_wdt_init,
        gpio_install_isr_service, heap_caps_calloc, heap_caps_free,
        mbedtls_platform_set_calloc_free, nvs_flash_init,
    };
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn esp_task_wdt_init(_: *const WdtConfig) -> EspErr {
        ESP_OK
    }
    pub unsafe fn mbedtls_platform_set_calloc_free(
        _: Option<unsafe extern "C" fn(usize, usize) -> *mut c_void>,
        _: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_int {
        0
    }
    pub unsafe fn heap_caps_calloc(_: usize, _: usize, _: u32) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn heap_caps_free(_: *mut c_void) {}
    pub unsafe fn nvs_flash_init() -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_psram_get_size() -> usize {
        0
    }
    pub unsafe fn gpio_install_isr_service(_: c_int) -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_netif_init() -> EspErr {
        ESP_OK
    }
    pub unsafe fn esp_event_loop_create_default() -> EspErr {
        ESP_OK
    }
}

/// `ESP_ERROR_CHECK()`
fn check(e: EspErr, what: &str) {
    if e != ESP_OK {
        panic!("ESP_ERROR_CHECK failed: {what}: {}", err_name(e));
    }
}

/// mbedTLS allocations in PSRAM (TLS costs a lot of internal RAM), internal
/// RAM as the fallback.
unsafe extern "C" fn psram_calloc(n: usize, size: usize) -> *mut c_void {
    let p = if n == 0 || size == 0 {
        core::ptr::null_mut()
    } else {
        sys::heap_caps_calloc(n, size, CAP_PSRAM)
    };
    if !p.is_null() {
        return p;
    }
    log!(
        Error,
        TAG,
        "PSRAM allocation failed! Falling back to internal RAM."
    );
    sys::heap_caps_calloc(n, size, CAP_DEFAULT)
}

unsafe extern "C" fn psram_free(p: *mut c_void) {
    sys::heap_caps_free(p)
}

/// The board the firmware is built for (`BOARD`, build.rs).
fn board_id() -> Option<crate::board_profile::BoardId> {
    use crate::board_profile::BoardId::*;
    Some(match crate::sdkconfig::BOARD {
        "NERDAXE" => NerdAxe,
        "NERDAXEGAMMA" => NerdAxeGamma,
        "NERDAXEGAIA" => NerdAxeGaia,
        "NERDQAXEPLUS" => NerdQAxePlus,
        "NERDOCTAXEPLUS" => NerdOctaxePlus,
        "NERDQAXEPLUS2" => NerdQAxePlus2,
        "NERDQX" => NerdQX,
        "NERDHAXEGAMMA" => NerdHaxeGamma,
        "NERDEKO" => NerdEko,
        "NERDOCTAXEGAMMA" => NerdOctaxeGamma,
        "Q1370" => Q1370,
        "Q1373" => Q1373,
        _ => return None,
    })
}

/// A profile string of the board info.
fn info_text(p: &'static CStr) -> std::borrow::Cow<'static, str> {
    p.to_string_lossy()
}

fn board_info() -> crate::board::api::Info {
    crate::board::api::info()
}

fn task(code: TaskFn, name: &'static CStr, stack: u32, prio: u32) {
    crate::idf::task::spawn_psram(code, name, stack, 0, prio);
}

/// Network preferred route changed (ETH <-> WiFi): the pools reconnect.
fn on_preferred_changed() {
    log!(
        Warn,
        TAG,
        "Network preferred route changed -> request stratum reconnect"
    );
    crate::pool_manager::api::pool_mgr_reconnect_all();
}

/// The firmware's boot steps.
#[derive(Default)]
pub struct Firmware {
    eth_started: bool,
}

fn cfg_str(key: &[u8], def: &str) -> Vec<u8> {
    crate::config::get_str(key)
        .map(|v| crate::cutil::until_nul(&v).to_vec())
        .unwrap_or_else(|| def.as_bytes().to_vec())
}

impl Hooks for Firmware {
    fn early_init(&mut self) -> bool {
        let wdt = WdtConfig {
            timeout_ms: WATCHDOG_TIMEOUT_MS,
            idle_core_mask: 0,
            trigger_panic: true,
        };
        // SAFETY: wdt is a valid config on the stack; esp_task_wdt_init only reads it during the
        // call
        let e = unsafe { sys::esp_task_wdt_init(&wdt) };
        if e != ESP_OK {
            log!(Error, TAG, "Failed to initialize watchdog: {e}");
        }
        // SAFETY: both are plain functions with the calloc/free signatures mbedTLS expects, set
        // once before any TLS use
        unsafe { sys::mbedtls_platform_set_calloc_free(Some(psram_calloc), Some(psram_free)) };

        log!(Info, TAG, "Welcome to the Nerd*Axe - hack the planet!");
        // SAFETY: called once at boot from the main task, before any NVS access
        check(unsafe { sys::nvs_flash_init() }, "nvs_flash_init");
        // shows and saves the last reset reason
        let reason = crate::system::read_reset_reason();
        // loads (or migrates) the config; the NerdAxe and NerdAxe-γ have one fan
        let single_fan = matches!(crate::sdkconfig::BOARD, "NERDAXE" | "NERDAXEGAMMA");
        crate::config_nvs::init(single_fan, crate::sdkconfig::OVERHEAT_TEMP as u16);

        // SAFETY: plain ESP-IDF call with no pointer arguments; an already installed service is
        // tolerated below
        let e = unsafe { sys::gpio_install_isr_service(0) };
        if e != ESP_OK && e != ESP_ERR_INVALID_STATE {
            log!(
                Error,
                TAG,
                "gpio_install_isr_service failed: {}",
                err_name(e)
            );
        }
        // SAFETY: called once at boot from the main task
        check(unsafe { sys::esp_netif_init() }, "esp_netif_init");
        // SAFETY: called once at boot from the main task; an existing default loop is tolerated
        // below
        let e = unsafe { sys::esp_event_loop_create_default() };
        if e != ESP_ERR_INVALID_STATE {
            check(e, "esp_event_loop_create_default");
        }
        reason == RST_TASK_WDT
    }

    fn board_create(&mut self) {
        let id = board_id();
        if !id.is_some_and(|id| crate::board::api::board_create(id as c_int)) {
            log!(Error, c"board", "no board {}", crate::sdkconfig::BOARD);
            std::process::abort();
        }
    }

    /// The W5500 if the board has Ethernet; before and after the board init,
    /// set up (or attempted) once.
    fn eth_init(&mut self) {
        if self.eth_started || !crate::board::api::board_has_ethernet() {
            return;
        }
        self.eth_started = true;
        // a HAT on the board's pins (no reset line, no interrupt), else the
        // on-board W5500's defaults
        if let Some(p) = crate::board::get().and_then(|b| b.eth_pins()) {
            crate::w5500::set_pins(
                p.sclk,
                p.mosi,
                p.miso,
                p.cs,
                crate::w5500::GPIO_NC,
                crate::w5500::GPIO_NC,
            );
        }
        crate::network::api::network_early_eth_init();
    }

    fn board_load_settings(&mut self) {
        crate::board::api::board_load_settings()
    }

    fn board_init(&mut self) {
        crate::board::api::board_init();
    }

    fn board_detect_eth(&mut self) {
        crate::board::api::board_detect_ethernet()
    }

    fn system_set_board(&mut self) {
        // SAFETY: a plain query
        let psram = unsafe { sys::esp_psram_get_size() };
        log!(Info, TAG, "PSRAM found with {}MB", psram / (1024 * 1024));
        let i = board_info();
        log!(
            Info,
            TAG,
            "Found Device Model: {}",
            info_text(i.device_model)
        );
        log!(Info, TAG, "Found Board Version: {}", i.version);
    }

    fn self_test_enabled(&mut self) -> bool {
        crate::config::get_u16(b"selftest", 0) != 0
    }

    fn best_diff(&mut self) -> u64 {
        crate::config::get_u64(b"bestdiff", 0)
    }

    /// Powers the ASICs and shows on the display whether all were found
    /// (checks the soldering, not the hashrate); formerly `Board::selfTest()`.
    fn self_test(&mut self) {
        use crate::board::api::{board_init_asics, board_is_axe, board_power_value};
        // the NerdAxe family's core voltage is lower
        let (vmin, vmax) = if board_is_axe() {
            (1.0, 1.35)
        } else {
            (1.1, 1.4)
        };
        crate::display::init();
        let model = info_text(board_info().asic_model);
        crate::display::log(&format!(
            "\nSelfTest initiated, wait...\r\nTesting chip: {model}\n\n\n\n\n\
             [Warning] Only checks soldering\nHashrate not checked"
        ));

        board_init_asics();
        let power = board_power_value(2);
        let vout = board_power_value(3);
        let i = board_info();
        let power_ok = power > i.min_pin && power < i.max_pin;
        let vr_ok = vout > vmin && vout < vmax;
        let all = i.chips_detected == i.asic_count;
        let ok = |b: bool| if b { "OK" } else { "Warning" };
        crate::display::log(&format!(
            "\nTest result [{model}]:\r\n- Asics detected [{}/{}]\n\
             - Power status: {} ({power:.2} W)\n- Asic voltage: {} ({vout:.2} V)\r\n\n{}",
            i.chips_detected,
            i.asic_count,
            ok(power_ok),
            ok(vr_ok),
            if all {
                "OOOOOOOO TEST OK!!! OOOOOOO"
            } else {
                "XXXXXXXXX TEST KO XXXXXXXXX"
            }
        ));
        if all {
            crate::config::set(
                b"selftest",
                crate::ajson::Json::Num(crate::ajson::Num::U(0)),
            );
            crate::config_nvs::request_flush();
        }
    }

    fn delay_ms(&mut self, ms: u32) {
        std::thread::sleep(Duration::from_millis(ms as u64));
    }

    fn is_can_slave(&mut self) -> bool {
        crate::board::api::board_is_can_slave()
    }

    fn log_can_slave(&mut self) {
        log!(Info, TAG, "CAN Slave mode");
    }

    fn system_init(&mut self) {
        crate::system::init(board_info().asic_count);
        crate::display::init();
    }

    fn start_system_tasks(&mut self) {
        task(crate::system::api::system_task, c"SYSTEM_task", 4096, 3);
        task(
            crate::power_mgmt::api::pm_task,
            c"power mangement",
            8192,
            10,
        );
        crate::system::set_startup_done();
    }

    fn can_init(&mut self) {
        crate::can::init(
            crate::board::api::board_can_tx_pin(),
            crate::board::api::board_can_rx_pin(),
        );
    }

    fn init_asics(&mut self) {
        crate::power_mgmt::lock();
        if !crate::board::api::board_init_asics() {
            let model = info_text(board_info().device_model);
            log!(Error, TAG, "error initializing board {model}");
        }
        crate::power_mgmt::unlock();
    }

    fn start_can_slave_tasks(&mut self) {
        use crate::can::api as can;
        task(can::can_slave_task, c"can slave", 4096, 10);
        task(can::can_slave_result_task, c"can result", 4096, 10);
        task(can::can_slave_telemetry_task, c"can telem", 4096, 5);
    }

    fn has_hashrate_counter(&mut self) -> bool {
        board_info().has_hash_counter
    }

    fn start_hashrate_monitor(&mut self) {
        crate::tasks::api::hashrate_monitor_start();
    }

    fn pool_mode(&mut self) -> c_int {
        crate::cfg::pool_mode() as c_int
    }

    fn log_invalid_pool_mode(&mut self, v: c_int) {
        log!(Error, TAG, "invalid pool mode {v}, using failover");
    }

    fn create_stratum_manager(&mut self, mode: PoolMode) {
        crate::pool_manager::api::pool_mgr_create(mode == PoolMode::Dual);
    }

    fn stratum_load_settings(&mut self) {
        crate::pool_manager::api::pool_mgr_load_settings();
    }

    /// WiFi (and Ethernet), the HTTP server, then waiting for an IP;
    /// mDNS once there is one.
    fn setup_network(&mut self) {
        let ssid = cfg_str(b"wifissid", crate::sdkconfig::ESP_WIFI_SSID);
        let pass = cfg_str(b"wifipass", crate::sdkconfig::ESP_WIFI_PASSWORD);
        let hostname = crate::cfg::hostname();
        crate::system::set_ssid(&ssid);

        crate::network::api::network_set_hook_preferred_changed(Some(on_preferred_changed));
        let has_eth = crate::board::api::board_has_ethernet();
        check(
            crate::network::start(&ssid, &pass, &hostname, has_eth),
            "network start",
        );

        // the REST server for the AP fallback / config (also reachable over
        // LAN and WiFi)
        crate::http_server::api::start_rest_server(core::ptr::null_mut());

        loop {
            if crate::network::wait_any_ip(Duration::from_secs(30)) != 0 {
                if crate::netmgr::api::netmgr_eth_has_ip() {
                    log!(Info, TAG, "Network up via ETH (preferred)");
                    crate::system::set_wifi_status(b"LAN Connected!");
                } else {
                    log!(Info, TAG, "Network up via WiFi");
                    crate::system::set_wifi_status(b"WiFi Connected!");
                }
                let b = board_info();
                let s = |p: &CStr| p.to_string_lossy().into_owned();
                crate::mdns::start(
                    &hostname.to_string_lossy(),
                    b.version,
                    b.asic_count,
                    &s(b.device_model),
                    &s(b.asic_model),
                );
                return;
            }
            log!(
                Info,
                TAG,
                "Waiting for network IP... (AP is available for config)"
            );
        }
    }

    fn has_pool_user(&mut self) -> bool {
        // without the Kconfig default: no user configured, no mining
        !cfg_str(b"stratumuser", "").is_empty()
    }

    fn ap_off_once(&mut self) {
        crate::network::shutdown_ap_once()
    }

    fn discord_start(&mut self) {
        crate::discord::start()
    }

    fn otp_init(&mut self) {
        let model = info_text(board_info().device_model);
        let hostname = crate::system::hostname().to_string_lossy().into_owned();
        let mac = crate::network::mac_addr().to_string_lossy().into_owned();
        crate::otp_device::init(&hostname, &mac, &model);
    }

    fn sntp_start(&mut self) {
        crate::clock::start()
    }

    fn discord_watchdog_alert(&mut self) {
        crate::discord::api::discord_send_watchdog();
    }

    fn start_mining_tasks(&mut self) {
        use crate::tasks::api as t;
        task(t::create_jobs_task, c"stratum miner", 8192, 10);
        task(t::ASIC_result_task, c"asic result", 8192, 15);
        task(t::influx_task, c"influx", 8192, 1);
        task(crate::apis_fetcher::api::apis_task, c"apis ticker", 8192, 5);
        task(
            crate::wifi_health::api::wifi_monitor_task,
            c"wifi monitor",
            4096,
            1,
        );
    }

    fn can_master_wanted(&mut self) -> bool {
        crate::cfg::can_enabled() && crate::board::api::board_has_can_extension()
    }

    fn start_can_master(&mut self) {
        self.can_init();
        // internal RAM, as before (xTaskCreate)
        create_internal(crate::can::api::can_master_task, c"can master", 4096, 5);
    }

    fn start_ota_task(&mut self) {
        // writes flash: its task is in internal RAM
        crate::ota_factory_task::api::ota_factory_start();
    }

    fn start_stratum_task(&mut self) {
        task(
            crate::pool_manager::api::pool_mgr_task,
            c"stratum manager",
            8192,
            5,
        );
    }

    fn is_shutdown(&mut self) -> bool {
        crate::power_mgmt::is_shutdown()
    }

    fn suspend(&mut self) {
        // the stratum task leaves the watchdog on shutdown
        log!(Warn, TAG, "suspended");
        crate::idf::task::suspend_self();
    }

    fn free_internal_heap(&mut self) -> usize {
        crate::idf::task::free_heap(CAP_INTERNAL)
    }

    fn log_low_heap(&mut self, bytes: usize) {
        log!(
            Warn,
            TAG,
            "*** WARNING *** Free internal heap: {bytes} bytes"
        );
    }
}

/// The firmware's entry point (the ESP-IDF main task).
#[cfg(target_os = "espidf")]
#[no_mangle]
pub extern "C" fn app_main() {
    let mut f = Firmware::default();
    crate::boot::start(&mut f);
    while crate::boot::idle(&mut f) {}
}
