//! The v2 API (`/api/v2/...`): one module per resource with its handlers,
//! documents and request bodies.

pub mod alert;
pub mod can;
pub mod dashboard;
pub mod influx;
pub mod otp;
pub mod settings;
pub mod system;
pub mod wifi;

use crate::http::routes::Handler;
use crate::idf::httpd::HandlerFn;

/// The handler of `h`, if it is a v2 one.
pub fn handler(h: Handler) -> Option<HandlerFn> {
    Some(match h {
        Handler::V2Dashboard => dashboard::get,
        Handler::V2SettingsGet => settings::get,
        Handler::V2SettingsPatch => settings::patch,
        Handler::V2Identify => system::identify,
        Handler::V2System => system::system,
        Handler::InfluxGet => influx::get,
        Handler::InfluxPatch => influx::patch,
        Handler::AlertGet => alert::get,
        Handler::AlertPatch => alert::patch,
        Handler::AlertTest => alert::test,
        Handler::OtpCreate => otp::create,
        Handler::OtpPatch => otp::patch,
        Handler::OtpStatus => otp::status,
        Handler::OtpSession => otp::session,
        Handler::CanNodes => can::nodes,
        Handler::CanSlaveAction => can::action,
        Handler::CanSlavePatch => can::patch,
        Handler::CanSlaveDelete => can::delete,
        Handler::WifiScan => wifi::scan,
        _ => return None,
    })
}
