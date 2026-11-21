//! Request/response bookkeeping of a Stratum v1 connection: message IDs and
//! which request each pending ID belongs to, so responses are classified by
//! what was actually sent.
//!
//! Responses to pending IDs are classified by their request (without
//! `mining.extranonce.subscribe`, the first share gets ID 5, inside the
//! fixed setup range). IDs that are not pending (lost or unexpected
//! responses) follow the fixed rule: 1..=5 are setup requests, everything
//! above a share result.

use std::collections::VecDeque;

use super::{parse, request, Message};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Subscribe,
    Configure,
    Authorize,
    SuggestDifficulty,
    ExtranonceSubscribe,
    Submit,
}

impl Request {
    /// The former fixed classification of a response ID.
    pub(super) fn legacy(id: i64) -> Option<Request> {
        Some(match id {
            1 => Request::Subscribe,
            2 => Request::Configure,
            3 => Request::Authorize,
            4 => Request::SuggestDifficulty,
            5 => Request::ExtranonceSubscribe,
            6.. => Request::Submit,
            _ => return None,
        })
    }
}

/// Pending requests kept for matching; older ones fall back to the fixed rule.
pub const MAX_PENDING: usize = 64;

pub struct Session {
    next_id: i32,
    pending: VecDeque<(i64, Request)>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            next_id: 1,
            pending: VecDeque::new(),
        }
    }
}

impl Session {
    /// Starts over for a new connection: IDs from 1, nothing pending.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    fn track(&mut self, kind: Request) -> i32 {
        let id = self.next_id;
        self.next_id = self.next_id.checked_add(1).unwrap_or(1);
        if self.pending.len() == MAX_PENDING {
            self.pending.pop_front();
        }
        self.pending.push_back((id as i64, kind));
        id
    }

    /// Undoes the last `track()` (for a request that was not sent). A request
    /// dropped by the pending limit stays dropped; it falls back to the fixed
    /// rule then.
    pub(crate) fn untrack_last(&mut self) {
        if let Some((id, _)) = self.pending.pop_back() {
            self.next_id = id as i32;
        }
    }

    fn take(&mut self, id: i64) -> Option<Request> {
        let i = self.pending.iter().position(|&(p, _)| p == id)?;
        self.pending.remove(i).map(|(_, kind)| kind)
    }

    pub fn subscribe(&mut self, device: &[u8], asic: &[u8], version: &[u8]) -> Vec<u8> {
        request::subscribe(self.track(Request::Subscribe), device, asic, version)
    }

    pub fn configure_version_rolling(&mut self) -> Vec<u8> {
        request::configure_version_rolling(self.track(Request::Configure))
    }

    pub fn authorize(&mut self, user: &[u8], password: &[u8]) -> Vec<u8> {
        request::authorize(self.track(Request::Authorize), user, password)
    }

    pub fn suggest_difficulty(&mut self, difficulty: u32) -> Vec<u8> {
        request::suggest_difficulty(self.track(Request::SuggestDifficulty), difficulty)
    }

    pub fn extranonce_subscribe(&mut self) -> Vec<u8> {
        request::extranonce_subscribe(self.track(Request::ExtranonceSubscribe))
    }

    /// `version` is the rolled version delta, as `mining.submit` expects.
    pub fn submit(
        &mut self,
        user: &[u8],
        job_id: &[u8],
        extranonce2: &[u8],
        ntime: u32,
        nonce: u32,
        version: u32,
    ) -> Vec<u8> {
        let id = self.track(Request::Submit);
        request::submit(id, user, job_id, extranonce2, ntime, nonce, version)
    }

    /// Parses a line; a response is classified by the request it answers.
    pub fn parse(&mut self, line: &[u8], message: &mut Message) -> bool {
        parse::parse_with(line, message, &mut |id| {
            self.take(id).or_else(|| Request::legacy(id))
        })
    }
}

