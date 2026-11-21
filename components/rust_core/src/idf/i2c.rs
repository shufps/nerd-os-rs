//! The board's I2C bus (port of `main/boards/drivers/i2c_master.cpp` and the
//! SMBus transactions of `TPS53647.cpp` and `nerdaxe/TPS546.cpp`): the
//! legacy ESP-IDF I2C driver on port 0 (SDA 44, SCL 43, 100 kHz), register
//! reads and writes (`i2c_master.h`), and the SMBus bus objects for the
//! TPS53647/53667 (`tps536x7_smbus`) and TPS546 (`tps546_bus`) drivers,
//! with the same transactions (start, address, ack checks, NACK on the last
//! byte read) and 1 s timeouts as before.

use super::idf_const;
use core::ffi::{c_int, c_void};

use super::{ms_to_ticks, EspErr, Ticks, ESP_ERR_INVALID_ARG, ESP_OK};

idf_const!(
    /// `i2c_port_t` of the board's bus
    pub PORT: u32 = i2c_port_t_I2C_NUM_0, 0
);
const SDA_IO: c_int = 44;
const SCL_IO: c_int = 43;
const FREQ_HZ: u32 = 100_000;
/// `I2C_MASTER_TIMEOUT_TICKS` / `SMBUS_DEFAULT_TIMEOUT`
pub const TIMEOUT: Ticks = ms_to_ticks(1000);

idf_const!(ESP_ERR_NO_MEM: EspErr = ESP_ERR_NO_MEM, 0x101);
idf_const!(pub MODE_MASTER: u32 = i2c_mode_t_I2C_MODE_MASTER, 1);
idf_const!(WRITE_BIT: u8 = i2c_rw_t_I2C_MASTER_WRITE, 0);
idf_const!(READ_BIT: u8 = i2c_rw_t_I2C_MASTER_READ, 1);
idf_const!(ACK: u32 = i2c_ack_type_t_I2C_MASTER_ACK, 0);
idf_const!(NACK: u32 = i2c_ack_type_t_I2C_MASTER_NACK, 1);
const ACK_CHECK: bool = true;
const MAX_BLOCK_LEN: usize = 32;

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::i2c_config_t as Config;

/// `i2c_config_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Config {
    pub mode: u32,
    pub sda_io_num: c_int,
    pub scl_io_num: c_int,
    pub sda_pullup_en: bool,
    pub scl_pullup_en: bool,
    /// `master.clk_speed`
    pub clk_speed: u32,
    pub clk_flags: u32,
}

/// `i2c_cmd_handle_t`
pub type Cmd = *mut c_void;

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        i2c_cmd_link_create, i2c_cmd_link_delete, i2c_driver_delete, i2c_driver_install,
        i2c_master_cmd_begin, i2c_master_read, i2c_master_read_byte, i2c_master_start,
        i2c_master_stop, i2c_master_write_byte, i2c_master_write_read_device,
        i2c_master_write_to_device, i2c_param_config,
    };
}

/// On the host every transaction fails.
#[cfg(not(target_os = "espidf"))]
#[allow(clippy::missing_safety_doc)]
mod sys {
    use super::*;
    use crate::idf::ESP_FAIL;
    pub unsafe fn i2c_param_config(_: u32, _: *const Config) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_driver_install(_: u32, _: u32, _: usize, _: usize, _: c_int) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_driver_delete(_: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_write_read_device(
        _: u32,
        _: u8,
        _: *const u8,
        _: usize,
        _: *mut u8,
        _: usize,
        _: Ticks,
    ) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_write_to_device(
        _: u32,
        _: u8,
        _: *const u8,
        _: usize,
        _: Ticks,
    ) -> EspErr {
        ESP_FAIL
    }
    /// a placeholder link (never dereferenced: every call fails)
    pub unsafe fn i2c_cmd_link_create() -> Cmd {
        core::ptr::NonNull::dangling().as_ptr()
    }
    pub unsafe fn i2c_cmd_link_delete(_: Cmd) {}
    pub unsafe fn i2c_master_start(_: Cmd) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_stop(_: Cmd) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_write_byte(_: Cmd, _: u8, _: bool) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_read(_: Cmd, _: *mut u8, _: usize, _: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_read_byte(_: Cmd, _: *mut u8, _: u32) -> EspErr {
        ESP_FAIL
    }
    pub unsafe fn i2c_master_cmd_begin(_: u32, _: Cmd, _: Ticks) -> EspErr {
        ESP_FAIL
    }
}

/// One command link: `build` queues the transaction, then it runs on the
/// bus (the former `i2c_cmd_link_create()` .. `i2c_cmd_link_delete()`).
fn transaction(build: impl FnOnce(Cmd)) -> EspErr {
    // SAFETY: the link is created, filled by `build`, run on the installed port and deleted
    // exactly once here; it does not escape.
    unsafe {
        let cmd = sys::i2c_cmd_link_create();
        if cmd.is_null() {
            return ESP_ERR_NO_MEM;
        }
        build(cmd);
        let err = sys::i2c_master_cmd_begin(PORT, cmd, TIMEOUT);
        sys::i2c_cmd_link_delete(cmd);
        err
    }
}

fn write_addr(cmd: Cmd, addr: u8, rw: u8) {
    // SAFETY: only called from `transaction` builders with their live command link.
    unsafe {
        sys::i2c_master_write_byte(cmd, (addr << 1) | rw, ACK_CHECK);
    }
}

/// SMBus read byte
pub fn smbus_read_byte(addr: u8, command: u8, out: &mut u8) -> EspErr {
    // SAFETY: `cmd` is the live command link of `transaction`; the queued `out` pointer
    // outlives the `i2c_master_cmd_begin` run inside `transaction`.
    transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, command, ACK_CHECK);
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, READ_BIT);
        sys::i2c_master_read_byte(cmd, out, NACK);
        sys::i2c_master_stop(cmd);
    })
}

