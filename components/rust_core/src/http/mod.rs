//! The HTTP server: routes, authentication, the handlers and the JSON
//! documents of the REST API, the web UI files, OTA uploads and the
//! websocket log.

pub mod auth;
pub mod can;
pub mod coredump;
pub mod data;
pub mod file;
pub mod files;
pub mod handlers;
pub mod json;
pub mod ota;
pub mod query;
pub mod routes;
pub mod server;
pub mod settings;
pub mod ws;
