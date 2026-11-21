//! The ASIC chain's UART (port of `components/bm1397/serial.cpp`): UART1 on
//! GPIO17 (TX) / GPIO18 (RX), 115200 baud until the driver raises it, and
//! the `asic_uart_t` callbacks of the ASIC driver (`asic/driver.rs`).
//! The IDF structs and constants come from the generated bindings.

use crate::idf::idf_const;
use core::ffi::{c_int, c_void, CStr};
use std::sync::Mutex;

use crate::asic::driver::api::CUart;
use crate::log::log;

const TAG: &CStr = c"serial";

idf_const!(PORT: u32 = uart_port_t_UART_NUM_1, 1);
const TXD: c_int = 17;
const RXD: c_int = 18;
const BUF_SIZE: c_int = 1024;
idf_const!(PIN_NO_CHANGE: c_int = UART_PIN_NO_CHANGE, -1);
idf_const!(DATA_8_BITS: u32 = uart_word_length_t_UART_DATA_8_BITS, 3);
idf_const!(PARITY_DISABLE: u32 = uart_parity_t_UART_PARITY_DISABLE, 0);
idf_const!(STOP_BITS_1: u32 = uart_stop_bits_t_UART_STOP_BITS_1, 1);
idf_const!(HW_FLOWCTRL_DISABLE: u32 = uart_hw_flowcontrol_t_UART_HW_FLOWCTRL_DISABLE, 0);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::uart_config_t as UartConfig;

/// `uart_config_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[derive(Default)]
pub struct UartConfig {
    pub baud_rate: c_int,
    pub data_bits: u32,
    pub parity: u32,
    pub stop_bits: u32,
    pub flow_ctrl: u32,
    pub rx_flow_ctrl_thresh: u8,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        uart_driver_install, uart_flush, uart_param_config, uart_read_bytes, uart_set_baudrate,
        uart_set_pin, uart_write_bytes,
    };
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    pub unsafe fn uart_param_config(_: u32, _: *const UartConfig) -> c_int {
        0
    }
    pub unsafe fn uart_set_pin(_: u32, _: c_int, _: c_int, _: c_int, _: c_int) -> c_int {
        0
    }
    pub unsafe fn uart_driver_install(
        _: u32,
        _: c_int,
        _: c_int,
        _: c_int,
        _: *mut c_void,
        _: c_int,
    ) -> c_int {
        0
    }
    pub unsafe fn uart_set_baudrate(_: u32, _: u32) -> c_int {
        0
    }
    pub unsafe fn uart_write_bytes(_: u32, _: *const c_void, len: usize) -> c_int {
        len as c_int
    }
    pub unsafe fn uart_read_bytes(_: u32, _: *mut c_void, _: u32, _: u32) -> c_int {
        0
    }
    pub unsafe fn uart_flush(_: u32) -> c_int {
        0
    }
}

/// Whole frames: the jobs, the results and the counter tasks send.
static TX: Mutex<()> = Mutex::new(());

pub fn init() {
    log!(Info, TAG, "Initializing serial");
    // the host model has no more fields
    #[allow(clippy::needless_update)]
    let cfg = UartConfig {
        baud_rate: 115_200,
        data_bits: DATA_8_BITS,
        parity: PARITY_DISABLE,
        stop_bits: STOP_BITS_1,
        flow_ctrl: HW_FLOWCTRL_DISABLE,
        rx_flow_ctrl_thresh: 122,
        // the source clock 0 (the default) and no flags, as the memset
        ..Default::default()
    };
    // SAFETY: `cfg` is a valid local, only read; null event queue with size 0 means none.
    unsafe {
        sys::uart_param_config(PORT, &cfg);
        sys::uart_set_pin(PORT, TXD, RXD, PIN_NO_CHANGE, PIN_NO_CHANGE);
        // no event queue
        sys::uart_driver_install(
            PORT,
            BUF_SIZE * 2,
            BUF_SIZE * 2,
            0,
            core::ptr::null_mut(),
            0,
        );
    }
}

pub fn set_baud(baud: c_int) {
    log!(Info, TAG, "Changing UART baud to {baud}");
    // SAFETY: plain call with the port number; the driver checks the port.
    unsafe { sys::uart_set_baudrate(PORT, baud as u32) };
}

pub fn send(data: &[u8]) -> c_int {
    let _g = TX.lock().unwrap_or_else(|e| e.into_inner());
    // SAFETY: `data` is readable for `data.len()` bytes; the driver copies them to its TX buffer.
    unsafe { sys::uart_write_bytes(PORT, data.as_ptr() as *const c_void, data.len()) }
}

/// Bytes read within `timeout_ms`, -1 on an error (as a 16-bit count, as
/// before).
pub fn rx(buf: &mut [u8], timeout_ms: u16) -> i16 {
    let len = buf.len() as u16;
    let ticks = crate::idf::ms_to_ticks(timeout_ms as u32);
    // SAFETY: at most `len` (<= `buf.len()`) bytes are written to `buf`.
    unsafe { sys::uart_read_bytes(PORT, buf.as_mut_ptr() as *mut c_void, len as u32, ticks) as i16 }
}

pub fn clear() {
    // SAFETY: plain call with the port number; the driver checks the port.
    unsafe { sys::uart_flush(PORT) };
}

unsafe extern "C" fn uart_send(_: *mut c_void, data: *const u8, len: usize) {
    send(core::slice::from_raw_parts(data, len));
}

unsafe extern "C" fn uart_rx(_: *mut c_void, buf: *mut u8, len: usize, timeout_ms: u16) -> i32 {
    // the length as the former SERIAL_rx() took it
    let len = len as u16 as usize;
    rx(core::slice::from_raw_parts_mut(buf, len), timeout_ms) as i32
}

unsafe extern "C" fn uart_clear(_: *mut c_void) {
    clear()
}

unsafe extern "C" fn uart_delay(_: *mut c_void, ms: u32) {
    crate::idf::task::delay_ms(ms)
}

/// The ASIC driver's UART.
pub fn uart() -> CUart {
    CUart {
        ctx: core::ptr::null_mut(),
        send: uart_send,
        rx: uart_rx,
        clear: uart_clear,
        delay_ms: uart_delay,
    }
}
