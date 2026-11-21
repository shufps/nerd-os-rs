//! Stratum v1: parsing of pool messages, request formatting, line framing and
//! job construction; the connection loop is `connection.rs`.
//!
//! Port of the codec part of `main/stratum/stratum_api.cpp`, verified against
//! it by `tests/stratum_v1.rs`. Input validation:
//!
//! * malformed messages (missing or mistyped fields) are rejected,
//! * `extranonce2_size` must be 1..=16,
//! * requests escape `"`, `\` and control characters in strings, and are
//!   rejected instead of truncated if they do not fit the buffer,
//! * JSON syntax is strict (ArduinoJson also accepted single quotes, unquoted
//!   keys and trailing garbage); value conversions follow ArduinoJson.

pub mod connection;
pub mod job;
pub mod lines;
pub mod parse;
pub mod request;
pub mod session;

use core::ffi::{c_char, c_int};

pub const MAX_MERKLE_BRANCHES: usize = 32;
pub const HASH_SIZE: usize = 32;

/// `stratum_method`
pub mod method {
    pub const STRATUM_UNKNOWN: i32 = 0;
    pub const MINING_NOTIFY: i32 = 1;
    pub const MINING_SET_DIFFICULTY: i32 = 2;
    pub const MINING_SET_VERSION_MASK: i32 = 3;
    pub const MINING_SET_EXTRANONCE: i32 = 4;
    pub const STRATUM_RESULT: i32 = 5;
    pub const STRATUM_RESULT_SETUP: i32 = 6;
    pub const STRATUM_RESULT_VERSION_MASK: i32 = 7;
    pub const STRATUM_RESULT_SUBSCRIBE: i32 = 8;
    pub const CLIENT_RECONNECT: i32 = 9;
}

/// Accepted `extranonce2_size` range
pub const EXTRANONCE2_LEN: core::ops::RangeInclusive<i32> = 1..=16;

/// `mining_notify`; strings are allocated with `malloc()`-compatible memory
/// and freed by the C side.
#[repr(C)]
pub struct MiningNotify {
    pub job_id: *mut c_char,
    pub prev_block_hash: [u8; HASH_SIZE],
    pub coinbase_1: *mut c_char,
    pub coinbase_2: *mut c_char,
    pub merkle_branches: [[u8; HASH_SIZE]; MAX_MERKLE_BRANCHES],
    pub n_merkle_branches: usize,
    pub version: u32,
    pub version_mask: u32,
    pub target: u32,
    pub ntime: u32,
    pub difficulty: u32,
}

/// `StratumApiV1Message`
#[repr(C)]
pub struct Message {
    pub extranonce_str: *mut c_char,
    pub extranonce_2_len: c_int,
    pub message_id: i64,
    /// One of [`method`]
    pub method: i32,
    pub should_abandon_work: c_int,
    pub mining_notification: *mut MiningNotify,
    pub new_difficulty: u32,
    pub version_mask: u32,
    pub response_success: bool,
}

// Must match the static assertions in stratum_v1.h.
#[cfg(target_pointer_width = "32")]
const _: () = {
    assert!(core::mem::size_of::<MiningNotify>() == 1092);
    assert!(core::mem::size_of::<Message>() == 40);
    assert!(core::mem::offset_of!(Message, response_success) == 36);
};
#[cfg(target_pointer_width = "64")]
const _: () = {
    assert!(core::mem::size_of::<MiningNotify>() == 1112);
    assert!(core::mem::size_of::<Message>() == 56);
    assert!(core::mem::offset_of!(Message, response_success) == 48);
};

impl Message {
    pub const fn zeroed() -> Self {
        Self {
            extranonce_str: core::ptr::null_mut(),
            extranonce_2_len: 0,
            message_id: 0,
            method: method::STRATUM_UNKNOWN,
            should_abandon_work: 0,
            mining_notification: core::ptr::null_mut(),
            new_difficulty: 0,
            version_mask: 0,
            response_success: false,
        }
    }
}

pub(crate) mod ffi {
    use super::*;

    /// Frees what `stratum_v1_session_parse()` allocated in `message` and
    /// clears the pointers (port of `StratumManager::freeStratumV1Message()`).
    ///
    /// # Safety
    /// `message` is NULL or a message filled by `stratum_v1_session_parse()`.
    pub unsafe extern "C" fn stratum_v1_message_free(message: *mut Message) {
        use crate::cutil::c_free;
        let Some(m) = message.as_mut() else {
            return;
        };
        if let Some(n) = m.mining_notification.as_mut() {
            c_free(n.job_id as _);
            c_free(n.coinbase_1 as _);
            c_free(n.coinbase_2 as _);
        }
        c_free(m.mining_notification as _);
        c_free(m.extranonce_str as _);
        m.mining_notification = core::ptr::null_mut();
        m.extranonce_str = core::ptr::null_mut();
    }
}
