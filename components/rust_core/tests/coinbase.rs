//! Golden tests of the coinbase decoder (`coinbase_process`): random
//! coinbase data, started from well-formed coinbases and then mutated
//! (truncated, corrupted, odd lengths, bogus lengths and varints, odd
//! addresses), against the outputs of the former C code
//! (tests/golden/coinbase.txt, recorded by the differential tests).

use std::cell::Cell;
use std::ffi::CString;

use rust_core::pool::coinbase::{coinbase_process, CoinbaseResult, ESP_ERR_INVALID_ARG, ESP_OK};

mod golden;
use golden::Rec;

struct Rng(Cell<u64>);

impl Rng {
    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&self, percent: usize) -> bool {
        self.below(100) < percent
    }
    fn bytes(&self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.next() as u8).collect()
    }
    fn pick<'a, T>(&self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }
}

fn hex(bytes: &[u8]) -> Vec<u8> {
    bytes
        .iter()
        .flat_map(|b| format!("{b:02x}").into_bytes())
        .collect()
}

/// (address, scriptPubKey hex) — BIP-173/350 vectors, genesis P2PKH, a P2SH.
const ADDRESSES: &[(&str, &str)] = &[
    (
        "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa",
        "76a91462e907b15cbf27d5425399ebf6f0fb50ebb88f1888ac",
    ),
    ("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy", ""),
    (
        "bc1qw508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
        "0014751e76e8199196d454941c45d1b3a323f1433bd6",
    ),
    (
        "bc1qrp33g0q5c5txsp9arysrx4k6zdkfs4nce4xj0gdcccefvpysxf3qccfmv3",
        "00201863143c14c5166804bd19203356da136c985678cd4d27a1b8c6329604903262",
    ),
    (
        "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj0",
        "512079be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
    ),
];

const ODD_ADDRESSES: &[&str] = &[
    "",
    "BC1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
    "bc1QW508D6QEJXTDG4Y5R3ZARVARY0C5XW7KV8F3T4",
    "tb1qw508d6qejxtdg4y5r3zarvary0c5xw7kxpjzsx",
    "bc1zw508d6qejxtdg4y5r3zarvaryvaxxpcs",
    "BC1SW50QGDZ25J",
    "bc1qr508d6qejxtdg4y5r3zarvary0c5xw7kv8f3t4",
    "bc1p0xlxvlhemja6c4dqv22uapctqupfhlxm9h8z3k2e72q4k9hcz7vqzk5jj1",
    "bc10w508d6qejxtdg4y5r3zarvary0c5xw7kw508d6qejxtdg4y5r3zarvary0c5xw7kw5rljs90",
    "1111111111111111111114oLvT2",
    "mipcBbFg9gMiCh81Kj8tqqdgoZub1ZJRfn",
    "2MzQwSSnBHWHqSAqtTVQ6v47XtaisrJa1Vc",
    "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb",
    "1A1zP1eP5QGefi2DMPTfTL5SLmv7Div0Na",
    "11111111111111111111111111111111111111111",
    "zzz",
];

struct Case {
    coinbase_1: Vec<u8>,
    coinbase_2: Vec<u8>,
    block_version: u32,
    nbits: u32,
    extranonce1: Vec<u8>,
    extranonce2_len: i32,
    user_address: Option<Vec<u8>>,
}

fn varint(rng: &Rng, n: u64) -> Vec<u8> {
    match rng.below(20) {
        0 => [&[0xfd][..], &(n as u16).to_le_bytes()].concat(),
        1 => [&[0xfe][..], &(n as u32).to_le_bytes()].concat(),
        2 => [&[0xff][..], &n.to_le_bytes()].concat(),
        3 => vec![rng.next() as u8],
        _ => vec![n.min(0xfc) as u8],
    }
}

