//! The W5500 SPI Ethernet controller (port of `main/network/w5500.cpp`):
//! the probe for a chip on the pins, the early setup (netif, SPI bus,
//! reset, MAC/PHY driver, MAC address, netif glue) with cleanup on a
//! failure, the start with the ETH/IP event handlers and the link/IP state.
//! The IDF structs come from the generated bindings; the network manager is
//! told about a new IP and a link loss through hooks (`w5500.h`).

use crate::idf::idf_const;
use core::ffi::{c_char, c_int, c_void, CStr};
use core::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::Mutex;

use crate::idf::nvs::err_name;
use crate::idf::{EspErr, ESP_FAIL, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"w5500";

idf_const!(ETHERNET_EVENT_CONNECTED: i32 = eth_event_t_ETHERNET_EVENT_CONNECTED, 2);
idf_const!(ETHERNET_EVENT_DISCONNECTED: i32 = eth_event_t_ETHERNET_EVENT_DISCONNECTED, 3);
idf_const!(IP_EVENT_ETH_GOT_IP: i32 = ip_event_t_IP_EVENT_ETH_GOT_IP, 4);
idf_const!(ESP_EVENT_ANY_ID: i32 = ESP_EVENT_ANY_ID, -1);
idf_const!(pub GPIO_NC: c_int = gpio_num_t_GPIO_NUM_NC, -1);
idf_const!(ESP_ERR_INVALID_STATE: EspErr = ESP_ERR_INVALID_STATE, 0x103);

/// W5500 frame: 16-bit register address, control byte (block select,
/// R/W bit 2, VDM), data.
const CTRL_COMMON_READ: u8 = 0x00;
const CTRL_COMMON_WRITE: u8 = 0x04;
/// mode register (bit 7: reset), chip version (reads 0x04)
const REG_MR: u16 = 0x0000;
const REG_VERSIONR: u16 = 0x0039;
const VERSION: u8 = 0x04;

type EventHandler = unsafe extern "C" fn(*mut c_void, *const c_char, i32, *mut c_void);

// The IDF objects of the driver, owned by this module: only `sys` creates
// them (from a successful create call), and the calls that free them take
// them by value, so a freed or foreign handle cannot reach IDF. They have no
// Drop: one that is dropped without its delete call is leaked, not freed.

/// `esp_eth_mac_t *`
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
struct Mac(NonNull<c_void>);
/// `esp_eth_phy_t *`
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
struct Phy(NonNull<c_void>);
/// The ETH `esp_netif_t *`
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
struct Netif(NonNull<c_void>);
/// `esp_eth_netif_glue_handle_t`
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
struct Glue(NonNull<c_void>);
/// The installed driver (`esp_eth_handle_t`); it owns MAC and PHY until it
/// is uninstalled.
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
struct Eth {
    h: NonNull<c_void>,
    mac: Mac,
    phy: Phy,
}

/// The IDF calls: SPI on `SPI2_HOST`, the W5500 MAC/PHY driver with the
/// `ETH_*_DEFAULT_CONFIG()`s, the ETH netif. In the firmware on the
/// generated bindings; on the host, stubs.
#[cfg(target_os = "espidf")]
mod sys {
    use super::*;
    use crate::idf::raw;

    const SPI_HOST: raw::spi_host_device_t = raw::spi_host_device_t_SPI2_HOST;

    /// The bus on these pins (`ESP_ERR_INVALID_STATE`: already set up).
    pub fn spi_bus_init(sclk: c_int, mosi: c_int, miso: c_int) -> EspErr {
        let mut bus = raw::spi_bus_config_t::default();
        bus.__bindgen_anon_1.mosi_io_num = mosi;
        bus.__bindgen_anon_2.miso_io_num = miso;
        bus.sclk_io_num = sclk;
        bus.__bindgen_anon_3.quadwp_io_num = -1;
        bus.__bindgen_anon_4.quadhd_io_num = -1;
        // SAFETY: `bus` is a live local that spi_bus_initialize only reads
        unsafe { raw::spi_bus_initialize(SPI_HOST, &bus, raw::spi_common_dma_t_SPI_DMA_CH_AUTO) }
    }
    pub fn spi_bus_free() {
        // SAFETY: the bus is freed only by its owner (we_init_bus) or after an init failure;
        // an error code is returned if devices are still attached
        unsafe { raw::spi_bus_free(SPI_HOST) };
    }
    /// The W5500 frame format at 2 MHz (mode 0).
    fn device_config(cs: c_int, queue_size: c_int) -> raw::spi_device_interface_config_t {
        raw::spi_device_interface_config_t {
            command_bits: 16,
            address_bits: 8,
            mode: 0,
            clock_speed_hz: 2_000_000,
            spics_io_num: cs,
            queue_size,
            ..Default::default()
        }
    }
    /// One register access with a temporary device (read or write per the
    /// control byte).
    pub fn reg_access(cs: c_int, reg: u16, ctrl: u8, data: &mut u8) -> EspErr {
        let cfg = device_config(cs, 1);
        let mut dev = core::ptr::null_mut();
        // SAFETY: `cfg` and `dev` are live locals; the driver copies the config
        let e = unsafe { raw::spi_bus_add_device(SPI_HOST, &cfg, &mut dev) };
        if e != ESP_OK {
            return e;
        }
        let write = ctrl & 0x04 != 0;
        let mut t = raw::spi_transaction_t {
            cmd: reg,
            addr: ctrl as u64,
            length: 8,
            ..Default::default()
        };
        if write {
            t.flags = raw::SPI_TRANS_USE_TXDATA;
            // SAFETY: SPI_TRANS_USE_TXDATA is set, so `tx_data` is the active union field
            unsafe { t.__bindgen_anon_1.tx_data[0] = *data };
        } else {
            t.rxlength = 8;
            t.flags = raw::SPI_TRANS_USE_RXDATA;
        }
        // SAFETY: `dev` comes from the successful spi_bus_add_device above and `t` is a live
        // local transaction (TX/RX in its inline data per the flags)
        let e = unsafe { raw::spi_device_polling_transmit(dev, &mut t) };
        if e == ESP_OK && !write {
            // SAFETY: SPI_TRANS_USE_RXDATA was set, so the driver filled `rx_data`
            *data = unsafe { t.__bindgen_anon_2.rx_data[0] };
        }
        // SAFETY: `dev` comes from spi_bus_add_device above and is removed only here
        unsafe { raw::spi_bus_remove_device(dev) };
        e
    }
    pub fn gpio_output(pin: c_int) -> EspErr {
        let io = raw::gpio_config_t {
            pin_bit_mask: 1u64 << pin,
            mode: raw::gpio_mode_t_GPIO_MODE_OUTPUT,
            ..Default::default()
        };
        // SAFETY: `io` is a live local that gpio_config only reads
        unsafe { raw::gpio_config(&io) }
    }
    pub fn gpio_set_level(pin: c_int, level: u32) {
        // SAFETY: plain value arguments; an invalid pin returns an error code
        unsafe { raw::gpio_set_level(pin, level) };
    }
    pub fn efuse_mac() -> Option<[u8; 6]> {
        let mut mac = [0u8; 6];
        // SAFETY: `mac` is a 6-byte local, the size esp_efuse_mac_get_default writes
        (unsafe { raw::esp_efuse_mac_get_default(mac.as_mut_ptr()) } == ESP_OK).then_some(mac)
    }
    /// `esp_netif_new(ESP_NETIF_DEFAULT_ETH())`
    pub fn netif_new_eth() -> Option<Netif> {
        // SAFETY: only takes the address of / copies the ETH netif default config statics,
        // as ESP_NETIF_DEFAULT_ETH() does; `cfg` is a live local that is only read
        let p = unsafe {
            let cfg = raw::esp_netif_config_t {
                base: core::ptr::addr_of!(raw::_g_esp_netif_inherent_eth_config),
                driver: core::ptr::null(),
                stack: raw::_g_esp_netif_netstack_default_eth,
            };
            raw::esp_netif_new(&cfg)
        };
        NonNull::new(p.cast()).map(Netif)
    }
    pub fn netif_destroy(netif: Netif) {
        // SAFETY: a netif from esp_netif_new, consumed here (destroyed once)
        unsafe { raw::esp_netif_destroy(netif.0.as_ptr().cast()) };
    }
    pub fn netif_attach(netif: &Netif, glue: &Glue) -> EspErr {
        // SAFETY: both are live objects from their create calls (not freed: owned)
        unsafe { raw::esp_netif_attach(netif.0.as_ptr().cast(), glue.0.as_ptr().cast()) }
    }
    /// The MAC driver: `ETH_MAC_DEFAULT_CONFIG()` and
    /// `ETH_W5500_DEFAULT_CONFIG()` with the interrupt pin, else 1 ms
    /// polling (16 KB RX buffer: safe).
    pub fn mac_new(cs: c_int, irq: c_int) -> Option<Mac> {
        let dev = device_config(cs, 20);
        let w5500 = raw::eth_w5500_config_t {
            int_gpio_num: irq,
            poll_period_ms: if irq == GPIO_NC { 1 } else { 0 },
            spi_host_id: SPI_HOST,
            spi_devcfg: &dev as *const _ as *mut _,
            custom_spi_driver: Default::default(),
        };
        let mac = raw::eth_mac_config_t {
            sw_reset_timeout_ms: 100,
            rx_task_stack_size: 4096,
            rx_task_prio: 15,
            flags: 0,
        };
        // SAFETY: `w5500`, `mac` and `dev` are live locals; the driver adds its SPI device
        // from `spi_devcfg` during this call and keeps no pointer to them
        let p = unsafe { raw::esp_eth_mac_new_w5500(&w5500, &mac) };
        NonNull::new(p.cast()).map(Mac)
    }
    /// The PHY driver: `ETH_PHY_DEFAULT_CONFIG()` without its reset GPIO.
    /// The default (GPIO 5) is the LCD reset of the T-Display S3, and the
    /// driver pulses it on init; the W5500 is reset before (hardware pin or
    /// SPI soft reset), so the PHY driver gets no reset pin.
    pub fn phy_new() -> Option<Phy> {
        let phy = raw::eth_phy_config_t {
            phy_addr: -1,
            reset_timeout_ms: 100,
            autonego_timeout_ms: 4000,
            reset_gpio_num: -1,
        };
        // SAFETY: `phy` is a live local the driver copies
        let p = unsafe { raw::esp_eth_phy_new_w5500(&phy) };
        NonNull::new(p.cast()).map(Phy)
    }
    pub fn mac_del(mac: Mac) {
        let mac = mac.0.as_ptr() as *mut raw::esp_eth_mac_t;
        // SAFETY: a MAC object from esp_eth_mac_new_w5500, not owned by an installed
        // driver (Eth would hold it), consumed here
        unsafe {
            if let Some(del) = (*mac).del {
                del(mac);
            }
        }
    }
    pub fn phy_del(phy: Phy) {
        let phy = phy.0.as_ptr() as *mut raw::esp_eth_phy_t;
        // SAFETY: as mac_del, for the PHY object
        unsafe {
            if let Some(del) = (*phy).del {
                del(phy);
            }
        }
    }
    /// `esp_eth_driver_install(ETH_DEFAULT_CONFIG(mac, phy))`; on a failure
    /// MAC and PHY come back.
    pub fn driver_install(mac: Mac, phy: Phy) -> Result<Eth, (EspErr, Mac, Phy)> {
        let cfg = raw::esp_eth_config_t {
            mac: mac.0.as_ptr().cast(),
            phy: phy.0.as_ptr().cast(),
            check_link_period_ms: 2000,
            ..Default::default()
        };
        let mut h = core::ptr::null_mut();
        // SAFETY: `cfg` and `h` are live locals; MAC and PHY are live objects, which the
        // driver uses from now on (the Eth owns them)
        let e = unsafe { raw::esp_eth_driver_install(&cfg, &mut h) };
        match NonNull::new(h) {
            Some(h) if e == ESP_OK => Ok(Eth { h, mac, phy }),
            _ => Err((if e != ESP_OK { e } else { ESP_FAIL }, mac, phy)),
        }
    }
    /// Uninstalls the driver; MAC and PHY come back only then (else the
    /// driver keeps using them and gets them back).
    pub fn driver_uninstall(eth: Eth) -> Result<(Mac, Phy), (EspErr, Eth)> {
        // SAFETY: the installed driver handle, uninstalled once (consumed on success)
        let e = unsafe { raw::esp_eth_driver_uninstall(eth.h.as_ptr()) };
        if e == ESP_OK {
            Ok((eth.mac, eth.phy))
        } else {
            Err((e, eth))
        }
    }
    pub fn set_mac(eth: &Eth, mac: &mut [u8; 6]) -> EspErr {
        // SAFETY: the installed driver handle and a 6-byte buffer, the size
        // ETH_CMD_S_MAC_ADDR reads
        unsafe {
            raw::esp_eth_ioctl(
                eth.h.as_ptr(),
                raw::esp_eth_io_cmd_t_ETH_CMD_S_MAC_ADDR,
                mac.as_mut_ptr().cast(),
            )
        }
    }
    pub fn glue_new(eth: &Eth) -> Option<Glue> {
        // SAFETY: the installed driver handle
        let p = unsafe { raw::esp_eth_new_netif_glue(eth.h.as_ptr()) };
        NonNull::new(p.cast()).map(Glue)
    }
    pub fn glue_del(glue: Glue) {
        // SAFETY: a glue from esp_eth_new_netif_glue, consumed here (deleted once)
        unsafe { raw::esp_eth_del_netif_glue(glue.0.as_ptr().cast()) };
    }
    pub fn eth_start(eth: &Eth) -> EspErr {
        // SAFETY: the installed driver handle
        unsafe { raw::esp_eth_start(eth.h.as_ptr()) }
    }
    /// A handler for an ETH (`eth`) or IP event.
    pub fn register_handler(eth: bool, id: i32, handler: EventHandler) -> EspErr {
        // SAFETY: ETH_EVENT/IP_EVENT are immutable link-time event bases and `handler`
        // is a valid extern "C" fn; no handler argument
        unsafe {
            let base = if eth { raw::ETH_EVENT } else { raw::IP_EVENT };
            raw::esp_event_handler_register(base, id, Some(handler), core::ptr::null_mut())
        }
    }
    /// IP, netmask and gateway of an `IP_EVENT_ETH_GOT_IP`.
    ///
    /// # Safety
    /// `data` the event's `ip_event_got_ip_t`.
    pub unsafe fn got_ip(data: *mut c_void) -> [[u8; 4]; 3] {
        let i = &(*(data as *const raw::ip_event_got_ip_t)).ip_info;
        [
            i.ip.addr.to_ne_bytes(),
            i.netmask.addr.to_ne_bytes(),
            i.gw.addr.to_ne_bytes(),
        ]
    }
}

#[cfg(not(target_os = "espidf"))]
#[allow(clippy::missing_safety_doc)]
mod sys {
    use super::*;
    pub fn spi_bus_init(_: c_int, _: c_int, _: c_int) -> EspErr {
        ESP_FAIL
    }
    pub fn spi_bus_free() {}
    pub fn reg_access(_: c_int, _: u16, _: u8, _: &mut u8) -> EspErr {
        ESP_FAIL
    }
    pub fn gpio_output(_: c_int) -> EspErr {
        ESP_OK
    }
    pub fn gpio_set_level(_: c_int, _: u32) {}
    pub fn efuse_mac() -> Option<[u8; 6]> {
        None
    }
    pub fn netif_new_eth() -> Option<Netif> {
        None
    }
    pub fn netif_destroy(_: Netif) {}
    pub fn netif_attach(_: &Netif, _: &Glue) -> EspErr {
        ESP_FAIL
    }
    pub fn mac_new(_: c_int, _: c_int) -> Option<Mac> {
        None
    }
    pub fn phy_new() -> Option<Phy> {
        None
    }
    pub fn mac_del(_: Mac) {}
    pub fn phy_del(_: Phy) {}
    pub fn driver_install(mac: Mac, phy: Phy) -> Result<Eth, (EspErr, Mac, Phy)> {
        Err((ESP_FAIL, mac, phy))
    }
    pub fn driver_uninstall(eth: Eth) -> Result<(Mac, Phy), (EspErr, Eth)> {
        Ok((eth.mac, eth.phy))
    }
    pub fn set_mac(_: &Eth, _: &mut [u8; 6]) -> EspErr {
        ESP_FAIL
    }
    pub fn glue_new(_: &Eth) -> Option<Glue> {
        None
    }
    pub fn glue_del(_: Glue) {}
    pub fn eth_start(_: &Eth) -> EspErr {
        ESP_FAIL
    }
    pub fn register_handler(_: bool, _: i32, _: EventHandler) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn got_ip(_: *mut c_void) -> [[u8; 4]; 3] {
        [[0; 4]; 3]
    }
}

fn delay_ms(ms: u32) {
    std::thread::sleep(std::time::Duration::from_millis(ms as u64));
}

fn reg_access(cs: c_int, reg: u16, ctrl: u8, data: &mut u8) -> EspErr {
    sys::reg_access(cs, reg, ctrl, data)
}

/// True if a W5500 answers on these pins (VERSIONR). Sets the SPI bus up
/// if needed and frees it again, so it works before `early_init()`.
pub fn probe(sclk: c_int, mosi: c_int, miso: c_int, cs: c_int) -> bool {
    if [sclk, mosi, miso, cs].contains(&GPIO_NC) {
        return false;
    }
    let e = sys::spi_bus_init(sclk, mosi, miso);
    let we_init = e == ESP_OK;
    if e != ESP_OK && e != ESP_ERR_INVALID_STATE {
        log!(
            Warn,
            TAG,
            "probe: spi_bus_initialize failed ({})",
            err_name(e)
        );
        return false;
    }
    let mut ver = 0u8;
    let e = reg_access(cs, REG_VERSIONR, CTRL_COMMON_READ, &mut ver);
    if e != ESP_OK {
        log!(Warn, TAG, "probe: VERSIONR read failed ({})", err_name(e));
    }
    let present = e == ESP_OK && ver == VERSION;
    log!(
        Info,
        TAG,
        "probe: VERSIONR=0x{ver:02x} -> {}",
        if present { "present" } else { "absent" }
    );
    if we_init {
        sys::spi_bus_free();
    }
    present
}

/// The pins (`GPIO_NC`: rst = no reset line (RC on the board), irq =
/// polling).
#[derive(Clone, Copy)]
struct Pins {
    sclk: c_int,
    mosi: c_int,
    miso: c_int,
    cs: c_int,
    rst: c_int,
    irq: c_int,
}

/// What `early_init()` set up (released in reverse order on a failure).
struct Driver {
    pins: Pins,
    we_init_bus: bool,
    /// MAC and PHY until the driver is installed (then `eth` owns them)
    mac: Option<Mac>,
    phy: Option<Phy>,
    glue: Option<Glue>,
    netif: Option<Netif>,
    eth: Option<Eth>,
}

// SAFETY: the IDF objects may be used from any task; the driver lock
// serializes the calls that create, attach and free them.
unsafe impl Send for Driver {}

static DRIVER: Mutex<Driver> = Mutex::new(Driver {
    pins: Pins {
        sclk: 2,
        mosi: 12,
        miso: 16,
        cs: 21,
        rst: 13,
        irq: 11,
    },
    we_init_bus: false,
    mac: None,
    phy: None,
    glue: None,
    netif: None,
    eth: None,
});

/// Read by the event handlers' hooks (the network manager): not behind the
/// driver lock, which `start()` must not hold while it registers handlers.
static NETIF: AtomicPtr<c_void> = AtomicPtr::new(core::ptr::null_mut());
static STARTED: AtomicBool = AtomicBool::new(false);

fn driver() -> std::sync::MutexGuard<'static, Driver> {
    DRIVER.lock().unwrap_or_else(|e| e.into_inner())
}

