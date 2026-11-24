//! The state of the factory-image update (port of the `FactoryOTAUpdate`
//! state in `main/http_server/handler_ota_factory.cpp`): the request (URL,
//! keep-config flag) handed from the HTTP handler to the update task, the
//! step and the progress shown by `GET /api/v2/ota/status`, the status JSON
//! and the redirect policy of the download. The download itself
//! (`esp_http_client`, OTA and partition writes) is `ota/factory_task.rs`.
//!
//! Resetting the progress also resets the received WWW bytes, so a second
//! attempt does not start with the first one's progress.

use core::ffi::c_int;
use std::sync::{Condvar, Mutex};

/// The update steps (`FactoryOTAUpdate::OtaStep`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Step {
    Idle = 0,
    ResolvingUrl,
    UpdatingFw,
    DownloadingWww,
    UpdatingWww,
    ErasingNvs,
    Rebooting,
    Error,
}

impl Step {
    pub fn from_int(v: c_int) -> Option<Step> {
        Some(match v {
            0 => Step::Idle,
            1 => Step::ResolvingUrl,
            2 => Step::UpdatingFw,
            3 => Step::DownloadingWww,
            4 => Step::UpdatingWww,
            5 => Step::ErasingNvs,
            6 => Step::Rebooting,
            7 => Step::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Step::Idle => "idle",
            Step::ResolvingUrl => "resolving_url",
            Step::UpdatingFw => "updating_fw",
            Step::DownloadingWww => "downloading_www",
            Step::UpdatingWww => "updating_www",
            Step::ErasingNvs => "erasing_nvs",
            Step::Rebooting => "rebooting",
            Step::Error => "error",
        }
    }
}

/// Firmware and WWW sizes of the factory image (`FW_LEN_BYTES`,
/// `WWW_LEN_BYTES`).
pub const FW_LEN: u64 = 4 * 1024 * 1024;
pub const WWW_LEN: u64 = 3 * 1024 * 1024;
pub const MAX_REDIRECTS: c_int = 8;

struct State {
    pending: bool,
    running: bool,
    keep_config: bool,
    url: Option<Vec<u8>>,
    fw_written: u64,
    www_written: u64,
    www_recv: u64,
    step: Step,
}

/// The status of `GET /api/v2/ota/status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub pending: bool,
    pub running: bool,
    pub step: Step,
    pub progress: u32,
}

pub struct Updater {
    s: Mutex<State>,
    cv: Condvar,
}

impl Default for Updater {
    fn default() -> Self {
        Self::new()
    }
}

