//! The Stratum v2 connection loop (port of `StratumTaskV2::protocolLoop()`
//! with its setup steps): SetupConnection, OpenChannel, then frames into the
//! session until the connection ends. Frames go through [`FrameIo`] (the
//! Noise transport on the device); the task, the manager and the session's
//! events through [`Hooks`].
//!
//! Between frames the loop waits for data in short steps and checks for
//! stop and reconnect requests in between, instead of blocking in the read.
//! A connection without any frame for 3 minutes is dropped.

use std::sync::Mutex;

use super::protocol::{self as p, FrameHeader};
use super::session::Session;
use crate::log::log;

const TAG: &core::ffi::CStr = c"stratum_v2";

/// Largest frame payload accepted (the former receive buffer).
pub const MAX_RECV_SIZE: usize = 16384;
/// SetupConnection.flags: the ASICs always roll version bits.
const SETUP_FLAGS_REQUIRES_VERSION_ROLLING: u32 = 1 << 2;
const SETUP_FLAGS_REQUIRES_STANDARD_JOBS: u32 = 1 << 0;
/// What OpenChannel announces; the pool sets the real difficulty.
const NOMINAL_HASH_RATE: f32 = 1e12;
const MIN_EXTRANONCE_SIZE: u16 = 6;
const REQUEST_ID: u32 = 1;

/// Waiting for the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Readiness {
    Readable,
    Timeout,
    Error,
}

/// Step for waiting between frames.
pub const POLL_MS: u32 = 1000;
/// A connection that sends nothing for this long is dropped (the former
/// read timeout).
pub const IDLE_LIMIT_MS: u32 = 3 * 60 * 1000;

pub trait FrameIo {
    /// Waits up to `ms` for data of the next frame.
    fn wait_readable(&mut self, ms: u32) -> Readiness;
    /// Sends a complete frame (header and payload).
    fn send(&mut self, frame: &[u8]) -> bool;
    /// Receives the next frame's payload into `buf`; None on any failure.
    fn recv(&mut self, buf: &mut [u8]) -> Option<FrameHeader>;
}

pub trait Hooks {
    /// stop flag, reconnect request or power shutdown
    fn exit_requested(&mut self) -> bool;
    /// the channel is open: the connection works
    fn connected(&mut self);
    /// hand the session's events to the manager and the job pipeline
    fn drain_events(&mut self);
    /// an event made the connection unusable (e.g. no version rolling)
    fn setup_failed(&mut self) -> bool;
}

pub struct Setup<'a> {
    pub host: &'a [u8],
    pub port: u16,
    pub vendor: &'a [u8],
    pub hw_version: &'a [u8],
    pub user: &'a [u8],
    pub extended: bool,
}

/// Why the connection loop ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    SetupFailed,
    OpenChannelFailed,
    ExitRequested,
    ReceiveFailed,
    ConnectionUnusable,
}

fn exchange(
    io: &mut impl FrameIo,
    buf: &mut [u8],
    frame: Option<Vec<u8>>,
    what: &str,
) -> Option<(u8, usize)> {
    let Some(frame) = frame else {
        log!(Error, TAG, "Failed to build {what} frame");
        return None;
    };
    if !io.send(&frame) {
        log!(Error, TAG, "Failed to send {what}");
        return None;
    }
    match io.recv(buf) {
        Some(h) => Some((h.msg_type, h.msg_length as usize)),
        None => {
            log!(Error, TAG, "Failed to receive {what} answer");
            None
        }
    }
}