/// The link and address (written by the event task).
static LINK_UP: AtomicBool = AtomicBool::new(false);
static HAS_IP: AtomicBool = AtomicBool::new(false);
static IP: Mutex<String> = Mutex::new(String::new());

/// The network manager's hooks: a new IP, a lost link.
type Hook = unsafe extern "C" fn();
static HOOKS: Mutex<(Option<Hook>, Option<Hook>)> = Mutex::new((None, None));

pub fn set_pins(sclk: c_int, mosi: c_int, miso: c_int, cs: c_int, rst: c_int, irq: c_int) {
    driver().pins = Pins {
        sclk,
        mosi,
        miso,
        cs,
        rst,
        irq,
    };
}

/// The ETH MAC: the base MAC, locally administered, last bit flipped.
fn eth_mac() -> [u8; 6] {
    let Some(mut mac) = sys::efuse_mac() else {
        return [0x02, 0, 0, 0, 0, 0x01];
    };
    mac[0] = (mac[0] & 0xFE) | 0x02;
    mac[5] ^= 0x01;
    mac
}

fn hw_reset(rst: c_int) {
    if sys::gpio_output(rst) != ESP_OK {
        log!(Error, TAG, "RST GPIO {rst} config failed");
        return;
    }
    sys::gpio_set_level(rst, 0);
    delay_ms(50);
    sys::gpio_set_level(rst, 1);
    delay_ms(200);
}

