//! Coinbase transaction decoder.
//!
//! Extracts block height, scriptsig (miner tag) and transaction outputs from
//! the coinbase data of a stratum `mining.notify`.
//!
//! This is a port of the former C component `coinbase_decoder`. It reproduces
//! the C behaviour exactly (verified by `tests/coinbase.rs`)
//! for well-formed input. Malformed input is handled as follows:
//!
//! * a coinbase_1 that ends inside the block height push is rejected with
//!   `ESP_ERR_INVALID_ARG`,
//! * an output script length that runs past the end of the data ends output
//!   parsing.

use core::ffi::{c_char, c_int, CStr};

use crate::cutil::hex2bin;

pub const MAX_SCRIPTSIG_LEN: usize = 256;

const BIP110_SIGNAL_BIT: u32 = 4;
const BIP110_SIGNAL_EXPIRY_BLOCK: u32 = 965_664;

// Bitcoin script opcodes
const OP_0: u8 = 0x00;
const OP_PUSHDATA_20: u8 = 0x14;
const OP_PUSHDATA_32: u8 = 0x20;
const OP_1: u8 = 0x51;
const OP_DUP: u8 = 0x76;
const OP_EQUAL: u8 = 0x87;
const OP_EQUALVERIFY: u8 = 0x88;
const OP_HASH160: u8 = 0xa9;
const OP_CHECKSIG: u8 = 0xac;

/// `esp_err_t`
pub type EspErr = i32;
pub const ESP_OK: EspErr = 0;
pub const ESP_ERR_NO_MEM: EspErr = 0x101;
pub const ESP_ERR_INVALID_ARG: EspErr = 0x102;

/// `coinbase_result_t`
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CoinbaseResult {
    pub network_difficulty: f64,
    pub block_height: u32,
    /// NUL-terminated, non-printable characters replaced by '.'
    pub scriptsig: [c_char; MAX_SCRIPTSIG_LEN],
    pub total_value_satoshis: u64,
    pub user_value_satoshis: u64,
    pub bip54_signaling: bool,
    pub bip110_signaling: bool,
}

// Must match the static assertions in coinbase_decoder.h.
const _: () = assert!(core::mem::size_of::<CoinbaseResult>() == 296);
const _: () = assert!(core::mem::align_of::<CoinbaseResult>() == 8);
const _: () = assert!(core::mem::offset_of!(CoinbaseResult, total_value_satoshis) == 272);

impl CoinbaseResult {
    pub const fn zeroed() -> Self {
        Self {
            network_difficulty: 0.0,
            block_height: 0,
            scriptsig: [0; MAX_SCRIPTSIG_LEN],
            total_value_satoshis: 0,
            user_value_satoshis: 0,
            bip54_signaling: false,
            bip110_signaling: false,
        }
    }

    /// The scriptsig up to its NUL terminator.
    pub fn scriptsig_bytes(&self) -> &[u8] {
        // SAFETY: c_char and u8 have the same size and alignment.
        let bytes: &[u8; MAX_SCRIPTSIG_LEN] =
            unsafe { &*(&self.scriptsig as *const _ as *const _) };
        let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
        &bytes[..end]
    }
}