fn script_pubkey(rng: &Rng) -> Vec<u8> {
    let (_, spk) = rng.pick(ADDRESSES);
    if !spk.is_empty() && rng.chance(70) {
        let mut out = vec![0; spk.len() / 2];
        for (o, pair) in out.iter_mut().zip(spk.as_bytes().chunks(2)) {
            *o = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        out
    } else {
        let len = rng.below(40);
        rng.bytes(len)
    }
}

fn generate(rng: &Rng) -> Case {
    let height: u32 = match rng.below(4) {
        0 => *rng.pick(&[0, 1, 0xff, 0x100, 965_663, 965_664, 965_665]),
        1 => rng.next() as u32,
        _ => 800_000 + rng.below(300_000) as u32,
    };
    let height_bytes = height.to_le_bytes();
    let height_len = 4 - height_bytes
        .iter()
        .rev()
        .take_while(|&&b| b == 0)
        .count()
        .min(3);

    let tag_len = rng.below(if rng.chance(10) { 300 } else { 60 });
    let tag: Vec<u8> = if rng.chance(50) {
        (0..tag_len).map(|_| b' ' + rng.below(95) as u8).collect()
    } else {
        rng.bytes(tag_len)
    };
    // Pools may put part of the tag after the extranonces.
    let split = rng.below(tag.len() + 1);
    let extranonce1 = rng.bytes(rng.below(9));
    let extranonce2_len = rng.below(9);

    let scriptsig_len = 1 + height_len + tag.len() + extranonce1.len() + extranonce2_len;
    let mut cb1 = vec![1, 0, 0, 0, 1];
    cb1.extend([0; 32]);
    cb1.extend([0xff; 4]);
    cb1.push(scriptsig_len as u8);
    cb1.push(height_len as u8);
    cb1.extend(&height_bytes[..height_len]);
    cb1.extend(&tag[..split]);

    let mut cb2 = tag[split..].to_vec();
    let sequence = *rng.pick(&[0xffff_ffff, 0xffff_fffe, 0, rng.next() as u32]);
    cb2.extend(sequence.to_le_bytes());
    let outputs = rng.below(6);
    cb2.extend(varint(rng, outputs as u64));
    for _ in 0..outputs {
        let value = *rng.pick(&[0, 1, 312_500_000, rng.next(), rng.next() >> 20]);
        cb2.extend(value.to_le_bytes());
        let spk = script_pubkey(rng);
        cb2.extend(varint(rng, spk.len() as u64));
        cb2.extend(spk);
    }
    let lock_time = if rng.chance(50) {
        height.wrapping_sub(1)
    } else {
        rng.next() as u32
    };
    cb2.extend(lock_time.to_le_bytes());

    let mut case = Case {
        coinbase_1: hex(&cb1),
        coinbase_2: hex(&cb2),
        block_version: *rng.pick(&[0x2000_0000, 0x2000_0010, rng.next() as u32]),
        nbits: *rng.pick(&[0x1703_4219, 0x1d00_ffff, 0, rng.next() as u32]),
        extranonce1: hex(&extranonce1),
        extranonce2_len: extranonce2_len as i32,
        user_address: match rng.below(10) {
            0 => None,
            1..=3 => Some(rng.pick(ODD_ADDRESSES).as_bytes().to_vec()),
            _ => Some(rng.pick(ADDRESSES).0.as_bytes().to_vec()),
        },
    };
    mutate(rng, &mut case);
    case
}

fn mutate(rng: &Rng, case: &mut Case) {
    fn corrupt(rng: &Rng, s: &mut [u8]) {
        if s.is_empty() {
            return;
        }
        let i = rng.below(s.len());
        s[i] = *rng.pick(b"0123456789abcdefABCDEFxzG ~-\x7f\x80\xff");
    }
    fn truncate(rng: &Rng, s: &mut Vec<u8>) {
        let len = rng.below(s.len() + 1);
        s.truncate(len);
    }
    fn set_byte(rng: &Rng, s: &mut [u8], byte_index: usize) {
        if s.len() >= 2 * byte_index + 2 {
            let b = hex(&[rng.next() as u8]);
            s[2 * byte_index..2 * byte_index + 2].copy_from_slice(&b);
        }
    }

    if rng.chance(60) {
        return;
    }
    for _ in 0..1 + rng.below(3) {
        match rng.below(12) {
            0 => corrupt(rng, &mut case.coinbase_1),
            1 => corrupt(rng, &mut case.coinbase_2),
            2 => truncate(rng, &mut case.coinbase_1),
            3 => truncate(rng, &mut case.coinbase_2),
            4 => set_byte(rng, &mut case.coinbase_1, 41), // scriptsig length
            5 => set_byte(rng, &mut case.coinbase_1, 42), // block height length
            6 => {
                case.extranonce2_len =
                    *rng.pick(&[-1, -100, 0, 255, i32::MAX, i32::MIN, rng.next() as i32])
            }
            7 => truncate(rng, &mut case.extranonce1),
            8 => {
                let i = rng.below(case.coinbase_2.len() / 2 + 1);
                set_byte(rng, &mut case.coinbase_2, i);
            }
            9 => {
                if let Some(addr) = &mut case.user_address {
                    corrupt(rng, addr);
                }
            }
            10 => case.coinbase_1.extend(hex(&rng.bytes(rng.below(8)))),
            _ => case.coinbase_2.push(b'f'), // odd length
        }
    }
}

/// Inputs outside what the C code defined (not compared with it when
/// recording), and what Rust does with them:
/// * coinbase_1 ends inside the block height push: Rust returns
///   ESP_ERR_INVALID_ARG.
/// * extranonce2_len pushes the coinbase_2 offset to near INT_MAX: Rust stops.
/// * an output script length beyond the remaining data (0xff varint, or a 0xfe
///   varint >= 2^31): Rust stops parsing.
fn reference_undefined(case: &Case) -> Option<i32> {
    let cb1_len = case.coinbase_1.len() / 2;
    if cb1_len < 43 {
        return None;
    }
    let digit = |c: u8| (c as char).to_digit(16).unwrap_or(0) as usize;
    let byte = |i: usize| digit(case.coinbase_1[2 * i]) << 4 | digit(case.coinbase_1[2 * i + 1]);
    let (scriptsig_len, height_len) = (byte(41), byte(42));
    if !(1..=4).contains(&height_len) {
        return None;
    }
    if cb1_len < 43 + height_len {
        return Some(ESP_ERR_INVALID_ARG);
    }
    if scriptsig_len < 1 + height_len {
        return None;
    }
    let remainder = (scriptsig_len - 1 - height_len) as i32 - (cb1_len - 43 - height_len) as i32;
    let extranonces = ((case.extranonce1.len() / 2) as i32).wrapping_add(case.extranonce2_len);
    let cb2_offset = remainder.wrapping_sub(extranonces);
    if remainder > 0 && cb2_offset > i32::MAX - 4 {
        return Some(ESP_OK);
    }
    let cb2_offset = if remainder > 0 && cb2_offset > 0 {
        cb2_offset as usize
    } else {
        0
    };
    c_offset_wraps(case, cb2_offset).then_some(ESP_OK)
}

/// Walks the outputs like the decoder and reports whether C would wrap the
/// offset around on a script length (see `reference_undefined`).
fn c_offset_wraps(case: &Case, mut offset: usize) -> bool {
    let digit = |c: u8| (c as char).to_digit(16).unwrap_or(0) as u8;
    let data: Vec<u8> = case
        .coinbase_2
        .chunks_exact(2)
        .map(|p| digit(p[0]) << 4 | digit(p[1]))
        .collect();
    let len = data.len();
    // (value as decoded correctly, value as C decodes it)
    let varint = |offset: &mut usize| -> (u64, u64) {
        let Some(&first) = data.get(*offset) else {
            return (0, 0);
        };
        *offset += 1;
        let n = match first {
            0xfd => 2,
            0xfe => 4,
            0xff => 8,
            _ => return (first as u64, first as u64),
        };
        let Some(bytes) = data.get(*offset..*offset + n) else {
            return (0, 0);
        };
        *offset += n;
        let v = bytes.iter().rev().fold(0u64, |acc, &b| acc << 8 | b as u64);
        let c = if n == 4 {
            v as u32 as i32 as i64 as u64
        } else {
            v
        };
        (v, c)
    };

    if offset + 4 > len {
        return false;
    }
    offset += 4;
    if offset >= len {
        return false;
    }
    let (count, _) = varint(&mut offset);
    let mut i = 0;
    while i < count && offset < len {
        if offset + 8 > len {
            return false;
        }
        offset += 8;
        if offset >= len {
            return false;
        }
        let (script_len, c_script_len) = varint(&mut offset);
        if (offset as u64).checked_add(c_script_len).is_none()
            && (offset as u64).wrapping_add(c_script_len) <= len as u64
        {
            return true;
        }
        if offset as u64 + script_len > len as u64 {
            return false;
        }
        offset += script_len as usize;
        i += 1;
    }
    false
}

fn run(case: &Case) -> (i32, CoinbaseResult) {
    let c =
        |s: &[u8]| CString::new(s.iter().copied().filter(|&b| b != 0).collect::<Vec<_>>()).unwrap();
    let (cb1, cb2, ext1) = (
        c(&case.coinbase_1),
        c(&case.coinbase_2),
        c(&case.extranonce1),
    );
    let addr = case.user_address.as_deref().map(c);
    // Garbage, to check that the result is reset
    let mut result = CoinbaseResult {
        block_height: 0xdead_beef,
        ..CoinbaseResult::zeroed()
    };
    // SAFETY: NUL-terminated strings (or NULL), a writable result.
    let rc = unsafe {
        coinbase_process(
            cb1.as_ptr() as _,
            cb2.as_ptr() as _,
            case.block_version,
            case.nbits,
            ext1.as_ptr() as _,
            case.extranonce2_len,
            addr.as_ref().map_or(std::ptr::null(), |a| a.as_ptr()) as _,
            &mut result,
        )
    };
    (rc, result)
}

/// The result as recorded in the golden file.
fn result_text(r: &CoinbaseResult) -> String {
    let scriptsig = rust_core::util::cutil::to_hex(r.scriptsig_bytes());
    format!(
        "{:#x} {} {scriptsig} {} {} {} {}",
        r.network_difficulty.to_bits(),
        r.block_height,
        r.total_value_satoshis,
        r.user_value_satoshis,
        r.bip54_signaling,
        r.bip110_signaling
    )
}

#[test]
fn rust_matches_c() {
    // Fixed (the golden file records the outputs for exactly these inputs).
    let iterations: usize = 150_000;
    let seed: u64 = 0x5eed_c01b_a5e0_0001;
    let rng = Rng(Cell::new(seed));
    let mut rec = Rec::new("coinbase", "rust_matches_c");

    let (mut compared, mut ok, mut user_matches, mut skipped) = (0, 0, 0, 0);
    for i in 0..iterations {
        let case = generate(&rng);
        let (rust_rc, rust) = run(&case);

        if let Some(expected) = reference_undefined(&case) {
            assert_eq!(rust_rc, expected, "case {i}");
            rec.add(format!("{i} undefined {rust_rc:#x}"));
            skipped += 1;
            continue;
        }
        rec.add(format!("{i} {rust_rc:#x} {}", result_text(&rust)));
        compared += 1;
        ok += (rust_rc == 0) as usize;
        user_matches += (rust.user_value_satoshis > 0) as usize;
    }
    eprintln!(
        "{compared} cases ({ok} decoded OK, {user_matches} with user payout), {skipped} skipped (undefined in the reference)"
    );
    // Make sure the generator actually exercises the interesting paths.
    assert!(
        ok > compared / 4 && user_matches > compared / 20,
        "generator coverage too low"
    );
    rec.finish();
}

#[test]
fn null_arguments() {
    let mut result = CoinbaseResult::zeroed();
    let s = c"00";
    let null = std::ptr::null();
    let mut rec = Rec::new("coinbase", "null_arguments");
    for args in [
        (null, s.as_ptr(), s.as_ptr()),
        (s.as_ptr(), null, s.as_ptr()),
        (s.as_ptr(), s.as_ptr(), null),
    ] {
        let rust = unsafe { coinbase_process(args.0, args.1, 0, 0, args.2, 0, null, &mut result) };
        assert_eq!(rust, ESP_ERR_INVALID_ARG);
        rec.add(format!("{rust:#x}"));
    }
    rec.finish();
}