/// Software reset over SPI (MR.RST): the Eth interposer has no RST pin.
fn sw_reset(cs: c_int) {
    let mut mr = 0x80u8;
    let e = reg_access(cs, REG_MR, CTRL_COMMON_WRITE, &mut mr);
    // done in about 1 ms
    delay_ms(2);
    log!(
        Info,
        TAG,
        "W5500 software reset (MR RST) {}",
        if e == ESP_OK { "sent" } else { "failed" }
    );
}

/// Releases what `early_init()` set up, in reverse order.
fn release(d: &mut Driver) {
    // as the IDF example tears it down: the glue holds a reference on the
    // driver, which refuses to uninstall while it is attached
    if let Some(g) = d.glue.take() {
        sys::glue_del(g);
    }
    if let Some(n) = d.netif.take() {
        sys::netif_destroy(n);
    }
    if let Some(eth) = d.eth.take() {
        match sys::driver_uninstall(eth) {
            Ok((mac, phy)) => {
                d.mac = Some(mac);
                d.phy = Some(phy);
            }
            Err((e, _eth)) => {
                // the driver may still use MAC and PHY: dropping the handle
                // (no Drop) leaks them rather than freeing objects in use
                log!(Error, TAG, "driver uninstall failed: {}", err_name(e));
            }
        }
    }
    if let Some(p) = d.phy.take() {
        sys::phy_del(p);
    }
    if let Some(m) = d.mac.take() {
        sys::mac_del(m);
    }
    if d.we_init_bus {
        sys::spi_bus_free();
        d.we_init_bus = false;
    }
}