impl Updater {
    pub const fn new() -> Self {
        Self {
            s: Mutex::new(State {
                pending: false,
                running: false,
                keep_config: true,
                url: None,
                fw_written: 0,
                www_written: 0,
                www_recv: 0,
                step: Step::Idle,
            }),
            cv: Condvar::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.s.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A new request; false if empty or an update is pending/running.
    pub fn trigger(&self, url: &[u8], keep_config: bool) -> bool {
        if url.is_empty() {
            return false;
        }
        let mut s = self.lock();
        if s.running || s.pending {
            return false;
        }
        s.url = Some(url.to_vec());
        s.keep_config = keep_config;
        s.pending = true;
        self.cv.notify_one();
        true
    }

    /// Waits for a request and marks it running: (URL, keep config).
    pub fn wait(&self) -> (Vec<u8>, bool) {
        let mut s = self.lock();
        while !s.pending {
            s = self.cv.wait(s).unwrap_or_else(|e| e.into_inner());
        }
        s.pending = false;
        s.running = true;
        (s.url.take().unwrap_or_default(), s.keep_config)
    }

    /// The update ended (failed): a new request is accepted.
    pub fn done(&self) {
        self.lock().running = false;
    }

    pub fn set_step(&self, step: Step) {
        self.lock().step = step;
    }

    pub fn reset_progress(&self) {
        let mut s = self.lock();
        s.fw_written = 0;
        s.www_written = 0;
        s.www_recv = 0;
        s.step = Step::Idle;
    }

    /// `kind`: 0 firmware written, 1 WWW written, 2 WWW received.
    pub fn add(&self, kind: c_int, n: u32) {
        let mut s = self.lock();
        let c = match kind {
            0 => &mut s.fw_written,
            1 => &mut s.www_written,
            _ => &mut s.www_recv,
        };
        *c = c.saturating_add(n as u64);
    }

    pub fn status(&self) -> Status {
        let s = self.lock();
        // the WWW part is received and written: counted twice
        let total = FW_LEN + 2 * WWW_LEN;
        let done = (s.fw_written + s.www_written + s.www_recv).min(total);
        Status {
            pending: s.pending,
            running: s.running,
            step: s.step,
            progress: (done * 100 / total) as u32,
        }
    }

    /// `{"pending":..,"running":..,"step":"..","progress":..}`
    pub fn status_json(&self) -> String {
        let st = self.status();
        format!(
            "{{\"pending\":{},\"running\":{},\"step\":\"{}\",\"progress\":{}}}",
            st.pending,
            st.running,
            st.step.as_str(),
            st.progress
        )
    }
}

/// What to do with a response of the redirect loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub enum Redirect {
    /// a redirect: open the `Location`
    Follow = 0,
    /// 200/206: stream the body
    Body = 1,
    /// a redirect without `Location`
    MissingLocation = 2,
    /// another status
    Unexpected = 3,
}

pub fn redirect(status: c_int, has_location: bool) -> Redirect {
    match status {
        301 | 302 | 303 | 307 | 308 if has_location => Redirect::Follow,
        301 | 302 | 303 | 307 | 308 => Redirect::MissingLocation,
        200 | 206 => Redirect::Body,
        _ => Redirect::Unexpected,
    }
}

static UPDATER: Updater = Updater::new();

/// The firmware's update state.
pub fn updater() -> &'static Updater {
    &UPDATER
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn trigger_wait_done() {
        let u = Arc::new(Updater::new());
        assert!(!u.trigger(b"", true));
        assert!(u.trigger(b"https://github.com/x", false));
        // pending: busy
        assert!(!u.trigger(b"https://github.com/y", true));
        assert!(u.status().pending);
        let (url, keep) = u.wait();
        assert_eq!(
            (url.as_slice(), keep),
            (&b"https://github.com/x"[..], false)
        );
        let st = u.status();
        assert!(st.running && !st.pending);
        // running: busy
        assert!(!u.trigger(b"https://github.com/y", true));
        u.done();
        assert!(u.trigger(b"https://github.com/y", true));
        // a waiting task is woken
        let u2 = u.clone();
        let t = std::thread::spawn(move || {
            let r = u2.wait();
            u2.done();
            r
        });
        assert_eq!(t.join().unwrap(), (b"https://github.com/y".to_vec(), true));
    }

    #[test]
    fn progress_and_json() {
        let u = Updater::new();
        assert_eq!(
            u.status_json(),
            "{\"pending\":false,\"running\":false,\"step\":\"idle\",\"progress\":0}"
        );
        u.set_step(Step::UpdatingFw);
        u.add(0, FW_LEN as u32);
        // 4 of 10 MB
        assert_eq!(u.status().progress, 40);
        u.add(2, WWW_LEN as u32);
        u.add(1, WWW_LEN as u32 / 2);
        assert_eq!(u.status().progress, 85);
        u.add(1, u32::MAX);
        assert_eq!(u.status().progress, 100);
        assert_eq!(
            u.status_json(),
            "{\"pending\":false,\"running\":false,\"step\":\"updating_fw\",\"progress\":100}"
        );
        // everything is reset, the received WWW bytes too
        u.reset_progress();
        assert_eq!(u.status().progress, 0);
        assert_eq!(u.status().step, Step::Idle);
        for v in 0..8 {
            let s = Step::from_int(v).unwrap();
            assert_eq!(s as c_int, v);
        }
        assert_eq!(Step::from_int(8), None);
    }

    #[test]
    fn redirects() {
        for s in [301, 302, 303, 307, 308] {
            assert_eq!(redirect(s, true), Redirect::Follow);
            assert_eq!(redirect(s, false), Redirect::MissingLocation);
        }
        assert_eq!(redirect(200, false), Redirect::Body);
        assert_eq!(redirect(206, true), Redirect::Body);
        for s in [0, 100, 204, 304, 404, 500, -1] {
            assert_eq!(redirect(s, true), Redirect::Unexpected);
        }
    }
}
