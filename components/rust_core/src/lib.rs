//! The ESP-Miner firmware.
//!
//! This crate is built as a single static library and linked into the ESP-IDF
//! firmware by `CMakeLists.txt`; its entry point is `app_main()`
//! (`board/boot_firmware.rs`). C calls into it only through `app_main`,
//! `ws_log_enqueue` and the callbacks it registers.
//!
//! All Rust code of the firmware must live in this one crate: two Rust static
//! libraries in the same firmware would each bring their own copy of `std`.

// every unsafe block says why it is sound
#![warn(clippy::undocumented_unsafe_blocks)]
// symbols C calls must use the C ABI
#![deny(clippy::no_mangle_with_rust_abi)]

pub mod ajson;
pub mod alerts;
#[cfg(target_os = "espidf")]
mod alloc;
pub mod apis;
pub mod apis_fetcher;
pub mod board;
pub mod can;
pub mod can_master;
pub mod can_slave;
pub mod clock;
pub mod config;
pub mod crt_bundle;
pub mod crypto;
pub mod cutil;
pub mod discord;
pub mod display;
pub mod drivers;
pub mod fan_control;
pub mod heartbeat;
pub mod http;
pub mod idf;
pub mod influx;
mod log;
pub mod mining;
pub mod net;
pub mod ota_factory;
pub mod ota_factory_task;
pub mod ota_upload;
pub mod otp;
pub mod otp_device;
pub mod pool;
pub mod power_mgmt;
pub mod power_task;
pub mod secp;
pub mod stack_monitor;
pub mod suffix;
pub mod system;
pub mod tasks;
