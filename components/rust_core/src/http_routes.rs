//! The HTTP routes (port of the registrations in
//! `main/http_server/http_server.cpp` and of the access checks at the top of
//! every handler): URI, method, handler, and what runs before the handler:
//! `Connection: close` (closed after the response), the network check (401),
//! the CORS headers (500 if they cannot be set) and the OTP check (required,
//! forced, or skipped in recovery mode). The server (`http_server.rs`)
//! registers the routes in this order (the first match of a wildcard wins)
//! with one trampoline that carries out the checks and then calls the
//! handler.
//!
//! Checks that depend on the request stay in their handlers: the WiFi scan
//! sets CORS only after its access point check, the statistics reset needs
//! OTP only for `allTime`. Restart and shutdown deliberately have no OTP
//! check (home automation pauses mining with them).

use core::ffi::CStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Method {
    Get = 0,
    Post = 1,
    Patch = 2,
    Delete = 3,
    Options = 4,
}

/// The handlers (`HTTP_H_*`), indices into the C++ handler table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Handler {
    Recovery = 0,
    Options,
    SystemInfo,
    SystemAsic,
    InfluxGet,
    CanNodes,
    CanSlaveAction,
    CanSlavePatch,
    CanSlaveDelete,
    V2Dashboard,
    V2SettingsGet,
    V2SettingsPatch,
    V2Identify,
    V2System,
    Coredump,
    Restart,
    WifiScan,
    ResetStats,
    Shutdown,
    SettingsPatch,
    InfluxPatch,
    OtpPatch,
    OtpCreate,
    OtpSession,
    OtpStatus,
    AlertGet,
    AlertPatch,
    AlertTest,
    Ota,
    OtaWww,
    Websocket,
    OtaGithubPost,
    OtaGithubGet,
    File,
    ProfilesGet,
    ProfilesPatch,
    ProfileApply,
    /// GET /api/debug/screen (development)
    #[cfg(feature = "screenshot")]
    Screenshot,
    /// POST /api/debug/press?btn=1|2 (development)
    #[cfg(feature = "screenshot")]
    Press,
}

/// What runs before the handler, and when the route exists.
pub mod flag {
    /// `Connection: close`, the session closed after the handler
    pub const CLOSE: u16 = 1 << 0;
    /// the network check (401 "Unauthorized")
    pub const NET: u16 = 1 << 1;
    /// the CORS headers (500 if they cannot be set)
    pub const CORS: u16 = 1 << 2;
    /// `validateOTP()`
    pub const OTP: u16 = 1 << 3;
    /// `validateOTP(force)`: also with OTP disabled, no session token
    pub const OTP_FORCE: u16 = 1 << 4;
    /// `validateOTP()` except in recovery mode
    pub const OTP_UNLESS_RECOVERY: u16 = 1 << 5;
    /// a websocket route
    pub const WEBSOCKET: u16 = 1 << 6;
    /// the handler gets the server context (scratch buffer)
    pub const CTX: u16 = 1 << 7;
    /// only with core dumps in flash
    pub const COREDUMP_ONLY: u16 = 1 << 8;
    /// only in recovery mode
    pub const RECOVERY_ONLY: u16 = 1 << 9;
    /// only outside recovery mode
    pub const NORMAL_ONLY: u16 = 1 << 10;
}

/// The routes of optional development features, registered before
/// [`ROUTES`] (not part of the table the reference test compares).
#[cfg(feature = "screenshot")]
pub static EXTRA: [Route; 2] = [
    r(
        c"/api/debug/screen",
        Method::Get,
        Handler::Screenshot,
        API | OTP,
    ),
    r(c"/api/debug/press", Method::Post, Handler::Press, API | OTP),
];
#[cfg(not(feature = "screenshot"))]
pub static EXTRA: [Route; 0] = [];

/// One route of the table.
#[derive(Clone, Copy, Debug)]
pub struct Route {
    pub uri: &'static CStr,
    pub method: Method,
    pub handler: Handler,
    pub flags: u16,
}