fn early_steps(d: &mut Driver) -> EspErr {
    let p = d.pins;
    let e = sys::spi_bus_init(p.sclk, p.mosi, p.miso);
    if e != ESP_OK && e != ESP_ERR_INVALID_STATE {
        log!(Error, TAG, "spi_bus_initialize failed: {}", err_name(e));
        return e;
    }
    d.we_init_bus = e == ESP_OK;

    // a dedicated RST GPIO if wired, else a software reset over SPI
    if p.rst != GPIO_NC {
        hw_reset(p.rst);
    } else {
        sw_reset(p.cs);
    }

    let Some(mac) = sys::mac_new(p.cs, p.irq) else {
        log!(Error, TAG, "esp_eth_mac_new_w5500 failed");
        return ESP_FAIL;
    };
    d.mac = Some(mac);
    let Some(phy) = sys::phy_new() else {
        log!(Error, TAG, "esp_eth_phy_new_w5500 failed");
        return ESP_FAIL;
    };
    let mac = d.mac.take().expect("set above");
    let eth = match sys::driver_install(mac, phy) {
        Ok(eth) => eth,
        Err((e, mac, phy)) => {
            log!(Error, TAG, "esp_eth_driver_install failed: {}", err_name(e));
            d.mac = Some(mac);
            d.phy = Some(phy);
            return e;
        }
    };
    let eth = d.eth.insert(eth);

    let mut mac = eth_mac();
    let e = sys::set_mac(eth, &mut mac);
    if e != ESP_OK {
        // the chip keeps its default MAC; not worth a boot loop
        log!(Error, TAG, "setting the ETH MAC failed: {}", err_name(e));
    } else {
        log!(
            Warn,
            TAG,
            "ETH MAC: {}",
            mac.iter()
                .map(|b| format!("{b:02X}"))
                .collect::<Vec<_>>()
                .join(":")
        );
    }

    let Some(glue) = sys::glue_new(eth) else {
        log!(Error, TAG, "esp_eth_new_netif_glue returned NULL");
        return ESP_FAIL;
    };
    let glue = d.glue.insert(glue);
    let Some(netif) = d.netif.as_ref() else {
        return ESP_FAIL;
    };
    let e = sys::netif_attach(netif, glue);
    if e != ESP_OK {
        log!(Error, TAG, "esp_netif_attach failed: {}", err_name(e));
        return e;
    }
    ESP_OK
}

