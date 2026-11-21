//! The display for the rest of the firmware: the driver's functions
//! ([`crate::display_driver`], no-ops on the host) and the messages to the
//! LVGL task (formerly `main/displays/ui_ipc.cpp`), which the HTTP server
//! and the CAN bus send.

use std::collections::VecDeque;
use std::sync::Mutex;

#[cfg(target_os = "espidf")]
pub use crate::display_driver::api::*;
#[cfg(not(target_os = "espidf"))]
pub use host::*;

/// A request to the LVGL task.
pub enum Msg {
    /// the OTP enrollment QR code
    ShowQr,
    HideQr,
    /// blink for this long (ms)
    Identify(u32),
}

/// as the former FreeRTOS queue
const QUEUE_LEN: usize = 8;
static QUEUE: Mutex<VecDeque<Msg>> = Mutex::new(VecDeque::new());

/// False if the queue is full.
fn send(m: Msg) -> bool {
    let mut q = QUEUE.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() >= QUEUE_LEN {
        return false;
    }
    q.push_back(m);
    true
}

/// The next request (the LVGL task).
#[cfg_attr(not(target_os = "espidf"), allow(dead_code))]
pub(crate) fn take_msg() -> Option<Msg> {
    QUEUE.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
}

pub fn send_show_qr() -> bool {
    send(Msg::ShowQr)
}

pub fn send_hide_qr() -> bool {
    send(Msg::HideQr)
}

/// Blinks the screen for `ms` (0: 60 s).
pub fn send_identify(ms: u32) -> bool {
    send(Msg::Identify(if ms == 0 { 60_000 } else { ms }))
}

/// On the host: no display.
#[cfg(not(target_os = "espidf"))]
mod host {
    use core::ffi::CStr;

    pub fn init() {}
    pub fn load_settings() {}
    pub fn wait_for_splashs() {}
    pub fn portal_screen(_: &CStr) {}
    pub fn update_ip(_: &CStr) {}
    pub fn set_network_icon(_: bool) {}
    pub fn set_can_icon() {}
    pub fn mining_screen() {}
    pub fn show_found_block() {}
    pub fn show_error(_: &CStr, _: u32) {}
    pub fn hide_error() {}
    pub fn update(_: i32) {}
    pub fn log(_: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_holds_eight() {
        while take_msg().is_some() {}
        assert!(send_identify(0));
        assert!(matches!(take_msg(), Some(Msg::Identify(60_000))));
        for _ in 0..QUEUE_LEN {
            assert!(send_show_qr());
        }
        assert!(!send_hide_qr());
        while take_msg().is_some() {}
    }
}