impl Route {
    pub fn uri(&self) -> &'static str {
        self.uri.to_str().unwrap_or("")
    }
}

use flag::*;
use Handler as H;
use Method::*;

const fn r(uri: &'static CStr, method: Method, handler: Handler, flags: u16) -> Route {
    Route {
        uri,
        method,
        handler,
        flags,
    }
}

/// An API endpoint: closed after the response, network check, CORS.
const API: u16 = CLOSE | NET | CORS | CTX;
/// Its CORS preflight (no server context).
const PREFLIGHT: u16 = CLOSE | NET | CORS;

/// The routes in registration order.
pub static ROUTES: [Route; 50] = [
    r(c"/recovery", Get, H::Recovery, CLOSE | NET | CTX),
    r(c"/api/system/info", Get, H::SystemInfo, API),
    r(c"/api/system/asic", Get, H::SystemAsic, API),
    r(c"/api/v2/influx", Get, H::InfluxGet, API),
    r(c"/api/v2/can/nodes", Get, H::CanNodes, API),
    r(c"/api/v2/can/nodes", Options, H::Options, PREFLIGHT),
    // one wildcard per method: the handlers parse id and action
    r(c"/api/v2/can/nodes/*", Post, H::CanSlaveAction, API | OTP),
    r(c"/api/v2/can/nodes/*", Patch, H::CanSlavePatch, API | OTP),
    r(c"/api/v2/can/nodes/*", Delete, H::CanSlaveDelete, API | OTP),
    r(c"/api/v2/can/nodes/*", Options, H::Options, PREFLIGHT),
    r(c"/api/v2/dashboard", Get, H::V2Dashboard, API),
    r(c"/api/v2/dashboard", Options, H::Options, PREFLIGHT),
    r(c"/api/v2/settings", Get, H::V2SettingsGet, API),
    r(c"/api/v2/settings", Patch, H::V2SettingsPatch, API | OTP),
    r(c"/api/v2/settings", Options, H::Options, PREFLIGHT),
    r(c"/api/v2/identify", Get, H::V2Identify, API),
    r(c"/api/v2/identify", Options, H::Options, PREFLIGHT),
    r(c"/api/v2/system", Get, H::V2System, API),
    r(c"/api/v2/system", Options, H::Options, PREFLIGHT),
    // core dumps can contain credentials and other sensitive task RAM
    r(
        c"/api/system/coredump",
        Get,
        H::Coredump,
        API | OTP | COREDUMP_ONLY,
    ),
    r(
        c"/api/system/coredump",
        Options,
        H::Options,
        PREFLIGHT | COREDUMP_ONLY,
    ),
    // no OTP on purpose (home automation)
    r(c"/api/system/restart", Post, H::Restart, CLOSE | NET | CTX),
    // CORS after the access point check, in the handler
    r(c"/api/v2/wifi/scan", Get, H::WifiScan, CLOSE | NET | CTX),
    r(c"/api/system/restart", Options, H::Options, PREFLIGHT),
    // OTP only for allTime, in the handler
    r(
        c"/api/system/reset-stats",
        Post,
        H::ResetStats,
        CLOSE | NET | CTX,
    ),
    // no OTP on purpose (home automation)
    r(
        c"/api/system/shutdown",
        Post,
        H::Shutdown,
        CLOSE | NET | CTX,
    ),
    r(c"/api/system/shutdown", Options, H::Options, PREFLIGHT),
    r(c"/api/system", Patch, H::SettingsPatch, API | OTP),
    r(c"/api/v2/influx", Patch, H::InfluxPatch, API | OTP),
    r(c"/api/system", Options, H::Options, PREFLIGHT),
    r(c"/api/v2/otp", Patch, H::OtpPatch, API | OTP_FORCE),
    r(c"/api/v2/otp", Post, H::OtpCreate, API),
    r(c"/api/v2/otp/session", Post, H::OtpSession, API),
    r(c"/api/v2/otp/status", Get, H::OtpStatus, API),
    r(c"/api/v2/alert", Get, H::AlertGet, API),
    r(c"/api/v2/alert", Patch, H::AlertPatch, API | OTP),
    r(c"/api/v2/alert/test", Post, H::AlertTest, API | OTP),
    r(c"/api/v2/profiles", Get, H::ProfilesGet, API),
    r(c"/api/v2/profiles", Patch, H::ProfilesPatch, API | OTP),
    r(c"/api/v2/profiles", Options, H::Options, PREFLIGHT),
    // no OTP on purpose: only selects a stored profile (home automation)
    r(c"/api/v2/profiles/apply", Post, H::ProfileApply, API),
    r(c"/api/v2/profiles/apply", Options, H::Options, PREFLIGHT),
    r(c"/api/system/OTA", Post, H::Ota, CLOSE | NET | OTP),
    // the web UI can be reinstalled from the recovery page without OTP
    r(
        c"/api/system/OTAWWW",
        Post,
        H::OtaWww,
        CLOSE | NET | OTP_UNLESS_RECOVERY,
    ),
    // kept open: no Connection: close
    r(c"/api/ws", Get, H::Websocket, NET | WEBSOCKET),
    r(
        c"/api/system/OTA/github",
        Post,
        H::OtaGithubPost,
        CLOSE | NET | OTP,
    ),
    r(c"/api/system/OTA/github", Get, H::OtaGithubGet, CLOSE | NET),
    r(c"/api/system/OTA/github", Options, H::Options, PREFLIGHT),
    // everything else: the recovery page or the web UI files (public)
    r(c"/*", Get, H::Recovery, CLOSE | NET | CTX | RECOVERY_ONLY),
    r(c"/*", Get, H::File, CLOSE | CTX | NORMAL_ONLY),
];

