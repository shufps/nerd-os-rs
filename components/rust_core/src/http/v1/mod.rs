//! The v1 API (`/api/system/...`).

pub mod json;
pub mod settings;
pub mod system;

use crate::http::routes::Handler;
use crate::idf::httpd::HandlerFn;

/// The handler of `h`, if it is a v1 one.
pub fn handler(h: Handler) -> Option<HandlerFn> {
    Some(match h {
        Handler::SystemInfo => system::info,
        Handler::SystemAsic => system::asic,
        Handler::ResetStats => system::reset_stats,
        Handler::SettingsPatch => system::patch,
        Handler::Restart => system::restart,
        Handler::Shutdown => system::shutdown,
        _ => return None,
    })
}
