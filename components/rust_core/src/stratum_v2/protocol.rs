//! Stratum v2 binary codec: frame header, message builders and parsers.
//! C API: `sv2_protocol.h` (unchanged function names and signatures).
//!
//! Port of `components/stratum_v2/sv2_protocol.c`, verified against it by
//! `tests/sv2_protocol.rs`. Output parameters are only meaningful
//! on success (as the callers use them); the C code wrote some of them before
//! failing, which is not reproduced except for the channel ID of
//! `sv2_parse_new_extended_mining_job()`.

pub const FRAME_HEADER_SIZE: usize = 6;
pub const MAX_MERKLE_BRANCHES: usize = 20;
pub const CHANNEL_MSG_FLAG: u16 = 0x8000;

pub mod msg {
    pub const SETUP_CONNECTION: u8 = 0x00;
    pub const OPEN_STANDARD_MINING_CHANNEL: u8 = 0x10;
    pub const OPEN_EXTENDED_MINING_CHANNEL: u8 = 0x13;
    pub const SUBMIT_SHARES_STANDARD: u8 = 0x1a;
    pub const SUBMIT_SHARES_EXTENDED: u8 = 0x1b;
}

/// The C code builds variable-length payloads in a 512-byte stack buffer.
const PAYLOAD_LIMIT: usize = 512;

/// `sv2_frame_header_t`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameHeader {
    pub extension_type: u16,
    pub msg_type: u8,
    /// 24-bit value
    pub msg_length: u32,
}

pub fn parse_frame_header(d: &[u8; 6]) -> FrameHeader {
    FrameHeader {
        extension_type: u16::from_le_bytes([d[0], d[1]]),
        msg_type: d[2],
        msg_length: u32::from_le_bytes([d[3], d[4], d[5], 0]),
    }
}

pub fn encode_frame_header(extension_type: u16, msg_type: u8, msg_length: u32) -> [u8; 6] {
    let [e0, e1] = extension_type.to_le_bytes();
    let [l0, l1, l2, _] = msg_length.to_le_bytes();
    [e0, e1, msg_type, l0, l1, l2]
}

fn frame(extension_type: u16, msg_type: u8, payload: &[u8]) -> Vec<u8> {
    let mut f = encode_frame_header(extension_type, msg_type, payload.len() as u32).to_vec();
    f.extend_from_slice(payload);
    f
}

/// Payload under construction, with the C code's 512-byte limit for strings.
struct Payload(Vec<u8>);

impl Payload {
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend(v.to_le_bytes());
        self
    }
    /// STR0_255: length byte plus at most 255 bytes. None if it would not fit
    /// into the C code's payload buffer.
    fn str0_255(&mut self, s: &[u8]) -> Option<&mut Self> {
        let s = &s[..s.len().min(255)];
        if PAYLOAD_LIMIT - self.0.len() < 1 + s.len() {
            return None;
        }
        self.0.push(s.len() as u8);
        self.0.extend_from_slice(s);
        Some(self)
    }
}

pub fn setup_connection(
    host: &[u8],
    port: u16,
    vendor: &[u8],
    hw_version: &[u8],
    firmware: &[u8],
    device_id: &[u8],
    flags: u32,
) -> Option<Vec<u8>> {
    let mut p = Payload(vec![0x00]); // protocol: Mining Protocol
    p.u16(2).u16(2).u32(flags); // min/max version
    p.str0_255(host)?.u16(port);
    p.str0_255(vendor)?
        .str0_255(hw_version)?
        .str0_255(firmware)?
        .str0_255(device_id)?;
    Some(frame(0, msg::SETUP_CONNECTION, &p.0))
}

fn open_channel_payload(request_id: u32, user_identity: &[u8], nominal_hash_rate: f32) -> Payload {
    let mut p = Payload(Vec::new());
    p.u32(request_id);
    // A 255-byte string always fits after 4 bytes.
    p.str0_255(user_identity).expect("fits");
    p.u32(nominal_hash_rate.to_bits());
    p.0.extend([0xff; 32]); // max_target: accept any difficulty
    p
}

pub fn open_standard_mining_channel(
    request_id: u32,
    user_identity: &[u8],
    nominal_hash_rate: f32,
) -> Vec<u8> {
    let p = open_channel_payload(request_id, user_identity, nominal_hash_rate);
    frame(0, msg::OPEN_STANDARD_MINING_CHANNEL, &p.0)
}

pub fn open_extended_mining_channel(
    request_id: u32,
    user_identity: &[u8],
    nominal_hash_rate: f32,
    min_extranonce_size: u16,
) -> Vec<u8> {
    let mut p = open_channel_payload(request_id, user_identity, nominal_hash_rate);
    p.u16(min_extranonce_size);
    frame(0, msg::OPEN_EXTENDED_MINING_CHANNEL, &p.0)
}

fn submit_payload(
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
) -> Payload {
    let mut p = Payload(Vec::with_capacity(64));
    p.u32(channel_id)
        .u32(sequence_number)
        .u32(job_id)
        .u32(nonce)
        .u32(ntime)
        .u32(version);
    p
}