/// Netif, SPI bus and driver; on a failure everything is released (no ETH
/// netif, the bus free) and `start()` refuses.
pub fn early_init() -> EspErr {
    let mut d = driver();
    if d.eth.is_some() {
        return ESP_OK;
    }
    let p = d.pins;
    log!(
        Warn,
        TAG,
        "W5500::init start (sclk={} mosi={} miso={} cs={} rst={} int={})",
        p.sclk,
        p.mosi,
        p.miso,
        p.cs,
        p.rst,
        p.irq
    );
    let Some(netif) = sys::netif_new_eth() else {
        log!(Error, TAG, "esp_netif_new(ETH) failed");
        return ESP_FAIL;
    };
    d.netif = Some(netif);
    let e = early_steps(&mut d);
    if e != ESP_OK {
        // a failed ETH must not leave a netif without driver or a claimed bus
        release(&mut d);
    } else if let Some(n) = &d.netif {
        // published only now: from here on it is never destroyed (release()
        // runs only on a failed setup, early_init() returns early after one)
        NETIF.store(n.0.as_ptr(), Ordering::Release);
    }
    e
}

unsafe extern "C" fn eth_event(_: *mut c_void, _: *const c_char, id: i32, _: *mut c_void) {
    match id {
        ETHERNET_EVENT_CONNECTED => {
            LINK_UP.store(true, Ordering::Release);
            log!(Warn, TAG, "ETH link UP");
        }
        ETHERNET_EVENT_DISCONNECTED => {
            LINK_UP.store(false, Ordering::Release);
            HAS_IP.store(false, Ordering::Release);
            *IP.lock().unwrap_or_else(|e| e.into_inner()) = String::new();
            log!(Warn, TAG, "ETH link DOWN");
            // not under the lock: the manager may set the hooks
            let hook = HOOKS.lock().unwrap_or_else(|e| e.into_inner()).1;
            if let Some(h) = hook {
                h();
            }
        }
        _ => {}
    }
}

