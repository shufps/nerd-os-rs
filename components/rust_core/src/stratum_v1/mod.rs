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

/// `mining.notify`. The strings are the bytes before the first NUL (what C
/// code saw of them).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MiningNotify {
    pub job_id: Vec<u8>,
    pub prev_block_hash: [u8; HASH_SIZE],
    pub coinbase_1: Vec<u8>,
    pub coinbase_2: Vec<u8>,
    /// At most [`MAX_MERKLE_BRANCHES`]
    pub merkle_branches: Vec<[u8; HASH_SIZE]>,
    pub version: u32,
    pub target: u32,
    pub ntime: u32,
}

/// A parsed pool message (`StratumApiV1Message`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    /// extranonce1 (hex) of a subscribe result or `mining.set_extranonce`
    pub extranonce_str: Option<Vec<u8>>,
    pub extranonce_2_len: i32,
    pub message_id: i64,
    /// One of [`method`]
    pub method: i32,
    pub should_abandon_work: bool,
    /// Set by a successful `mining.notify` parse
    pub mining_notification: Option<MiningNotify>,
    pub new_difficulty: u32,
    pub version_mask: u32,
    pub response_success: bool,
}