/// SMBus write byte
pub fn smbus_write_byte(addr: u8, command: u8, data: u8) -> EspErr {
    // SAFETY: `cmd` is the live command link of `transaction`; only bytes are queued.
    transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, command, ACK_CHECK);
        sys::i2c_master_write_byte(cmd, data, ACK_CHECK);
        sys::i2c_master_stop(cmd);
    })
}

/// SMBus send byte (command only)
pub fn smbus_write_command(addr: u8, command: u8) -> EspErr {
    // SAFETY: `cmd` is the live command link of `transaction`; only bytes are queued.
    transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, command, ACK_CHECK);
        sys::i2c_master_stop(cmd);
    })
}

/// SMBus read word (little endian; the result only on success)
pub fn smbus_read_word(addr: u8, command: u8, out: &mut u16) -> EspErr {
    let mut data = [0u8; 2];
    // SAFETY: `cmd` is the live command link of `transaction`; the queued pointers (into
    // `data`, 2 bytes, so `add(1)` is in bounds) outlive the `i2c_master_cmd_begin` run.
    let err = transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, command, ACK_CHECK);
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, READ_BIT);
        sys::i2c_master_read(cmd, data.as_mut_ptr(), 1, ACK);
        sys::i2c_master_read_byte(cmd, data.as_mut_ptr().add(1), NACK);
        sys::i2c_master_stop(cmd);
    });
    if err == ESP_OK {
        *out = u16::from_le_bytes(data);
    }
    err
}

/// SMBus write word (little endian)
pub fn smbus_write_word(addr: u8, command: u8, data: u16) -> EspErr {
    // SAFETY: `cmd` is the live command link of `transaction`; only bytes are queued.
    transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, command, ACK_CHECK);
        sys::i2c_master_write_byte(cmd, data as u8, ACK_CHECK);
        sys::i2c_master_write_byte(cmd, (data >> 8) as u8, ACK_CHECK);
        sys::i2c_master_stop(cmd);
    })
}

/// A big-endian word read with ACK on both bytes (the TMP1075 read of the
/// former `TMP1075.cpp`; the result only on success)
pub fn read_word_be_acked(addr: u8, reg: u8, out: &mut u16) -> EspErr {
    let mut data = [0u8; 2];
    // SAFETY: `cmd` is the live command link of `transaction`; the queued pointer to the
    // 2-byte `data` (read of 2) outlives the `i2c_master_cmd_begin` run.
    let err = transaction(|cmd| unsafe {
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, WRITE_BIT);
        sys::i2c_master_write_byte(cmd, reg, ACK_CHECK);
        sys::i2c_master_start(cmd);
        write_addr(cmd, addr, READ_BIT);
        sys::i2c_master_read(cmd, data.as_mut_ptr(), 2, ACK);
        sys::i2c_master_stop(cmd);
    });
    if err == ESP_OK {
        *out = u16::from_be_bytes(data);
    }
    err
}

/// Register read: the register address, then `len` bytes.
pub fn register_read(addr: u8, reg: u8, data: &mut [u8]) -> EspErr {
    // SAFETY: `reg` is one readable byte and `data` is writable for `data.len()` bytes.
    unsafe {
        sys::i2c_master_write_read_device(
            PORT,
            addr,
            &reg,
            1,
            data.as_mut_ptr(),
            data.len(),
            TIMEOUT,
        )
    }
}

pub fn write_to_device(addr: u8, data: &[u8]) -> EspErr {
    // SAFETY: `data` is readable for `data.len()` bytes.
    unsafe { sys::i2c_master_write_to_device(PORT, addr, data.as_ptr(), data.len(), TIMEOUT) }
}

