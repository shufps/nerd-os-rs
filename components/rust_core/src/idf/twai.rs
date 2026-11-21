//! The TWAI (CAN) driver (`driver/twai.h`) for the CAN glue (`can.rs`). In
//! the firmware the types and functions are the generated bindings
//! (`raw`); on the host, models with the same fields and failing stubs.

use super::{idf_const, EspErr};

idf_const!(pub STATE_STOPPED: u32 = twai_state_t_TWAI_STATE_STOPPED, 0);
idf_const!(pub STATE_RUNNING: u32 = twai_state_t_TWAI_STATE_RUNNING, 1);
idf_const!(pub STATE_BUS_OFF: u32 = twai_state_t_TWAI_STATE_BUS_OFF, 2);

#[cfg(target_os = "espidf")]
mod imp {
    use super::super::raw;
    use super::*;

    pub use raw::{twai_message_t as Message, twai_status_info_t as StatusInfo};

    /// `TWAI_GENERAL_CONFIG_DEFAULT(tx, rx, TWAI_MODE_NORMAL)` with a
    /// longer RX queue (the default 5 is too small for the 7-frame
    /// telemetry bursts) and a shared interrupt,
    /// `TWAI_TIMING_CONFIG_500KBITS()` (ESP32-S3: APB clock, no XTAL source;
    /// `layout_check.c` fails the build otherwise) and
    /// `TWAI_FILTER_CONFIG_ACCEPT_ALL()`.
    pub fn install_500k(tx: i32, rx: i32) -> EspErr {
        let g = raw::twai_general_config_t {
            controller_id: 0,
            mode: raw::twai_mode_t_TWAI_MODE_NORMAL,
            tx_io: tx,
            rx_io: rx,
            clkout_io: raw::gpio_num_t_GPIO_NUM_NC,
            bus_off_io: raw::gpio_num_t_GPIO_NUM_NC,
            tx_queue_len: 5,
            rx_queue_len: 32,
            alerts_enabled: raw::TWAI_ALERT_NONE,
            clkout_divider: 0,
            intr_flags: (raw::ESP_INTR_FLAG_LEVEL1 | raw::ESP_INTR_FLAG_SHARED) as i32,
        };
        let t = raw::twai_timing_config_t {
            clk_src: raw::soc_periph_twai_clk_src_t_TWAI_CLK_SRC_DEFAULT,
            quanta_resolution_hz: 8_000_000,
            brp: 0,
            tseg_1: 11,
            tseg_2: 4,
            sjw: 2,
            triple_sampling: false,
        };
        let f = raw::twai_filter_config_t {
            acceptance_code: 0,
            acceptance_mask: 0xFFFF_FFFF,
            single_filter: true,
        };
        // SAFETY: the three configs are valid locals; the driver copies what it needs.
        unsafe { raw::twai_driver_install(&g, &t, &f) }
    }
    pub fn driver_uninstall() -> EspErr {
        // SAFETY: no arguments; the driver returns an error if it is not installed or not stopped.
        unsafe { raw::twai_driver_uninstall() }
    }
    pub fn start() -> EspErr {
        // SAFETY: no arguments; the driver checks its own state.
        unsafe { raw::twai_start() }
    }
    pub fn transmit(m: &Message, ticks: u32) -> EspErr {
        // SAFETY: `m` is a valid message, only read (copied into the TX queue).
        unsafe { raw::twai_transmit(m, ticks) }
    }
    pub fn receive(m: &mut Message, ticks: u32) -> EspErr {
        // SAFETY: `m` is a valid exclusive reference the received frame is written to.
        unsafe { raw::twai_receive(m, ticks) }
    }
    pub fn status() -> Option<StatusInfo> {
        let mut s = StatusInfo::default();
        // SAFETY: `s` is a valid local the status is written to.
        (unsafe { raw::twai_get_status_info(&mut s) } == super::super::ESP_OK).then_some(s)
    }
    pub fn initiate_recovery() -> EspErr {
        // SAFETY: no arguments; the driver checks its own state.
        unsafe { raw::twai_initiate_recovery() }
    }
    pub fn clear_transmit_queue() -> EspErr {
        // SAFETY: no arguments; the driver checks its own state.
        unsafe { raw::twai_clear_transmit_queue() }
    }
}

#[cfg(not(target_os = "espidf"))]
mod imp {
    use super::*;
    use crate::idf::ESP_FAIL;

    /// `twai_message_t` (the flags are 0: a standard data frame)
    #[derive(Clone, Copy, Default)]
    pub struct Message {
        pub identifier: u32,
        pub data_length_code: u8,
        pub data: [u8; 8],
    }

    /// `twai_status_info_t`
    #[derive(Clone, Copy, Default)]
    pub struct StatusInfo {
        pub state: u32,
        pub msgs_to_tx: u32,
        pub msgs_to_rx: u32,
        pub tx_error_counter: u32,
        pub rx_error_counter: u32,
        pub tx_failed_count: u32,
    }

    pub fn install_500k(_: i32, _: i32) -> EspErr {
        ESP_FAIL
    }
    pub fn driver_uninstall() -> EspErr {
        ESP_FAIL
    }
    pub fn start() -> EspErr {
        ESP_FAIL
    }
    pub fn transmit(_: &Message, _: u32) -> EspErr {
        ESP_FAIL
    }
    pub fn receive(_: &mut Message, _: u32) -> EspErr {
        ESP_FAIL
    }
    pub fn status() -> Option<StatusInfo> {
        None
    }
    pub fn initiate_recovery() -> EspErr {
        ESP_FAIL
    }
    pub fn clear_transmit_queue() -> EspErr {
        ESP_FAIL
    }
}

pub use imp::*;

/// A standard data frame with `data` (at most 8 bytes).
pub fn message(identifier: u32, data: &[u8]) -> Message {
    let mut m = Message {
        identifier,
        data_length_code: data.len() as u8,
        ..Default::default()
    };
    m.data[..data.len()].copy_from_slice(data);
    m
}
