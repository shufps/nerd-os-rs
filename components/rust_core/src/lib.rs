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
#[cfg(target_os = "espidf")]
mod alloc;
pub mod board;
pub mod can;
pub mod can_master;
pub mod can_slave;
pub mod clock;
pub mod config;
pub mod cutil;
pub mod display;
pub mod drivers;
pub mod fan_control;
pub mod http;
pub mod idf;
mod log;
pub mod mining;
pub mod net;
pub mod ota;
pub mod pool;
pub mod power_mgmt;
pub mod power_task;
pub mod security;
pub mod services;
pub mod stack_monitor;
pub mod suffix;
pub mod system;
pub mod tasks;
