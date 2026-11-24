//! BM13xx serial protocol (port of the protocol parts of `asic.cpp` and the
//! chip classes): frame encoding with CRC5/CRC16, job and nonce ID mapping per
//! chip, parsing of the 11-byte responses, the ticket (difficulty) mask, the
//! version rolling frequency register, the PLL settings for a hash frequency
//! and the 6.25 MHz frequency ramp. The driver on top: [`driver`].
//!
//! Behaviour notes:
//! * a short read (fewer than 11 bytes) is rejected like a bad preamble,
//! * a version rolling frequency of 0 gives no register value.

pub mod driver;

/// `asic_chip_t` (`asic_driver.h`)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chip {
    Bm1366 = 0,
    Bm1368 = 1,
    Bm1370 = 2,
    Bm1373 = 3,
}

impl Chip {
    pub fn from_raw(raw: i32) -> Option<Chip> {
        Some(match raw {
            0 => Chip::Bm1366,
            1 => Chip::Bm1368,
            2 => Chip::Bm1370,
            3 => Chip::Bm1373,
            _ => return None,
        })
    }

    /// The job ID as sent to the chip.
    pub fn job_to_asic_id(self, job_id: u8) -> u8 {
        match self {
            Chip::Bm1366 => job_id.wrapping_mul(8) & 0x7f,
            // job IDs: 00, 18, 30, 48, 60, 78, 10, 28, 40, 58, 70, 08, 20, 38, 50, 68
            _ => job_id.wrapping_mul(24) & 0x7f,
        }
    }

    /// The job ID from the ID in a nonce response.
    pub fn asic_to_job_id(self, asic_id: u8) -> u8 {
        match self {
            Chip::Bm1366 => asic_id & 0xf8,
            _ => (asic_id & 0xf0) >> 1,
        }
    }

    /// The chip that found `nonce` (as received, little endian).
    pub fn nonce_to_asic(self, nonce: u32, address_interval: u8) -> i32 {
        let nonce_h = nonce.swap_bytes();
        match self {
            Chip::Bm1373 => ((nonce_h >> 24) & 0x03) as i32,
            _ if address_interval == 0 => 0,
            _ => ((nonce_h >> 17) & 0xff) as i32 / address_interval as i32,
        }
    }
}

pub fn chip_index_from_addr(addr: u8, address_interval: u8) -> i32 {
    if address_interval == 0 {
        0
    } else {
        (addr / address_interval) as i32
    }
}

/// CRC5 (polynomial 0x05, initial 0x1f) over all bits, MSB first.
pub fn crc5(data: &[u8]) -> u8 {
    let mut crc: u8 = 0x1f;
    for &byte in data {
        for bit in (0..8).rev() {
            let din = (byte >> bit) & 1;
            let feedback = ((crc >> 4) & 1) ^ din;
            crc = ((crc << 1) & 0x1f) ^ if feedback != 0 { 0x05 } else { 0 };
        }
    }
    crc
}

