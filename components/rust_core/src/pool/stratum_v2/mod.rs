//! Stratum v2: the binary codec ([`protocol`]), the Noise transport
//! ([`noise`]), the session state ([`session`]) and job construction
//! ([`jobs`]).

pub mod authority;
pub mod connection;
pub mod jobs;
pub mod noise;
pub mod protocol;
pub mod session;

use protocol::{FrameHeader, MAX_MERKLE_BRANCHES};

/// `sv2_ext_job_t`: allocated by `sv2_parse_new_extended_mining_job()`,
/// released with `sv2_ext_job_free()`. The C side fills in the extranonce
/// prefix and may read every field.
#[repr(C)]
pub struct ExtJob {
    pub job_id: u32,
    pub version: u32,
    pub version_rolling_allowed: bool,
    pub prev_hash: [u8; 32],
    pub ntime: u32,
    pub nbits: u32,
    pub clean_jobs: bool,
    pub merkle_path: [[u8; 32]; MAX_MERKLE_BRANCHES],
    pub merkle_path_count: u8,
    pub coinbase_prefix: *mut u8,
    pub coinbase_prefix_len: u16,
    pub coinbase_suffix: *mut u8,
    pub coinbase_suffix_len: u16,
    pub extranonce_prefix: [u8; 32],
    pub extranonce_prefix_len: u8,
}

// Must match the static assertions in sv2_protocol.h.
#[cfg(target_pointer_width = "32")]
const _: () = assert!(core::mem::size_of::<ExtJob>() == 744);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<ExtJob>() == 760);
const _: () = assert!(core::mem::size_of::<FrameHeader>() == 8);

extern "C" {}

pub(crate) mod api {

    // --- frame header ---

    // --- builders ---

    // --- parsers: 0 on success, -1 on error ---
}
