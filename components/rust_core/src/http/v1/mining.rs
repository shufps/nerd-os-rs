//! `POST /api/system/pause` and `POST /api/system/resume`: the
//! request goes to the power task, which switches the chain within a few
//! seconds (`mining/pause.rs`); the answer is the state at the time
//! (`miningState` in `GET /api/v2/dashboard` follows the change). No OTP
//! on purpose (home automation), as restart and shutdown.

use crate::http::json::Obj;
use crate::http::reply::{send_doc, set_json};
use crate::idf::httpd::Request;
use crate::idf::EspErr;
use crate::log::log;
use crate::mining::pause::{self, Refused, Request as Change};

fn change(req: &mut Request, c: Change) -> EspErr {
    let r = pause::request(c);
    log!(
        Info,
        c"http_mining",
        "{c:?} requested by the API: {}",
        match r {
            Ok(()) => "ok",
            Err(Refused::Unavailable) => "refused (no mining)",
            Err(Refused::Busy) => "refused (busy)",
        }
    );
    if r.is_err() {
        req.set_status(c"409 Conflict");
    }
    set_json(req);
    let mut d = Obj::new();
    d.bool("ok", r.is_ok())
        .cstr("miningState", Some(pause::state().name()));
    send_doc(req, d.into())
}

pub(crate) fn pause(req: &mut Request) -> EspErr {
    change(req, Change::Pause)
}

pub(crate) fn resume(req: &mut Request) -> EspErr {
    change(req, Change::Resume)
}