/// The log lines of the former `StratumApi::parse()`.
pub(crate) fn log_parsed(ok: bool, m: &Message) {
    use super::method::*;
    use crate::log::log;
    use core::ffi::CStr;
    const TAG: &CStr = c"stratum_api";
    match m.method {
        MINING_NOTIFY => log!(Info, TAG, "mining notify"),
        MINING_SET_EXTRANONCE | STRATUM_RESULT_SUBSCRIBE if ok => {
            let en = String::from_utf8_lossy(m.extranonce_str.as_deref().unwrap_or_default());
            log!(Info, TAG, "extranonce_str: {en}");
            log!(Info, TAG, "extranonce_2_len: {}", m.extranonce_2_len);
        }
        MINING_SET_EXTRANONCE | STRATUM_RESULT_SUBSCRIBE => log!(
            Error,
            TAG,
            "Invalid extranonce (extranonce_2_len {})",
            m.extranonce_2_len
        ),
        STRATUM_RESULT_VERSION_MASK if ok => {
            log!(Info, TAG, "Set version mask: {:08x}", m.version_mask)
        }
        STRATUM_UNKNOWN => log!(Warn, TAG, "Unhandled stratum message"),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::super::method;
    use super::*;

    fn parse_method(s: &mut Session, json: &str) -> (bool, i32) {
        let mut m = Message::default();
        let ok = s.parse(json.as_bytes(), &mut m);
        assert!(m.mining_notification.is_none() && m.extranonce_str.is_none());
        (ok, m.method)
    }

    #[test]
    fn ids_and_matching() {
        let mut s = Session::default();
        assert!(String::from_utf8(s.subscribe(b"d", b"a", b"v"))
            .unwrap()
            .starts_with("{\"id\": 1,"));
        s.configure_version_rolling();
        s.authorize(b"u", b"p");
        s.suggest_difficulty(1000);
        // no extranonce.subscribe: the first share gets ID 5
        assert!(String::from_utf8(s.submit(b"u", b"j", b"00", 1, 2, 3))
            .unwrap()
            .starts_with("{\"id\": 5,"));

        assert_eq!(
            parse_method(&mut s, r#"{"id":3,"result":true,"error":null}"#),
            (true, method::STRATUM_RESULT_SETUP)
        );
        // ID 5 is pending as a share here, so its result is a share result
        assert_eq!(
            parse_method(&mut s, r#"{"id":5,"result":true,"error":null}"#),
            (true, method::STRATUM_RESULT)
        );
        // a second answer to ID 5 is unexpected: fixed rule
        assert_eq!(
            parse_method(&mut s, r#"{"id":5,"result":true,"error":null}"#),
            (true, method::STRATUM_RESULT_SETUP)
        );
        // unknown IDs: fixed rule
        assert_eq!(
            parse_method(&mut s, r#"{"id":77,"result":false,"error":null}"#),
            (true, method::STRATUM_RESULT)
        );
        assert_eq!(
            parse_method(&mut s, r#"{"id":0,"result":true}"#),
            (false, method::STRATUM_UNKNOWN)
        );
    }

    #[test]
    fn reset_and_pending_limit() {
        let mut s = Session::default();
        for _ in 0..MAX_PENDING + 10 {
            s.submit(b"u", b"j", b"00", 0, 0, 0);
        }
        assert_eq!(s.pending.len(), MAX_PENDING);
        // the oldest submits (IDs 1..=10) were dropped: fixed rule
        assert_eq!(
            parse_method(&mut s, r#"{"id":2,"result":true}"#).1,
            method::STRATUM_RESULT_VERSION_MASK
        );
        assert_eq!(
            parse_method(&mut s, r#"{"id":11,"result":true}"#).1,
            method::STRATUM_RESULT
        );

        s.reset();
        assert!(s.pending.is_empty());
        assert!(String::from_utf8(s.extranonce_subscribe())
            .unwrap()
            .starts_with("{\"id\": 1,"));
    }

    /// The connection task and the submitting task share one session.
    #[test]
    fn concurrent_submits_get_unique_ids() {
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};
        let session = Arc::new(Mutex::new(Session::default()));
        let threads: Vec<_> = (0..4)
            .map(|_| {
                let session = session.clone();
                std::thread::spawn(move || {
                    let mut ids = Vec::new();
                    for _ in 0..500 {
                        let line = session.lock().unwrap().submit(b"u", b"j", b"00", 0, 0, 0);
                        let v: serde_json::Value = serde_json::from_slice(&line).unwrap();
                        ids.push(v["id"].as_i64().unwrap());
                    }
                    ids
                })
            })
            .collect();
        let ids: Vec<i64> = threads
            .into_iter()
            .flat_map(|t| t.join().unwrap())
            .collect();
        assert_eq!(ids.iter().collect::<HashSet<_>>().len(), 2000);
    }
}