pub fn submit_shares_standard(
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
) -> Vec<u8> {
    let p = submit_payload(channel_id, sequence_number, job_id, nonce, ntime, version);
    frame(CHANNEL_MSG_FLAG, msg::SUBMIT_SHARES_STANDARD, &p.0)
}

#[allow(clippy::too_many_arguments)]
pub fn submit_shares_extended(
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
    extranonce: &[u8],
) -> Vec<u8> {
    let mut p = submit_payload(channel_id, sequence_number, job_id, nonce, ntime, version);
    // B0_32 in the spec; like the C code, the length is not limited to 32.
    p.0.push(extranonce.len() as u8);
    p.0.extend_from_slice(extranonce);
    frame(CHANNEL_MSG_FLAG, msg::SUBMIT_SHARES_EXTENDED, &p.0)
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(d: &'a [u8]) -> Self {
        Self { d, pos: 0 }
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let b = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(b)
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.bytes(N).map(|b| b.try_into().unwrap())
    }
    fn u8(&mut self) -> Option<u8> {
        self.array::<1>().map(|[b]| b)
    }
    fn u16(&mut self) -> Option<u16> {
        self.array().map(u16::from_le_bytes)
    }
    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }
    /// B0_32: length byte (at most 32) plus data.
    fn b0_32(&mut self) -> Option<&'a [u8]> {
        let n = self.u8()? as usize;
        if n > 32 {
            return None;
        }
        self.bytes(n)
    }
}

pub fn setup_connection_success(p: &[u8]) -> Option<(u16, u32)> {
    let mut r = Reader::new(p);
    Some((r.u16()?, r.u32()?))
}

#[derive(Debug, PartialEq, Eq)]
pub struct OpenChannelSuccess<'a> {
    pub request_id: u32,
    pub channel_id: u32,
    pub target: [u8; 32],
    pub extranonce_size: u16,
    pub extranonce_prefix: &'a [u8],
    pub group_channel_id: u32,
}

/// OpenStandardMiningChannel.Success (`extended` false) or
/// OpenExtendedMiningChannel.Success (`extended` true, with extranonce_size).
pub fn open_channel_success(p: &[u8], extended: bool) -> Option<OpenChannelSuccess<'_>> {
    // The C code checked a minimum length first: 45 resp. 47 bytes.
    if p.len() < if extended { 47 } else { 45 } {
        return None;
    }
    let mut r = Reader::new(p);
    let request_id = r.u32()?;
    let channel_id = r.u32()?;
    let target = r.array()?;
    let extranonce_size = if extended { r.u16()? } else { 0 };
    let extranonce_prefix = r.b0_32()?;
    let group_channel_id = r.u32()?;
    Some(OpenChannelSuccess {
        request_id,
        channel_id,
        target,
        extranonce_size,
        extranonce_prefix,
        group_channel_id,
    })
}

#[derive(Debug, PartialEq, Eq)]
pub struct NewMiningJob {
    pub channel_id: u32,
    pub job_id: u32,
    /// None: a future job
    pub min_ntime: Option<u32>,
    pub version: u32,
    pub merkle_root: [u8; 32],
}

pub fn new_mining_job(p: &[u8]) -> Option<NewMiningJob> {
    if p.len() < 45 {
        return None;
    }
    let mut r = Reader::new(p);
    let channel_id = r.u32()?;
    let job_id = r.u32()?;
    // Sv2Option<u32>: flag byte 1 means a value follows; like the C code,
    // any other flag value means none.
    let min_ntime = if r.u8()? == 1 { Some(r.u32()?) } else { None };
    Some(NewMiningJob {
        channel_id,
        job_id,
        min_ntime,
        version: r.u32()?,
        merkle_root: r.array()?,
    })
}

/// (channel_id, job_id, prev_hash, min_ntime, nbits)
pub fn set_new_prev_hash(p: &[u8]) -> Option<(u32, u32, [u8; 32], u32, u32)> {
    let mut r = Reader::new(p);
    Some((r.u32()?, r.u32()?, r.array()?, r.u32()?, r.u32()?))
}

pub fn set_extranonce_prefix(p: &[u8]) -> Option<(u32, &[u8])> {
    let mut r = Reader::new(p);
    Some((r.u32()?, r.b0_32()?))
}

pub fn set_target(p: &[u8]) -> Option<(u32, [u8; 32])> {
    let mut r = Reader::new(p);
    Some((r.u32()?, r.array()?))
}

/// (channel_id, new_submits_accepted_count)
pub fn submit_shares_success(p: &[u8]) -> Option<(u32, u32)> {
    // channel_id, last_sequence_number, new_submits_accepted_count, new_shares_sum(u64)
    let mut r = Reader::new(p);
    let channel_id = r.u32()?;
    r.u32()?;
    let accepted = r.u32()?;
    r.bytes(8)?;
    Some((channel_id, accepted))
}

