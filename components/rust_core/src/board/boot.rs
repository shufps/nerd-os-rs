//! The boot sequence (port of `app_main()` in `main/main.cpp`): early system
//! setup, the board with its Ethernet, the optional self test, then either
//! the CAN slave (ASICs fed by the master) or the full stack (pool manager,
//! system task and display, network, and once a pool user is configured:
//! OTP, SNTP, alerts, ASICs and the mining, API, monitor, CAN master, OTA
//! and stratum tasks), and the idle loop (suspend on shutdown, low heap
//! warning). The order and the decisions are here; the steps are the
//! [`Hooks`] (the firmware's: `board/boot_firmware.rs`, with `app_main()`).
//!
//! An invalid pool mode in the config falls back to failover, so the boot
//! always reaches display, network and HTTP server and the setting can be
//! corrected from the web UI.

use core::ffi::c_int;

/// Below this the idle loop warns (bytes of internal heap).
pub const LOW_HEAP: usize = 10000;

/// The self test holds the boot for an hour (ms).
pub const SELF_TEST_HOLD_MS: u32 = 60 * 60 * 1000;

/// The idle loop period (ms).
pub const IDLE_MS: u32 = 10000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PoolMode {
    Failover = 0,
    Dual = 1,
}

/// The pool mode of the config value; failover for an invalid one.
pub fn pool_mode(v: c_int) -> (PoolMode, bool) {
    match v {
        0 => (PoolMode::Failover, true),
        1 => (PoolMode::Dual, true),
        _ => (PoolMode::Failover, false),
    }
}

pub trait Hooks {
    /// watchdog, mbedTLS on PSRAM, NVS, reset reason, config migration, GPIO
    /// ISR service, netif, default event loop; true if the last reset was
    /// the task watchdog
    fn early_init(&mut self) -> bool;
    fn board_create(&mut self);
    /// the W5500 if the board has Ethernet (once)
    fn eth_init(&mut self);
    fn board_load_settings(&mut self);
    fn board_init(&mut self);
    fn board_detect_eth(&mut self);
    /// `SYSTEM_MODULE.setBoard()` and the board/PSRAM log lines
    fn system_set_board(&mut self);
    fn self_test_enabled(&mut self) -> bool;
    fn best_diff(&mut self) -> u64;
    fn self_test(&mut self);
    fn delay_ms(&mut self, ms: u32);
    fn is_can_slave(&mut self) -> bool;
    fn log_can_slave(&mut self);
    /// `SYSTEM_MODULE.init()` + `initDisplay()`
    fn system_init(&mut self);
    /// the system and power management tasks, then `setStartupDone()`
    fn start_system_tasks(&mut self);
    fn can_init(&mut self);
    /// `initAsics()` under the power management lock
    fn init_asics(&mut self);
    fn start_can_slave_tasks(&mut self);
    fn has_hashrate_counter(&mut self) -> bool;
    fn start_hashrate_monitor(&mut self);
    /// the raw config value
    fn pool_mode(&mut self) -> c_int;
    fn log_invalid_pool_mode(&mut self, v: c_int);
    fn create_stratum_manager(&mut self, mode: PoolMode);
    fn stratum_load_settings(&mut self);
    /// blocks until an interface has an IP
    fn setup_network(&mut self);
    fn has_pool_user(&mut self) -> bool;
    fn ap_off_once(&mut self);
    fn discord_start(&mut self);
    fn otp_init(&mut self);
    fn sntp_start(&mut self);
    fn discord_watchdog_alert(&mut self);
    /// create jobs, ASIC results, influx, APIs, WiFi monitor
    fn start_mining_tasks(&mut self);
    fn can_master_wanted(&mut self) -> bool;
    fn start_can_master(&mut self);
    fn start_ota_task(&mut self);
    fn start_stratum_task(&mut self);
    fn is_shutdown(&mut self) -> bool;
    /// `vTaskSuspend(NULL)`; returns only in tests
    fn suspend(&mut self);
    fn free_internal_heap(&mut self) -> usize;
    fn log_low_heap(&mut self, bytes: usize);
}

/// The boot up to the idle loop.
pub fn start<H: Hooks>(h: &mut H) {
    let watchdog_reset = h.early_init();
    h.board_create();

    // on-board W5500 (Q1370/Q1373) or a HAT found in the constructor (Gaia)
    h.eth_init();
    h.board_load_settings();
    h.board_init();
    // pluggable HATs only detectable with the board up; before the display
    h.board_detect_eth();
    h.eth_init();
    h.system_set_board();

    if h.self_test_enabled() && h.best_diff() == 0 {
        h.self_test();
        h.delay_ms(SELF_TEST_HOLD_MS);
    }

    if h.is_can_slave() {
        // no network, stratum or HTTP server: jobs come from the master
        h.log_can_slave();
        h.system_init();
        h.start_system_tasks();
        h.can_init();
        h.init_asics();
        h.start_can_slave_tasks();
        if h.has_hashrate_counter() {
            h.start_hashrate_monitor();
        }
        return;
    }

    let v = h.pool_mode();
    let (mode, valid) = pool_mode(v);
    if !valid {
        h.log_invalid_pool_mode(v);
    }
    h.create_stratum_manager(mode);
    h.stratum_load_settings();

    // the display tasks before the network: setup_network() blocks without
    // network and the portal screen needs the system task
    h.system_init();
    h.start_system_tasks();
    h.setup_network();

    // mining starts once a pool user is configured
    if !h.has_pool_user() {
        return;
    }
    h.ap_off_once();
    h.discord_start();
    h.otp_init();
    h.sntp_start();
    // alerting only in normal operation
    if watchdog_reset {
        h.discord_watchdog_alert();
    }
    h.init_asics();
    h.start_mining_tasks();
    // only with CAN hardware (shared with the Ethernet HAT on the NerdQAxe)
    if h.can_master_wanted() {
        h.start_can_master();
    }
    h.start_ota_task();
    h.start_stratum_task();
    if h.has_hashrate_counter() {
        h.start_hashrate_monitor();
    }
}

/// One period of the idle loop: false after a suspend (tests only).
pub fn idle<H: Hooks>(h: &mut H) -> bool {
    h.delay_ms(IDLE_MS);
    if h.is_shutdown() {
        h.suspend();
        return false;
    }
    let free = h.free_internal_heap();
    if free < LOW_HEAP {
        h.log_low_heap(free);
    }
    true
}