/// SMBus block read: the length byte is read and skipped.
pub fn smbus_read_block(addr: u8, command: u8, out: &mut [u8]) -> EspErr {
    if out.len() > MAX_BLOCK_LEN {
        return ESP_ERR_INVALID_ARG;
    }
    let mut buf = [0u8; MAX_BLOCK_LEN + 1];
    let n = out.len();
    let err = register_read(addr, command, &mut buf[..n + 1]);
    if err == ESP_OK {
        out.copy_from_slice(&buf[1..n + 1]);
    }
    err
}

/// SMBus block write: command, length byte, data.
pub fn smbus_write_block(addr: u8, command: u8, data: &[u8]) -> EspErr {
    if data.len() > MAX_BLOCK_LEN {
        return ESP_ERR_INVALID_ARG;
    }
    let mut buf = [0u8; MAX_BLOCK_LEN + 2];
    buf[0] = command;
    buf[1] = data.len() as u8;
    buf[2..2 + data.len()].copy_from_slice(data);
    write_to_device(addr, &buf[..data.len() + 2])
}

/// Removes the driver.
pub fn delete() -> EspErr {
    // SAFETY: plain call with the port number; the driver checks whether it is installed.
    let err = unsafe { sys::i2c_driver_delete(PORT) };
    if err == ESP_OK {
        // a later init() installs it again
        INSTALLED.store(false, std::sync::atomic::Ordering::Release);
    }
    err
}

static INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Installs the driver (once; called from the board constructors for strap
/// detection and from `initBoard()`).
pub fn init() -> EspErr {
    use std::sync::atomic::Ordering;
    if INSTALLED.load(Ordering::Acquire) {
        return ESP_OK;
    }
    // zeroed, as the C initializer
    let mut conf = Config {
        mode: MODE_MASTER,
        sda_io_num: SDA_IO,
        scl_io_num: SCL_IO,
        sda_pullup_en: true,
        scl_pullup_en: true,
        ..Config::default()
    };
    #[cfg(target_os = "espidf")]
    {
        conf.__bindgen_anon_1.master.clk_speed = FREQ_HZ;
    }
    #[cfg(not(target_os = "espidf"))]
    {
        conf.clk_speed = FREQ_HZ;
    }
    // SAFETY: `conf` is a valid local read only during the call; no ISR or queues requested.
    let err = unsafe {
        sys::i2c_param_config(PORT, &conf);
        sys::i2c_driver_install(PORT, conf.mode, 0, 0, 0)
    };
    if err == ESP_OK {
        INSTALLED.store(true, Ordering::Release);
    }
    err
}

/// The SMBus transactions as the bus of the TPS53647/53667 and TPS546
/// drivers (on the host every transaction fails).
pub struct SmBus;

impl crate::tps536x7::Bus for SmBus {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8> {
        let mut v = 0;
        (smbus_read_byte(addr, cmd, &mut v) == ESP_OK).then_some(v)
    }
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16> {
        let mut v = 0;
        (smbus_read_word(addr, cmd, &mut v) == ESP_OK).then_some(v)
    }
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool {
        smbus_write_byte(addr, cmd, data) == ESP_OK
    }
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool {
        smbus_write_word(addr, cmd, data) == ESP_OK
    }
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool {
        smbus_write_command(addr, cmd) == ESP_OK
    }
}

impl crate::tps546::Bus for SmBus {
    fn read_byte(&mut self, addr: u8, cmd: u8) -> Option<u8> {
        crate::tps536x7::Bus::read_byte(self, addr, cmd)
    }
    fn read_word(&mut self, addr: u8, cmd: u8) -> Option<u16> {
        crate::tps536x7::Bus::read_word(self, addr, cmd)
    }
    fn write_byte(&mut self, addr: u8, cmd: u8, data: u8) -> bool {
        crate::tps536x7::Bus::write_byte(self, addr, cmd, data)
    }
    fn write_word(&mut self, addr: u8, cmd: u8, data: u16) -> bool {
        crate::tps536x7::Bus::write_word(self, addr, cmd, data)
    }
    fn write_command(&mut self, addr: u8, cmd: u8) -> bool {
        crate::tps536x7::Bus::write_command(self, addr, cmd)
    }
    fn read_block(&mut self, addr: u8, cmd: u8, out: &mut [u8]) -> bool {
        out.len() <= MAX_BLOCK_LEN && smbus_read_block(addr, cmd, out) == ESP_OK
    }
    fn write_block(&mut self, addr: u8, cmd: u8, data: &[u8]) -> bool {
        data.len() <= MAX_BLOCK_LEN && smbus_write_block(addr, cmd, data) == ESP_OK
    }
}