/// (channel_id, sequence_number, error_code)
pub fn submit_shares_error(p: &[u8]) -> Option<(u32, u32, &[u8])> {
    let mut r = Reader::new(p);
    let channel_id = r.u32()?;
    let seq = r.u32()?;
    let n = r.u8()? as usize;
    Some((channel_id, seq, r.bytes(n)?))
}

/// NewExtendedMiningJob, parsed (not yet the C struct).
#[derive(Debug, PartialEq, Eq)]
pub struct NewExtendedJob<'a> {
    pub channel_id: u32,
    pub job_id: u32,
    pub min_ntime: Option<u32>,
    pub version: u32,
    pub version_rolling_allowed: bool,
    pub merkle_path: Vec<[u8; 32]>,
    pub coinbase_prefix: &'a [u8],
    pub coinbase_suffix: &'a [u8],
}

/// Ok(job), or Err(channel_id if it could be read) like the C code, which
/// reported the channel ID even for a job it then rejected.
pub fn new_extended_mining_job(p: &[u8]) -> Result<NewExtendedJob<'_>, Option<u32>> {
    if p.len() < 19 {
        return Err(None);
    }
    let mut r = Reader::new(p);
    let channel_id = r.u32().ok_or(None)?;
    let fail = Err(Some(channel_id));
    let Some(job_id) = r.u32() else { return fail };
    let parsed = (|| {
        let min_ntime = if r.u8()? == 1 { Some(r.u32()?) } else { None };
        let version = r.u32()?;
        let version_rolling_allowed = r.u8()? != 0;
        let count = r.u8()? as usize;
        if count > MAX_MERKLE_BRANCHES {
            return None;
        }
        let merkle_path = (0..count).map(|_| r.array()).collect::<Option<Vec<_>>>()?;
        let n = r.u16()? as usize;
        let coinbase_prefix = r.bytes(n)?;
        let n = r.u16()? as usize;
        let coinbase_suffix = r.bytes(n)?;
        Some((
            min_ntime,
            version,
            version_rolling_allowed,
            merkle_path,
            coinbase_prefix,
            coinbase_suffix,
        ))
    })();
    let Some((
        min_ntime,
        version,
        version_rolling_allowed,
        merkle_path,
        coinbase_prefix,
        coinbase_suffix,
    )) = parsed
    else {
        return fail;
    };
    Ok(NewExtendedJob {
        channel_id,
        job_id,
        min_ntime,
        version,
        version_rolling_allowed,
        merkle_path,
        coinbase_prefix,
        coinbase_suffix,
    })
}

/// Pool difficulty of a U256 LE target, clamped to 1..=u32::MAX.
pub fn target_to_pdiff(target: &[u8; 32]) -> u32 {
    let t = crate::mining::le256_to_f64(target);
    if t == 0.0 {
        return u32::MAX;
    }
    let pdiff = crate::mining::TRUEDIFFONE / t;
    if pdiff > u32::MAX as f64 {
        u32::MAX
    } else if pdiff < 1.0 {
        1
    } else {
        pdiff as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_header_round_trip() {
        let h = encode_frame_header(0x8000, 0x1b, 0x12_3456);
        assert_eq!(h, [0x00, 0x80, 0x1b, 0x56, 0x34, 0x12]);
        assert_eq!(
            parse_frame_header(&h),
            FrameHeader {
                extension_type: 0x8000,
                msg_type: 0x1b,
                msg_length: 0x12_3456
            }
        );
    }

    #[test]
    fn builders() {
        let f = submit_shares_standard(1, 2, 3, 4, 5, 6);
        assert_eq!(f.len(), 6 + 24);
        assert_eq!(&f[..6], &[0x00, 0x80, 0x1a, 24, 0, 0]);
        let f = setup_connection(b"pool", 3336, b"v", b"h", b"f", b"d", 4).unwrap();
        assert_eq!(&f[6..15], &[0, 2, 0, 2, 0, 4, 0, 0, 0]);
        assert_eq!(&f[15..20], b"\x04pool");
        let long = [b'x'; 300];
        assert_eq!(setup_connection(&long, 1, &long, &long, b"", b"", 0), None);
    }

    #[test]
    fn parsers_reject_short_or_bad_payloads() {
        assert_eq!(new_mining_job(&[0; 44]), None);
        let mut job = vec![0u8; 45];
        job[8] = 1; // min_ntime present, but only 45 bytes
        assert_eq!(new_mining_job(&job), None);
        let mut prefix = vec![0u8; 5];
        prefix[4] = 33;
        assert_eq!(set_extranonce_prefix(&prefix), None);
        assert_eq!(
            new_extended_mining_job(&[7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0, 0, 0]),
            Err(Some(7))
        );
    }

    #[test]
    fn pdiff() {
        let mut diff1 = [0u8; 32];
        diff1[26..28].copy_from_slice(&[0xff, 0xff]); // 0x00000000ffff0000...
        assert_eq!(target_to_pdiff(&diff1), 1);
        assert_eq!(target_to_pdiff(&[0; 32]), u32::MAX);
        assert_eq!(target_to_pdiff(&[0xff; 32]), 1);
    }
}