fn ip_str(a: &[u8; 4]) -> String {
    format!("{}.{}.{}.{}", a[0], a[1], a[2], a[3])
}

unsafe extern "C" fn ip_event(_: *mut c_void, _: *const c_char, id: i32, data: *mut c_void) {
    if id != IP_EVENT_ETH_GOT_IP || data.is_null() {
        return;
    }
    let [ip, netmask, gw] = sys::got_ip(data);
    *IP.lock().unwrap_or_else(|e| e.into_inner()) = ip_str(&ip);
    HAS_IP.store(true, Ordering::Release);
    log!(
        Warn,
        TAG,
        "ETH GOT IP: {}  MASK: {}  GW: {}",
        ip_str(&ip),
        ip_str(&netmask),
        ip_str(&gw)
    );
    let hook = HOOKS.lock().unwrap_or_else(|e| e.into_inner()).0;
    if let Some(h) = hook {
        h();
    }
}

/// Registers the event handlers and starts the driver (once).
pub fn start() -> EspErr {
    if STARTED.load(Ordering::Acquire) {
        return ESP_OK;
    }
    if driver().eth.is_none() {
        // early_init() failed (or was not called): nothing to start
        return ESP_ERR_INVALID_STATE;
    }
    // without the driver lock (the handlers' registration)
    let e = sys::register_handler(true, ESP_EVENT_ANY_ID, eth_event);
    assert_eq!(e, ESP_OK, "esp_event_handler_register(ETH_EVENT)");
    let e = sys::register_handler(false, IP_EVENT_ETH_GOT_IP, ip_event);
    assert_eq!(e, ESP_OK, "esp_event_handler_register(IP_EVENT)");
    let e = match driver().eth.as_ref() {
        Some(eth) => sys::eth_start(eth),
        None => ESP_ERR_INVALID_STATE,
    };
    if e != ESP_OK {
        log!(Error, TAG, "esp_eth_start failed: {}", err_name(e));
        return e;
    }
    STARTED.store(true, Ordering::Release);
    log!(Warn, TAG, "W5500 init done");
    ESP_OK
}

/// The ETH netif once its setup succeeded.
pub fn netif() -> Option<crate::network::NetifRef> {
    // SAFETY: NETIF holds only a netif whose setup succeeded, and such a
    // netif is never destroyed (see early_init())
    unsafe { crate::network::NetifRef::permanent(NETIF.load(Ordering::Acquire)) }
}

pub fn has_ip() -> bool {
    HAS_IP.load(Ordering::Acquire)
}

pub fn link_up() -> bool {
    LINK_UP.load(Ordering::Acquire)
}

/// The address ("0.0.0.0" without one).
pub fn ip() -> String {
    let ip = IP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if ip.is_empty() {
        "0.0.0.0".into()
    } else {
        ip
    }
}

pub(crate) mod ffi {
    use super::*;

    /// The network manager's hooks: a new IP, a lost link.
    pub extern "C" fn w5500_set_hooks(got_ip: Option<Hook>, link_down: Option<Hook>) {
        *HOOKS.lock().unwrap_or_else(|e| e.into_inner()) = (got_ip, link_down);
    }
}
