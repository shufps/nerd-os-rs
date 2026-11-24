//! The HTTP server: routes, authentication, the REST API (`v1/`, `v2/`)
//! with its shared documents, bodies and data sources, the web UI files,
//! OTA uploads and the websocket log.

pub mod auth;
pub mod coredump;
pub mod file;
pub mod files;
pub mod json;
pub mod ota;
pub mod query;
pub mod reply;
pub mod routes;
pub mod server;
pub mod settings;
pub mod sources;
pub mod v1;
pub mod v2;
pub mod ws;

/// The handler of a v1 or v2 API route, if it is one.
pub fn handler(h: routes::Handler) -> Option<crate::idf::httpd::HandlerFn> {
    v1::handler(h).or_else(|| v2::handler(h))
}