/// CRC-16/CCITT-FALSE (polynomial 0x1021, initial 0xffff).
pub fn crc16_false(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xffff;
    for &byte in data {
        crc ^= (byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

pub const TYPE_JOB: u8 = 0x20;

/// A frame: preamble 55 AA, header, length, data, then CRC16 (big endian)
/// for jobs or CRC5 for commands, both over header, length and data.
pub fn encode_frame(header: u8, data: &[u8]) -> Vec<u8> {
    let job = header & TYPE_JOB != 0;
    let mut frame = Vec::with_capacity(data.len() + 6);
    frame.extend([0x55, 0xaa, header]);
    // the length counts header, length, data and CRC
    frame.push((data.len() + if job { 4 } else { 3 }) as u8);
    frame.extend(data);
    if job {
        frame.extend(crc16_false(&frame[2..]).to_be_bytes());
    } else {
        frame.push(crc5(&frame[2..]));
    }
    frame
}

pub const RESPONSE_LEN: usize = 11;

/// `task_result` (`asic_driver.h`)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TaskResult {
    pub job_id: u8,
    pub nonce: u32,
    pub rolled_version: u32,
    pub asic_nr: i32,
    pub data: u32,
    pub reg: u8,
    pub is_reg_resp: u8,
}

// Must match the static assertion in asic_driver.h.
const _: () = assert!(core::mem::size_of::<TaskResult>() == 24);

/// Parses a response: AA 55, nonce (or register value), midstate number (or
/// chip address), job ID (or register), version bits, CRC5 with bit 7 set for
/// nonces. `None` for a short read or a bad preamble. The CRC is not checked,
/// as before.
pub fn parse_response(chip: Chip, address_interval: u8, buf: &[u8]) -> Option<TaskResult> {
    if buf.len() < RESPONSE_LEN || buf[0] != 0xaa || buf[1] != 0x55 {
        return None;
    }
    let nonce = u32::from_le_bytes(buf[2..6].try_into().unwrap());
    let (midstate_num, job_id) = (buf[6], buf[7]);
    let version = u16::from_le_bytes([buf[8], buf[9]]);
    let mut result = TaskResult::default();
    if buf[10] & 0x80 == 0 {
        result.data = nonce.swap_bytes();
        result.reg = job_id;
        result.is_reg_resp = 1;
        result.asic_nr = chip_index_from_addr(midstate_num, address_interval);
    } else {
        result.job_id = chip.asic_to_job_id(job_id);
        result.asic_nr = chip.nonce_to_asic(nonce, address_interval);
        result.nonce = nonce;
        // the rolled version bits, in place (bits 13..28)
        result.rolled_version = (version.swap_bytes() as u32) << 13;
    }
    Some(result)
}

/// The largest power of two not above `difficulty` (1 for values below 2),
/// minus one, and the TICKET_MASK register write for it: the four mask bytes
/// in reverse order, each bit-reversed.
pub fn difficulty_mask(difficulty: i32) -> (u32, [u8; 6]) {
    let mut power = 0;
    let mut n = difficulty;
    while n > 1 {
        n >>= 1;
        power += 1;
    }
    let mask = (1u32 << power) - 1;
    let mut data = [0x00, 0x14, 0, 0, 0, 0];
    for i in 0..4 {
        data[5 - i] = ((mask >> (8 * i)) as u8).reverse_bits();
    }
    (mask, data)
}

/// The version rolling counter ticks every 5000 cycles of the 15 MHz IO
/// clock; register 0x10 holds 65536 ticks per rolled version.
const VR_REG_PER_HZ: u64 = 65536 * (15_000_000 / 5000);

/// Register 0x10 for a version rolling frequency (rounded); `None` for 0.
pub fn vr_freq_to_reg(freq_hz: u32) -> Option<u32> {
    (freq_hz != 0).then(|| ((VR_REG_PER_HZ + freq_hz as u64 / 2) / freq_hz as u64) as u32)
}

/// The version rolling frequency of a register 0x10 value (rounded).
pub fn vr_reg_to_freq(reg: u32) -> Option<u32> {
    (reg != 0).then(|| ((VR_REG_PER_HZ + reg as u64 / 2) / reg as u64) as u32)
}

/// PLL settings for a hash frequency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pll {
    /// The register write (PLL0 parameter, register 0x08).
    pub data: [u8; 6],
    /// The frequency these settings give (MHz).
    pub actual: f32,
    /// |target - actual| (MHz).
    pub error: f32,
}

/// PLL settings for `target` MHz from the 25 MHz reference: the feedback
/// divider in 0xa0..=0xef, the reference divider 1 or 2 and two post dividers
/// (postdiv1 >= postdiv2), within 2 MHz of the target, preferring the smallest
/// post divider product (the same settings as the S21 dumps). `None` if there
/// are none. The arithmetic is that of the C++ code, partly in double (the
/// 25.0 literals), partly in float.
pub fn pll_settings(target: f32) -> Option<Pll> {
    let mut min_diff: f32 = 2.0;
    let (mut postdiv_min, mut postdiv2_min) = (255, 255);
    let mut best = None;
    for refdiv in (1..=2).rev() {
        for postdiv1 in (1..=7).rev() {
            for postdiv2 in (1..=7).rev() {
                let divs = refdiv * postdiv2 * postdiv1;
                let fb_divider = (target as f64 / 25.0 * divs as f64).round() as i32;
                let newf = (25.0 * fb_divider as f64 / divs as f64) as f32;
                let diff = (target - newf).abs();
                if (0xa0..=0xef).contains(&fb_divider)
                    && diff <= min_diff
                    && postdiv1 >= postdiv2
                    && postdiv1 * postdiv2 < postdiv_min
                    && postdiv2 <= postdiv2_min
                {
                    postdiv2_min = postdiv2;
                    postdiv_min = postdiv1 * postdiv2;
                    min_diff = diff;
                    best = Some((refdiv, fb_divider, postdiv1, postdiv2, newf));
                }
            }
        }
    }
    let (refdiv, fb_divider, postdiv1, postdiv2, actual) = best?;
    let vco_range = if fb_divider * 25 / refdiv >= 2400 {
        0x50
    } else {
        0x40
    };
    Some(Pll {
        data: [
            0x00,
            0x08,
            vco_range,
            fb_divider as u8,
            refdiv as u8,
            ((((postdiv1 - 1) & 0xf) << 4) | ((postdiv2 - 1) & 0xf)) as u8,
        ],
        actual,
        error: min_diff,
    })
}

/// The frequencies to set on the way from `current` to `target` (MHz): first
/// onto the 6.25 MHz grid in the direction of the target (if not on it), then
/// in 6.25 MHz steps up to the target, then the target once more. The chip
/// gets 100 ms after every step except the last. Lazy: an absurd target
/// fails at the first frequency without PLL settings, as before.
pub fn frequency_ramp(current: f32, target: f32) -> impl Iterator<Item = f32> {
    const STEP: f32 = 6.25;
    let direction = if target > current { STEP } else { -STEP };
    let mut current = current;
    let mut align = current % STEP != 0.0;
    let mut done = false;
    core::iter::from_fn(move || {
        if done {
            return None;
        }
        if align {
            align = false;
            current = if direction > 0.0 {
                (current / STEP).ceil() * STEP
            } else {
                (current / STEP).floor() * STEP
            };
            return Some(current);
        }
        if (direction > 0.0 && current < target) || (direction < 0.0 && current > target) {
            let step = direction.abs().min((target - current).abs());
            current += if direction > 0.0 { step } else { -step };
            return Some(current);
        }
        done = true;
        Some(target)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_frames() {
        // read register 0 on all chips (count_asics)
        assert_eq!(
            encode_frame(0x52, &[0, 0]),
            [0x55, 0xaa, 0x52, 0x05, 0, 0, 0x0a]
        );
        // chain inactive
        assert_eq!(
            encode_frame(0x53, &[0, 0]),
            [0x55, 0xaa, 0x53, 0x05, 0, 0, 0x03]
        );
    }

    #[test]
    fn crc16_check_value() {
        assert_eq!(crc16_false(b"123456789"), 0x29b1);
    }

    #[test]
    fn short_or_misaligned_responses_are_rejected() {
        let ok = [0xaa, 0x55, 1, 2, 3, 4, 0, 0x18, 0, 0, 0x80];
        assert!(parse_response(Chip::Bm1368, 2, &ok).is_some());
        assert!(parse_response(Chip::Bm1368, 2, &ok[..10]).is_none());
        let mut bad = ok;
        bad[1] = 0x56;
        assert!(parse_response(Chip::Bm1368, 2, &bad).is_none());
    }

    #[test]
    fn known_pll_settings() {
        // the exact values are checked against the C++ code (tests/asic.rs)
        let p = pll_settings(525.0).unwrap();
        assert_eq!(&p.data[..2], &[0x00, 0x08]);
        assert!(p.error <= 2.0 && (p.actual - 525.0).abs() <= 2.0);
        assert!(pll_settings(10_000.0).is_none());
    }

    #[test]
    fn ramp_aligns_steps_and_repeats_the_target() {
        let up: Vec<f32> = frequency_ramp(56.25, 70.0).collect();
        assert_eq!(up, [62.5, 68.75, 70.0, 70.0]);
        let down: Vec<f32> = frequency_ramp(503.0, 490.0).collect();
        assert_eq!(down, [500.0, 493.75, 490.0, 490.0]);
        assert_eq!(frequency_ramp(500.0, 500.0).collect::<Vec<_>>(), [500.0]);
    }

    #[test]
    fn vr_frequency_zero_has_no_register() {
        assert_eq!(vr_freq_to_reg(0), None);
        assert_eq!(vr_reg_to_freq(0), None);
        assert_eq!(vr_reg_to_freq(0x15a4), Some(35_489)); // 196608000 / 5540
    }
}