/// Whether the route is registered.
pub fn active(route: &Route, recovery: bool, coredump: bool) -> bool {
    let f = route.flags;
    !(f & COREDUMP_ONLY != 0 && !coredump
        || f & RECOVERY_ONLY != 0 && !recovery
        || f & NORMAL_ONLY != 0 && recovery)
}

/// Whether the route checks OTP before its handler.
pub fn needs_otp(route: &Route, recovery: bool) -> bool {
    let f = route.flags;
    f & (OTP | OTP_FORCE) != 0 || f & OTP_UNLESS_RECOVERY != 0 && !recovery
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Everything that changes the device needs OTP, apart from the
    /// deliberate exceptions.
    #[test]
    fn changes_need_otp() {
        for rt in ROUTES.iter() {
            let changes = rt.method != Get && rt.method != Options;
            let exempt = matches!(
                rt.handler,
                // home automation; the handler checks allTime; enrollment
                // and login are how OTP is set up and used
                H::Restart
                    | H::Shutdown
                    | H::ProfileApply
                    | H::ResetStats
                    | H::OtpCreate
                    | H::OtpSession
            );
            if changes && !exempt {
                assert!(needs_otp(rt, false), "{} {:?}", rt.uri(), rt.method);
            }
        }
        // the web UI upload works without OTP in recovery mode only
        let www = ROUTES.iter().find(|r| r.handler == H::OtaWww).unwrap();
        assert!(needs_otp(www, false) && !needs_otp(www, true));
    }

    #[test]
    fn api_routes_check_the_network() {
        for rt in ROUTES.iter() {
            if rt.uri().starts_with("/api/") {
                assert!(rt.flags & NET != 0, "{}", rt.uri());
            }
        }
    }

    #[test]
    fn one_catch_all_per_mode() {
        for recovery in [false, true] {
            let n = ROUTES
                .iter()
                .filter(|r| r.uri() == "/*" && active(r, recovery, false))
                .count();
            assert_eq!(n, 1);
        }
        // each (uri, method) at most once in a mode
        for (i, a) in ROUTES.iter().enumerate() {
            for b in &ROUTES[i + 1..] {
                if a.uri() == b.uri() && a.method == b.method {
                    assert!(a.flags & (RECOVERY_ONLY | NORMAL_ONLY) != 0, "{}", a.uri());
                }
            }
        }
    }
}