pub fn run(
    io: &mut impl FrameIo,
    hooks: &mut impl Hooks,
    setup: &Setup,
    session: &Mutex<Session>,
) -> Exit {
    let lock = || session.lock().unwrap_or_else(|e| e.into_inner());
    *lock() = Session::new(setup.extended);
    let channel = if setup.extended {
        "extended"
    } else {
        "standard"
    };
    log!(Info, TAG, "SV2 protocol loop starting (channel={channel})");

    let mut buf = vec![0u8; MAX_RECV_SIZE];

    // 1. SetupConnection
    let mut flags = SETUP_FLAGS_REQUIRES_VERSION_ROLLING;
    if !setup.extended {
        flags |= SETUP_FLAGS_REQUIRES_STANDARD_JOBS;
    }
    log!(
        Info,
        TAG,
        "Sending SetupConnection (vendor={}, hw={}, channel={channel})",
        String::from_utf8_lossy(setup.vendor),
        String::from_utf8_lossy(setup.hw_version)
    );
    let frame = p::setup_connection(
        setup.host,
        setup.port,
        setup.vendor,
        setup.hw_version,
        b"",
        b"",
        flags,
    );
    let ok = exchange(io, &mut buf, frame, "SetupConnection")
        .is_some_and(|(msg_type, len)| lock().setup_success(msg_type, &buf[..len]));
    if !ok {
        log!(Error, TAG, "SV2 SetupConnection failed");
        return Exit::SetupFailed;
    }

    // 2. OpenChannel
    let user = String::from_utf8_lossy(setup.user);
    log!(
        Info,
        TAG,
        "Opening {channel} mining channel (user={})",
        if user.is_empty() { "(empty)" } else { &user }
    );
    let frame = if setup.extended {
        p::open_extended_mining_channel(
            REQUEST_ID,
            setup.user,
            NOMINAL_HASH_RATE,
            MIN_EXTRANONCE_SIZE,
        )
    } else {
        p::open_standard_mining_channel(REQUEST_ID, setup.user, NOMINAL_HASH_RATE)
    };
    let ok = exchange(io, &mut buf, Some(frame), "OpenChannel")
        .is_some_and(|(msg_type, len)| lock().open_channel_success(msg_type, &buf[..len]));
    hooks.drain_events();
    if !ok {
        log!(Error, TAG, "SV2 OpenChannel failed");
        return Exit::OpenChannelFailed;
    }

    hooks.connected();
    log!(Info, TAG, "SV2+Noise connection ready, waiting for jobs");

    // 3. frames until the connection ends
    let mut idle_ms = 0;
    loop {
        if hooks.exit_requested() {
            log!(Info, TAG, "SV2 loop exit requested");
            return Exit::ExitRequested;
        }
        // Wait for the next frame in short steps; a frame is
        // only read once it has started arriving, so a read is never
        // abandoned halfway through an encrypted frame.
        match io.wait_readable(POLL_MS) {
            Readiness::Readable => idle_ms = 0,
            Readiness::Timeout => {
                idle_ms += POLL_MS;
                if idle_ms >= IDLE_LIMIT_MS {
                    log!(
                        Error,
                        TAG,
                        "No SV2 frame for {} s, reconnecting...",
                        IDLE_LIMIT_MS / 1000
                    );
                    return Exit::ReceiveFailed;
                }
                continue;
            }
            Readiness::Error => {
                log!(Error, TAG, "SV2 connection error, reconnecting...");
                return Exit::ReceiveFailed;
            }
        }
        let Some(header) = io.recv(&mut buf) else {
            log!(Error, TAG, "Failed to receive SV2 frame, reconnecting...");
            return Exit::ReceiveFailed;
        };
        let len = (header.msg_length as usize).min(buf.len());
        lock().handle(header.msg_type, &buf[..len]);
        hooks.drain_events();
        if hooks.setup_failed() {
            return Exit::ConnectionUnusable;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Plain frames instead of Noise: `incoming` are (msg_type, payload).
    #[derive(Default)]
    struct FakeIo {
        sent: Vec<Vec<u8>>,
        incoming: VecDeque<(u8, Vec<u8>)>,
    }

    impl FrameIo for FakeIo {
        fn wait_readable(&mut self, _ms: u32) -> Readiness {
            if self.incoming.is_empty() {
                Readiness::Timeout
            } else {
                Readiness::Readable
            }
        }
        fn send(&mut self, frame: &[u8]) -> bool {
            self.sent.push(frame.to_vec());
            true
        }
        fn recv(&mut self, buf: &mut [u8]) -> Option<FrameHeader> {
            let (msg_type, payload) = self.incoming.pop_front()?;
            buf[..payload.len()].copy_from_slice(&payload);
            Some(FrameHeader {
                extension_type: 0,
                msg_type,
                msg_length: payload.len() as u32,
            })
        }
    }

    #[derive(Default)]
    struct FakeHooks {
        connected: usize,
        drains: usize,
        exit_after_frames: Option<usize>,
        frames: usize,
    }

    impl Hooks for FakeHooks {
        fn exit_requested(&mut self) -> bool {
            self.frames += 1;
            self.exit_after_frames.is_some_and(|n| self.frames > n)
        }
        fn connected(&mut self) {
            self.connected += 1;
        }
        fn drain_events(&mut self) {
            self.drains += 1;
        }
        fn setup_failed(&mut self) -> bool {
            false
        }
    }

    const SETUP: Setup<'static> = Setup {
        host: b"pool",
        port: 3336,
        vendor: b"NerdQAxe+",
        hw_version: b"BM1370",
        user: b"u",
        extended: true,
    };

    fn setup_success() -> (u8, Vec<u8>) {
        (
            0x01,
            [&2u16.to_le_bytes()[..], &0u32.to_le_bytes()].concat(),
        )
    }

    fn open_success(extranonce_size: u16) -> (u8, Vec<u8>) {
        let mut m = Vec::new();
        m.extend(1u32.to_le_bytes());
        m.extend(7u32.to_le_bytes());
        m.extend([0xff; 32]);
        m.extend(extranonce_size.to_le_bytes());
        m.push(0);
        m.extend(0u32.to_le_bytes());
        (0x14, m)
    }

    #[test]
    fn setup_then_frames() {
        let mut io = FakeIo::default();
        io.incoming
            .extend([setup_success(), open_success(8), (0x21, vec![0; 36])]);
        let mut hooks = FakeHooks::default();
        let session = Mutex::new(Session::new(true));
        assert_eq!(
            run(&mut io, &mut hooks, &SETUP, &session),
            Exit::ReceiveFailed
        );
        assert_eq!(io.sent.len(), 2);
        assert_eq!(io.sent[0][2], 0x00); // SetupConnection
        assert_eq!(io.sent[1][2], 0x13); // OpenExtendedMiningChannel
        assert_eq!(&io.sent[1][io.sent[1].len() - 2..], &6u16.to_le_bytes()); // min extranonce size
        assert_eq!(hooks.connected, 1);
        assert_eq!(hooks.drains, 2); // after OpenChannel, after the SetTarget frame
    }

    #[test]
    fn setup_failures() {
        let session = Mutex::new(Session::new(true));
        // SetupConnection.Error
        let mut io = FakeIo::default();
        io.incoming.push_back((0x02, vec![]));
        assert_eq!(
            run(&mut io, &mut FakeHooks::default(), &SETUP, &session),
            Exit::SetupFailed
        );
        // unusable extranonce size
        let mut io = FakeIo::default();
        io.incoming.extend([setup_success(), open_success(64)]);
        let mut hooks = FakeHooks::default();
        assert_eq!(
            run(&mut io, &mut hooks, &SETUP, &session),
            Exit::OpenChannelFailed
        );
        assert_eq!(hooks.connected, 0);
    }

    /// An exit request is noticed while the pool is quiet, and a
    /// quiet connection is still dropped after the idle limit.
    #[test]
    fn quiet_pool() {
        let session = Mutex::new(Session::new(true));
        let mut io = FakeIo::default();
        io.incoming.extend([setup_success(), open_success(8)]);
        let mut hooks = FakeHooks {
            exit_after_frames: Some(5),
            ..Default::default()
        };
        assert_eq!(
            run(&mut io, &mut hooks, &SETUP, &session),
            Exit::ExitRequested
        );
        assert_eq!(hooks.frames, 6);

        let mut io = FakeIo::default();
        io.incoming.extend([setup_success(), open_success(8)]);
        let mut hooks = FakeHooks::default();
        assert_eq!(
            run(&mut io, &mut hooks, &SETUP, &session),
            Exit::ReceiveFailed
        );
        assert_eq!(hooks.frames as u32, IDLE_LIMIT_MS / POLL_MS);
    }

    #[test]
    fn exit_request() {
        let mut io = FakeIo::default();
        io.incoming.extend([setup_success(), open_success(8)]);
        let mut hooks = FakeHooks {
            exit_after_frames: Some(0),
            ..Default::default()
        };
        let session = Mutex::new(Session::new(true));
        assert_eq!(
            run(&mut io, &mut hooks, &SETUP, &session),
            Exit::ExitRequested
        );
    }
}
