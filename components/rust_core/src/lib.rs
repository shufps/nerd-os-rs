//! Rust parts of the ESP-Miner firmware.
//!
//! This crate is built as a single static library and linked into the ESP-IDF
//! firmware by `CMakeLists.txt`. Every module exposes a C ABI that matches a
//! header in `include/`, so C/C++ callers do not notice the language change.
//!
//! All Rust code of the firmware must live in this one crate: two Rust static
//! libraries in the same firmware would each bring their own copy of `std`.

// every unsafe block says why it is sound
#![warn(clippy::undocumented_unsafe_blocks)]

pub mod ajson;
pub mod alerts;
#[cfg(target_os = "espidf")]
mod alloc;
pub mod apis;
pub mod apis_fetcher;
pub mod asic;
pub mod board;
pub mod board_api;
pub mod board_core;
pub mod board_power;
pub mod board_profile;
pub mod boot;
pub mod boot_firmware;
pub mod can;
pub mod can_master;
pub mod can_slave;
pub mod cfg;
pub mod clock;
pub mod coinbase;
pub mod config;
pub mod config_nvs;
pub mod crt_bundle;
pub mod crypto;
pub mod cutil;
pub mod discord;
pub mod display_shot;
pub mod display_text;
pub mod dns;
pub mod ds4432u;
pub mod emc2101;
pub mod emc2302;
pub mod fan_control;
pub mod fxl6408;
pub mod hashrate;
pub mod history;
pub mod http_auth;
pub mod http_can;
pub mod http_coredump;
pub mod http_data;
pub mod http_file;
pub mod http_files;
pub mod http_handlers;
pub mod http_json;
pub mod http_ota;
pub mod http_query;
pub mod http_routes;
pub mod http_server;
pub mod http_settings;
pub mod http_ws;
pub mod i2c;
pub mod idf;
pub mod ina260;
pub mod influx;
pub mod jobs;
mod log;
/// LVGL (the display): firmware only
#[cfg(target_os = "espidf")]
pub mod lvgl;
pub mod mdns;
pub mod mining;
pub mod mining_tasks;
pub mod netmgr;
pub mod network;
pub mod ota_factory;
pub mod ota_factory_task;
pub mod ota_upload;
pub mod otp;
pub mod otp_device;
pub mod ping;
pub mod ping_session;
pub mod pool;
pub mod pool_coinbase;
pub mod pool_dispatch;
pub mod pool_manager;
pub mod pool_stats;
pub mod power_mgmt;
pub mod power_task;
pub mod profiles;
pub mod sdkconfig;
pub mod secp;
pub mod serial;
pub mod settings_apply;
pub mod stratum_config;
pub mod stratum_task;
pub mod stratum_transport;
pub mod stratum_v1;
pub mod stratum_v2;
pub mod suffix;
pub mod system;
pub mod tasks;
pub mod tmp1075;
pub mod tmp451;
pub mod tps536x7;
pub mod tps546;
#[cfg(target_os = "espidf")]
pub mod ui;
pub mod w5500;
pub mod wifi;
pub mod wifi_health;