impl Default for CoinbaseResult {
    fn default() -> Self {
        Self::zeroed()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidArg,
    NoMem,
}

impl Error {
    pub const fn code(self) -> EspErr {
        match self {
            Error::InvalidArg => ESP_ERR_INVALID_ARG,
            Error::NoMem => ESP_ERR_NO_MEM,
        }
    }
}

/// Network difficulty from nBits.
pub fn network_difficulty(nbits: u32) -> f64 {
    let mantissa = nbits & 0x007f_ffff;
    let exponent = (nbits >> 24) as i32;
    // powf maps to libm pow(), so the result is bit-identical to the C version.
    let target = mantissa as f64 * 256f64.powf((exponent - 3) as f64);
    (2f64.powf(208.0) * 65535.0) / target
}

/// `hex` starting at byte offset `byte_offset` (empty if past the end).
fn hex_from(hex: &[u8], byte_offset: usize) -> &[u8] {
    hex.get(byte_offset * 2..).unwrap_or(&[])
}

fn read_le<const N: usize>(data: &[u8], offset: usize) -> Option<[u8; N]> {
    data.get(offset..offset.checked_add(N)?)?.try_into().ok()
}

/// Bitcoin varint. Like the C version, returns 0 without advancing if no byte
/// is left, and returns 0 after consuming only the prefix byte if the value is
/// truncated.
fn decode_varint(data: &[u8], offset: &mut usize) -> u64 {
    let Some(&first) = data.get(*offset) else {
        return 0;
    };
    *offset += 1;
    let value = match first {
        0xfd => read_le::<2>(data, *offset).map(|b| u16::from_le_bytes(b) as u64),
        0xfe => read_le::<4>(data, *offset).map(|b| u32::from_le_bytes(b) as u64),
        0xff => read_le::<8>(data, *offset).map(u64::from_le_bytes),
        _ => return first as u64,
    };
    match value {
        Some(v) => {
            *offset += match first {
                0xfd => 2,
                0xfe => 4,
                _ => 8,
            };
            v
        }
        None => 0,
    }
}

/// The scriptPubKey a mainnet address pays to (P2PKH, P2SH, P2WPKH, P2WSH,
/// P2TR). Coinbase outputs are matched against these raw bytes, which is
/// case-correct for Base58 addresses, unlike comparing address strings.
///
/// Like the C version, the Base58Check checksum is not verified.
fn address_to_script_pubkey(addr: &[u8]) -> Option<Vec<u8>> {
    if addr.is_empty() {
        return None;
    }

    // SegWit (bech32/bech32m) with mainnet HRP "bc"; anything else falls
    // through to Base58, as in the C version.
    if let Some((_, version, program)) = core::str::from_utf8(addr)
        .ok()
        .and_then(|s| bech32::segwit::decode(s).ok())
        .filter(|(hrp, _, _)| *hrp == bech32::hrp::BC)
    {
        let prefix = match (version.to_u8(), program.len()) {
            (0, 20) => [OP_0, OP_PUSHDATA_20],
            (0, 32) => [OP_0, OP_PUSHDATA_32],
            (1, 32) => [OP_1, OP_PUSHDATA_32],
            _ => return None,
        };
        return Some([&prefix[..], &program].concat());
    }

    // Base58Check: [version][hash160(20)][checksum(4)]
    let raw = bs58::decode(addr)
        .into_vec()
        .ok()
        .filter(|raw| raw.len() == 25)?;
    let hash160 = &raw[1..21];
    match raw[0] {
        0x00 => Some(
            [
                &[OP_DUP, OP_HASH160, OP_PUSHDATA_20][..],
                hash160,
                &[OP_EQUALVERIFY, OP_CHECKSIG],
            ]
            .concat(),
        ),
        0x05 => Some([&[OP_HASH160, OP_PUSHDATA_20][..], hash160, &[OP_EQUAL]].concat()),
        _ => None,
    }
}

/// Decodes the coinbase of a `mining.notify` into `result`.
///
/// `coinbase_1`, `coinbase_2` and `extranonce1` are hex strings. As in the C
/// version, `result` is reset first and keeps whatever was decoded before an
/// error; running out of data in coinbase_2 is not an error.
#[allow(clippy::too_many_arguments)]
pub fn process(
    coinbase_1: &[u8],
    coinbase_2: &[u8],
    block_version: u32,
    nbits: u32,
    extranonce1: &[u8],
    extranonce2_len: i32,
    user_address: Option<&[u8]>,
    result: &mut CoinbaseResult,
) -> Result<(), Error> {
    *result = CoinbaseResult::zeroed();

    // 1. Difficulty
    result.network_difficulty = network_difficulty(nbits);

    // 2. coinbase_1: block height and scriptsig.
    // Skip version(4) + input count(1) + prevhash(32) + vout(4).
    let cb1_len = coinbase_1.len() / 2;
    let mut offset = 41;

    let mut byte = [0u8; 1];
    if cb1_len < offset + 1 {
        return Err(Error::InvalidArg);
    }
    hex2bin(hex_from(coinbase_1, offset), &mut byte);
    let scriptsig_len = byte[0] as usize;
    offset += 1;

    if cb1_len < offset + 1 {
        return Err(Error::InvalidArg);
    }
    hex2bin(hex_from(coinbase_1, offset), &mut byte);
    let height_len = byte[0] as usize;
    offset += 1;

    if !(1..=4).contains(&height_len) || cb1_len < offset + height_len {
        return Err(Error::InvalidArg);
    }
    let mut height = [0u8; 4];
    hex2bin(hex_from(coinbase_1, offset), &mut height[..height_len]);
    result.block_height = u32::from_le_bytes(height);
    offset += height_len;

    result.bip110_signaling = result.block_height < BIP110_SIGNAL_EXPIRY_BLOCK
        && block_version & (1 << BIP110_SIGNAL_BIT) != 0;

    // Remaining scriptsig length (without the block height push)
    if scriptsig_len < 1 + height_len {
        return Err(Error::InvalidArg);
    }
    // The C code does this arithmetic in int/size_t; wrapping i32 reproduces it.
    let tag_total = (scriptsig_len - 1 - height_len) as i32;
    let cb1_rest = (cb1_len - offset) as i32;
    let extranonces = ((extranonce1.len() / 2) as i32).wrapping_add(extranonce2_len);

    // If the scriptsig extends into coinbase_2, it covers the extranonces.
    let mut tag_len = tag_total;
    if cb1_rest < tag_len {
        tag_len = tag_len.wrapping_sub(extranonces);
    }

    // Miner tag
    if tag_len > 0 && (tag_len as usize) < MAX_SCRIPTSIG_LEN {
        let tag_len = tag_len as usize;
        let mut tag = [0u8; MAX_SCRIPTSIG_LEN];
        let tag1_len = tag_len.min(cb1_rest as usize);
        hex2bin(hex_from(coinbase_1, offset), &mut tag[..tag1_len]);

        let tag2_len = tag_len - tag1_len;
        if tag2_len > 0 && coinbase_2.len() / 2 >= tag2_len {
            hex2bin(coinbase_2, &mut tag[tag1_len..tag_len]);
        }

        for (dst, &b) in result.scriptsig.iter_mut().zip(&tag[..tag_len]) {
            *dst = if (0x20..=0x7e).contains(&b) { b } else { b'.' } as c_char;
        }
    }

    // 3. coinbase_2: outputs
    let mut cb2_offset = 0;
    let remainder = tag_total - cb1_rest;
    if remainder > 0 {
        let in_cb2 = remainder.wrapping_sub(extranonces);
        if in_cb2 > 0 {
            cb2_offset = in_cb2 as usize;
        }
    }

    let cb2_len = coinbase_2.len() / 2;
    let mut cb2 = Vec::new();
    cb2.try_reserve_exact(cb2_len).map_err(|_| Error::NoMem)?;
    cb2.resize(cb2_len, 0);
    hex2bin(coinbase_2, &mut cb2);

    let mut offset = cb2_offset;

    // nSequence, for BIP-54 detection
    let Some(sequence) = read_le::<4>(&cb2, offset).map(u32::from_le_bytes) else {
        return Ok(()); // partial success: got block height and scriptsig
    };
    offset += 4;

    if offset >= cb2_len {
        return Ok(());
    }
    let num_outputs = decode_varint(&cb2, &mut offset);

    let user_spk = user_address.and_then(address_to_script_pubkey);

    // Sum all outputs, and those paying to the user's address
    let mut i = 0;
    while i < num_outputs && offset < cb2_len {
        let Some(value) = read_le::<8>(&cb2, offset).map(u64::from_le_bytes) else {
            break;
        };
        offset += 8;
        result.total_value_satoshis = result.total_value_satoshis.wrapping_add(value);

        if offset >= cb2_len {
            break;
        }
        let script_len = decode_varint(&cb2, &mut offset);
        let Some(script) = usize::try_from(script_len)
            .ok()
            .and_then(|len| offset.checked_add(len))
            .and_then(|end| cb2.get(offset..end))
        else {
            break;
        };

        if value > 0 && user_spk.as_deref() == Some(script) {
            result.user_value_satoshis = result.user_value_satoshis.wrapping_add(value);
        }

        offset += script.len();
        i += 1;
    }

    // nLockTime, for BIP-54 detection
    let lock_time = read_le::<4>(&cb2, offset).map_or(0, u32::from_le_bytes);

    result.bip54_signaling =
        result.block_height > 0 && lock_time == result.block_height - 1 && sequence != 0xffff_ffff;

    Ok(())
}

/// The decoder with the former C signature (the golden tests use it).
///
/// # Safety
/// The string arguments must be NULL or NUL-terminated, `result` must be NULL
/// or point to a writable `coinbase_result_t`.
pub unsafe extern "C" fn coinbase_process(
    coinbase_1: *const c_char,
    coinbase_2: *const c_char,
    block_version: u32,
    nbits: u32,
    extranonce1: *const c_char,
    extranonce2_len: c_int,
    user_address: *const c_char,
    result: *mut CoinbaseResult,
) -> EspErr {
    if coinbase_1.is_null() || coinbase_2.is_null() || extranonce1.is_null() || result.is_null() {
        return ESP_ERR_INVALID_ARG;
    }
    let cstr = |p: *const c_char| CStr::from_ptr(p).to_bytes();
    let user_address = (!user_address.is_null()).then(|| cstr(user_address));

    match process(
        cstr(coinbase_1),
        cstr(coinbase_2),
        block_version,
        nbits,
        cstr(extranonce1),
        extranonce2_len,
        user_address,
        &mut *result,
    ) {
        Ok(()) => ESP_OK,
        Err(e) => e.code(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
    }

    fn unhex(s: &str) -> Vec<u8> {
        let mut out = vec![0; s.len() / 2];
        hex2bin(s.as_bytes(), &mut out);
        out
    }

    /// Builds coinbase_1/coinbase_2 hex as a pool would send them.
    struct Coinbase {
        height: u32,
        tag: Vec<u8>,
        extranonce1: Vec<u8>,
        extranonce2_len: usize,
        sequence: u32,
        outputs: Vec<(u64, Vec<u8>)>,
        lock_time: u32,
    }

    impl Coinbase {
        fn new(height: u32, tag: &[u8]) -> Self {
            Self {
                height,
                tag: tag.to_vec(),
                extranonce1: vec![0xaa; 4],
                extranonce2_len: 4,
                sequence: 0xffff_ffff,
                outputs: Vec::new(),
                lock_time: 0,
            }
        }

        /// (coinbase_1, coinbase_2), split at the extranonces after the tag.
        fn build(&self) -> (String, String) {
            let height = self.height.to_le_bytes();
            let height_len = 4 - height.iter().rev().take_while(|&&b| b == 0).count().min(3);
            let scriptsig_len =
                1 + height_len + self.tag.len() + self.extranonce1.len() + self.extranonce2_len;

            let mut cb1 = vec![1, 0, 0, 0, 1];
            cb1.extend([0; 32]);
            cb1.extend([0xff; 4]);
            cb1.push(scriptsig_len as u8);
            cb1.push(height_len as u8);
            cb1.extend(&height[..height_len]);
            cb1.extend(&self.tag);

            let mut cb2 = self.sequence.to_le_bytes().to_vec();
            cb2.push(self.outputs.len() as u8);
            for (value, spk) in &self.outputs {
                cb2.extend(value.to_le_bytes());
                cb2.push(spk.len() as u8);
                cb2.extend(spk);
            }
            cb2.extend(self.lock_time.to_le_bytes());
            (hex(&cb1), hex(&cb2))
        }

        fn process(&self, user_address: Option<&str>) -> (Result<(), Error>, CoinbaseResult) {
            let (cb1, cb2) = self.build();
            let mut result = CoinbaseResult::zeroed();
            let r = process(
                cb1.as_bytes(),
                cb2.as_bytes(),
                0x2000_0000,
                0x1703_4219,
                hex(&self.extranonce1).as_bytes(),
                self.extranonce2_len as i32,
                user_address.map(str::as_bytes),
                &mut result,
            );
            (r, result)
        }
    }

    // Test vectors from BIP-173 / BIP-350 and the genesis block.
    const P2PKH: (&str, &str) = (
        "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
        "76a91462e907b15cbf27d5425399ebf6f0fb50ebb88f1888ac",
    );
    const P2WPKH: (&str, &str) = (
        "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
        "0014751e76e8199196d454941c45d1b3a323f1433bd6",
    );
    const P2WSH: (&str, &str) = (
        "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3",
        "00201863143c14c5166804bd19203356da136c985678cd4d27a1b8c6329604903262",
    );
    const P2TR: (&str, &str) = (
        "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0",
        "512079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    );

    #[test]
    fn addresses_to_script_pubkey() {
        for (addr, spk) in [P2PKH, P2WPKH, P2WSH, P2TR] {
            assert_eq!(
                address_to_script_pubkey(addr.as_bytes()),
                Some(unhex(spk)),
                "{addr}"
            );
        }
        let p2sh = address_to_script_pubkey(b"3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy").unwrap();
        assert_eq!(
            (p2sh.len(), p2sh[..2].to_vec(), p2sh[22]),
            (23, vec![OP_HASH160, OP_PUSHDATA_20], OP_EQUAL)
        );

        // testnet, mixed case, wrong witness program length, garbage
        for addr in [
            "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
            "bc1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
            "bc1zw508d6qejxtdg4y5r3zarvaryvaxxpcs",
            "not an address",
            "",
        ] {
            assert_eq!(address_to_script_pubkey(addr.as_bytes()), None, "{addr}");
        }
    }

    #[test]
    fn decodes_height_tag_and_outputs() {
        let mut cb = Coinbase::new(870_000, b"/NerdMiner/\x01");
        cb.outputs = vec![
            (300_000_000, unhex(P2WPKH.1)),
            (12_500_000, unhex(P2TR.1)),
            (0, vec![0x6a, 0x24, 0xaa, 0x21, 0xa9, 0xed]),
            (1_000, unhex(P2WPKH.1)),
        ];
        let (r, res) = cb.process(Some(P2WPKH.0));
        assert_eq!(r, Ok(()));
        assert_eq!(res.block_height, 870_000);
        assert_eq!(res.scriptsig_bytes(), b"/NerdMiner/.");
        assert_eq!(res.total_value_satoshis, 312_501_000);
        assert_eq!(res.user_value_satoshis, 300_001_000);
        assert!(!res.bip54_signaling);
        assert!(!res.bip110_signaling);
        assert_eq!(res.network_difficulty, network_difficulty(0x1703_4219));
    }

    #[test]
    fn detects_bip54() {
        let mut cb = Coinbase::new(900_000, b"tag");
        cb.sequence = 0xffff_fffe;
        cb.lock_time = 899_999;
        assert!(cb.process(None).1.bip54_signaling);
        cb.sequence = 0xffff_ffff;
        assert!(!cb.process(None).1.bip54_signaling);
    }

    #[test]
    fn network_difficulty_matches_known_value() {
        // Difficulty 1 target
        assert_eq!(network_difficulty(0x1d00_ffff), 1.0);
    }

    #[test]
    fn rejects_truncated_block_height() {
        let (cb1, cb2) = Coinbase::new(870_000, b"").build();
        let mut result = CoinbaseResult::zeroed();
        let cut = &cb1.as_bytes()[..2 * 44]; // ends inside the 3-byte height push
        assert_eq!(
            process(cut, cb2.as_bytes(), 0, 0, b"", 0, None, &mut result),
            Err(Error::InvalidArg)
        );
    }

    #[test]
    fn huge_script_length_stops_parsing() {
        // One output whose script length (2^64 - 8) runs past any offset
        let script_len = [0xf8, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff];
        let cb2 = hex(&[
            &[0xff; 4][..],
            &[1],
            &[1, 0, 0, 0, 0, 0, 0, 0],
            &[0xff],
            &script_len,
            &[0; 8],
        ]
        .concat());
        let (cb1, _) = Coinbase::new(870_000, b"").build();
        let mut result = CoinbaseResult::zeroed();
        assert_eq!(
            process(
                cb1.as_bytes(),
                cb2.as_bytes(),
                0,
                0,
                b"aaaaaaaa",
                4,
                None,
                &mut result
            ),
            Ok(())
        );
        assert_eq!(result.total_value_satoshis, 1);
    }
}
