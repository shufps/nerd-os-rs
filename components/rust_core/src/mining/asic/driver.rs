//! The BM13xx chain driver (port of the `Asic` class and the chip classes
//! `BM1366`/`BM1368`/`BM1370`/`BM1373`): init sequences, chip enumeration,
//! jobs, responses, ticket mask, hash and version rolling frequency. The UART
//! is `drivers/serial.rs`, reached through [`Uart`].
//!
//! Several tasks use one driver at the same time (jobs, results, counters,
//! frequency changes), as before; the state is behind a mutex that is never
//! held during UART I/O (a receive blocks up to 60 s).
//!
//! A chip ID response shorter than 5 bytes is not counted.

use std::sync::Mutex;

use super::{
    difficulty_mask, encode_frame, frequency_ramp, parse_response, pll_settings, vr_freq_to_reg,
    vr_reg_to_freq, Chip, TaskResult, RESPONSE_LEN, TYPE_JOB,
};
use crate::log::log;

const TAG: &core::ffi::CStr = c"asic";

const TYPE_CMD: u8 = 0x40;
const GROUP_SINGLE: u8 = 0x00;
const GROUP_ALL: u8 = 0x10;
const CMD_SETADDRESS: u8 = 0x00;
const CMD_WRITE: u8 = 0x01;
const CMD_READ: u8 = 0x02;
const CMD_INACTIVE: u8 = 0x03;
const CMD_WRITE_SINGLE: u8 = TYPE_CMD | GROUP_SINGLE | CMD_WRITE;
const CMD_WRITE_ALL: u8 = TYPE_CMD | GROUP_ALL | CMD_WRITE;
const CMD_READ_ALL: u8 = TYPE_CMD | GROUP_ALL | CMD_READ;

/// A job as the chips take it (`BM1368_job`): job ID, midstate count,
/// starting nonce, nbits, ntime, merkle root and previous block hash (both
/// big endian), version.
pub const JOB_LEN: usize = 82;

/// The serial port to the chain.
pub trait Uart {
    fn send(&mut self, data: &[u8]);
    /// Bytes received (up to `buf.len()`), 0 on a timeout, < 0 on an error.
    fn rx(&mut self, buf: &mut [u8], timeout_ms: u16) -> i32;
    /// Drops what was received so far.
    fn clear(&mut self);
    fn delay_ms(&mut self, ms: u32);
}

impl Chip {
    pub fn name(self) -> &'static core::ffi::CStr {
        match self {
            Chip::Bm1366 => c"BM1366",
            Chip::Bm1368 => c"BM1368",
            Chip::Bm1370 => c"BM1370",
            Chip::Bm1373 => c"BM1373",
        }
    }

    pub fn small_core_count(self) -> u16 {
        match self {
            Chip::Bm1366 => 894,
            Chip::Bm1368 => 1276,
            Chip::Bm1370 => 2040,
            Chip::Bm1373 => 6860, // TODO in the C++ code
        }
    }

    /// The version rolling frequency of the chip's default register 0x10.
    pub fn default_vr_frequency(self) -> u32 {
        let reg = match self {
            Chip::Bm1366 => 0x151c,
            Chip::Bm1368 => 0x15a4,
            Chip::Bm1370 | Chip::Bm1373 => 0x1eb5,
        };
        vr_reg_to_freq(reg).unwrap_or(0)
    }

    /// The response to reading register 0 (the first 5 of AA 55 13 xx 00 ..).
    fn id(self) -> [u8; 5] {
        let model = match self {
            Chip::Bm1366 => 0x66,
            Chip::Bm1368 => 0x68,
            Chip::Bm1370 => 0x70,
            Chip::Bm1373 => 0x72,
        };
        [0xaa, 0x55, 0x13, model, 0x00]
    }
}

struct State {
    current_frequency: f32,
    actual_frequency: f32,
    /// The ticket mask sent last (0xffffffff: none yet).
    asic_difficulty: u32,
    /// Chip address spacing (set by init).
    address_interval: u8,
}

pub struct Driver {
    chip: Chip,
    state: Mutex<State>,
}

impl Driver {
    pub fn new(chip: Chip) -> Self {
        Driver {
            chip,
            state: Mutex::new(State {
                current_frequency: 56.25,
                actual_frequency: 0.0,
                asic_difficulty: 0xffff_ffff,
                address_interval: 2,
            }),
        }
    }

    pub fn chip(&self) -> Chip {
        self.chip
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn address_interval(&self) -> u8 {
        self.state().address_interval
    }

    pub fn set_address_interval(&self, interval: u8) {
        self.state().address_interval = interval;
    }

    pub fn actual_frequency(&self) -> f32 {
        self.state().actual_frequency
    }

    pub fn send(&self, uart: &mut impl Uart, header: u8, data: &[u8]) {
        uart.send(&encode_frame(header, data));
    }

    fn send6(&self, uart: &mut impl Uart, header: u8, data: [u8; 6]) {
        self.send(uart, header, &data);
    }

    fn write_all(&self, uart: &mut impl Uart, reg: u8, value: [u8; 4]) {
        let [a, b, c, d] = value;
        self.send6(uart, CMD_WRITE_ALL, [0x00, reg, a, b, c, d]);
    }

    fn write_single(&self, uart: &mut impl Uart, addr: u8, reg: u8, value: [u8; 4]) {
        let [a, b, c, d] = value;
        self.send6(uart, CMD_WRITE_SINGLE, [addr, reg, a, b, c, d]);
    }

    /// Reads register 0 on all chips and counts the chip ID responses.
    fn count_asics(&self, uart: &mut impl Uart) -> i32 {
        self.send(uart, CMD_READ_ALL, &[0x00, 0x00]);
        let id = self.chip.id();
        let mut buf = [0u8; RESPONSE_LEN];
        let mut count = 0;
        loop {
            let n = uart.rx(&mut buf, 1000);
            if n <= 0 {
                return count;
            }
            // the former strncmp() over 6 bytes stopped at the NUL of the ID
            if n >= 5 && buf[..5] == id {
                count += 1;
                log!(Info, TAG, "found asic #{count}");
            } else {
                log!(Error, TAG, "unexpected response ... ignoring ...");
            }
        }
    }

    /// Sets the PLL for `target` MHz; false if there are no settings for it.
    pub fn send_hash_frequency(&self, uart: &mut impl Uart, target: f32) -> bool {
        let Some(pll) = pll_settings(target) else {
            log!(
                Error,
                TAG,
                "Didn't find PLL settings for target frequency {}",
                mhz(target)
            );
            return false;
        };
        self.send(uart, CMD_WRITE_ALL, &pll.data);
        log!(
            Info,
            TAG,
            "Setting Frequency to {}MHz ({}) (error: {}MHZ)",
            mhz(target),
            mhz(pll.actual),
            mhz(pll.error)
        );
        let mut s = self.state();
        s.current_frequency = target;
        s.actual_frequency = pll.actual;
        true
    }

    /// Ramps from the current hash frequency to `target` MHz in 6.25 MHz
    /// steps (100 ms apart).
    pub fn set_frequency(&self, uart: &mut impl Uart, target: f32) -> bool {
        let current = self.state().current_frequency;
        let mut ramp = frequency_ramp(current, target).peekable();
        while let Some(f) = ramp.next() {
            if !self.send_hash_frequency(uart, f) {
                log!(Error, TAG, "Failed to set frequency to {} MHz", mhz(f));
                return false;
            }
            if ramp.peek().is_some() {
                uart.delay_ms(100);
            }
        }
        true
    }

    /// Switches the chips to 1 Mbaud; the UART follows with the returned rate.
    pub fn set_max_baud(&self, uart: &mut impl Uart) -> i32 {
        log!(Info, TAG, "Setting max baud of 1000000");
        self.write_all(uart, 0x28, [0x11, 0x30, 0x02, 0x00]);
        1_000_000
    }

    /// Version rolling frequency (register 0x10); 0 Hz is refused.
    pub fn set_vr_frequency(&self, uart: &mut impl Uart, freq_hz: u32) {
        let Some(reg) = vr_freq_to_reg(freq_hz) else {
            log!(
                Error,
                TAG,
                "invalid version rolling frequency 0 Hz, not set"
            );
            return;
        };
        log!(Info, TAG, "setting 0x10 to {reg:08x}");
        self.write_all(uart, 0x10, reg.to_be_bytes());
    }

    /// The ticket mask for `difficulty`, sent only when it changes.
    pub fn set_difficulty_mask(&self, uart: &mut impl Uart, difficulty: i32) {
        let (mask, data) = difficulty_mask(difficulty);
        {
            let mut s = self.state();
            if s.asic_difficulty == mask {
                return;
            }
            s.asic_difficulty = mask;
        }
        log!(Info, TAG, "Setting ASIC difficulty mask to {mask}");
        self.send(uart, CMD_WRITE_ALL, &data);
    }

    /// Sends a job; returns the job ID the chips got (and report back).
    #[allow(clippy::too_many_arguments)]
    pub fn send_work(
        &self,
        uart: &mut impl Uart,
        job_id: u32,
        starting_nonce: u32,
        nbits: u32,
        ntime: u32,
        merkle_root_be: &[u8; 32],
        prev_block_hash_be: &[u8; 32],
        version: u32,
    ) -> u8 {
        let asic_id = self.chip.job_to_asic_id(job_id as u8);
        let mut job = [0u8; JOB_LEN];
        job[0] = asic_id;
        job[1] = 0x01; // midstates
        job[2..6].copy_from_slice(&starting_nonce.to_le_bytes());
        job[6..10].copy_from_slice(&nbits.to_le_bytes());
        job[10..14].copy_from_slice(&ntime.to_le_bytes());
        job[14..46].copy_from_slice(merkle_root_be);
        job[46..78].copy_from_slice(prev_block_hash_be);
        job[78..82].copy_from_slice(&version.to_le_bytes());
        self.send_raw_job(uart, &job);
        asic_id
    }

    /// Sends a job built elsewhere (CAN slave).
    pub fn send_raw_job(&self, uart: &mut impl Uart, job: &[u8; JOB_LEN]) {
        self.send(uart, TYPE_JOB | GROUP_SINGLE | CMD_WRITE, job);
    }

    /// Waits up to 60 s for a response: a nonce or a register value. `None`
    /// on a timeout, an error or garbage (then the UART is flushed).
    pub fn process_work(&self, uart: &mut impl Uart) -> Option<TaskResult> {
        let mut buf = [0u8; RESPONSE_LEN];
        let n = uart.rx(&mut buf, 60_000);
        if n < 0 {
            log!(Info, TAG, "Error in serial RX");
            return None;
        }
        if n == 0 {
            return None;
        }
        let interval = self.address_interval();
        let result = parse_response(self.chip, interval, &buf[..n as usize]);
        if result.is_none() {
            log!(
                Error,
                TAG,
                "Serial RX invalid {n}: {}",
                crate::util::cutil::to_hex(&buf[..n as usize])
            );
            uart.clear();
        }
        result
    }

    /// Asks the chips for their temperature (BM1368 only; the others answer
    /// nothing useful).
    pub fn request_chip_temp(&self, uart: &mut impl Uart) {
        if self.chip != Chip::Bm1368 {
            return;
        }
        self.send(uart, CMD_READ_ALL, &[0x00, 0xb4]);
        for value in [
            [0x80, 0x00, 0x00, 0x00],
            [0x00, 0x02, 0x00, 0x00],
            [0x01, 0x02, 0x00, 0x00],
            [0x10, 0x02, 0x00, 0x00],
        ] {
            self.write_all(uart, 0xb0, value);
        }
    }

    pub fn reset_counter(&self, uart: &mut impl Uart, reg: u8) {
        self.write_all(uart, reg, [0; 4]);
    }

    pub fn read_counter(&self, uart: &mut impl Uart, reg: u8) {
        self.send(uart, CMD_READ_ALL, &[0x00, reg]);
    }

    /// Initializes the chain (after the board reset it): enumerates the
    /// chips, gives them addresses, configures them, ramps to `frequency`
    /// MHz and enables version rolling. Returns the number of chips found.
    pub fn init(
        &self,
        uart: &mut impl Uart,
        frequency: u64,
        asic_count: u16,
        difficulty: u32,
        vr_frequency: u32,
    ) -> u8 {
        let chip = self.chip;
        // version rolling mask 0xffff; the BM1373 without the enable bit
        // until the end
        let vr_mask = if chip == Chip::Bm1373 { 0x80 } else { 0x90 };
        for _ in 0..4 {
            self.write_all(uart, 0xa4, [vr_mask, 0x00, 0xff, 0xff]);
        }

        let chips = self.count_asics(uart);
        if chips == asic_count as i32 {
            log!(
                Info,
                TAG,
                "{chips} chip(s) detected on the chain, expected {asic_count}"
            );
        } else {
            log!(
                Error,
                TAG,
                "{chips} chip(s) detected on the chain, expected {asic_count}"
            );
        }

        self.write_all(uart, 0xa4, [vr_mask, 0x00, 0xff, 0xff]);
        self.write_all(uart, 0xa8, [0x00, 0x07, 0x00, 0x00]);
        let misc = match chip {
            Chip::Bm1366 | Chip::Bm1368 => [0xff, 0x0f, 0xc1, 0x00],
            Chip::Bm1370 => [0xf0, 0x00, 0xc1, 0x00],
            Chip::Bm1373 => [0xff, 0x00, 0xc1, 0x00],
        };
        self.write_all(uart, 0x18, misc);
        // chain inactive
        self.send(uart, TYPE_CMD | GROUP_ALL | CMD_INACTIVE, &[0x00, 0x00]);

        // chip addresses spread over 0..256 (a single chip gets interval 0,
        // as before: 256 does not fit into a byte)
        let interval = match chip {
            Chip::Bm1373 => 16,
            _ if chips > 0 => (256 / next_power_of_two(chips)) as u8,
            Chip::Bm1370 => 4,
            _ => 2,
        };
        self.set_address_interval(interval);
        for i in 0..chips {
            let addr = (i as u8).wrapping_mul(interval);
            self.send(
                uart,
                TYPE_CMD | GROUP_SINGLE | CMD_SETADDRESS,
                &[addr, 0x00],
            );
        }

        // core register control
        let core: &[[u8; 4]] = match chip {
            Chip::Bm1366 => &[[0x80, 0x00, 0x85, 0x40], [0x80, 0x00, 0x80, 0x20]],
            Chip::Bm1368 => &[[0x80, 0x00, 0x8b, 0x00], [0x80, 0x00, 0x80, 0x18]],
            Chip::Bm1370 => &[[0x80, 0x00, 0x8b, 0x00], [0x80, 0x00, 0x80, 0x0c]],
            Chip::Bm1373 => &[[0x80, 0x00, 0x80, 0x0c]],
        };
        for &value in core {
            self.write_all(uart, 0x3c, value);
        }

        self.set_difficulty_mask(uart, difficulty as i32);

        match chip {
            Chip::Bm1366 | Chip::Bm1368 => {
                self.write_all(uart, 0x54, [0x00, 0x00, 0x00, 0x03]); // analog mux
                self.write_all(uart, 0x58, [0x02, 0x11, 0x11, 0x11]); // IO driver strength
            }
            Chip::Bm1370 | Chip::Bm1373 => {
                self.write_all(uart, 0x58, [0x00, 0x01, 0x11, 0x11]); // IO driver strength
                self.write_all(uart, 0x68, [0x5a, 0xa5, 0x5a, 0xa5]); // PLL3 parameter
            }
        }

        let misc_single = match chip {
            Chip::Bm1373 => [0xff, 0x00, 0xc1, 0x00],
            _ => [0xf0, 0x00, 0xc1, 0x00],
        };
        for i in 0..chips {
            let addr = (i as u8).wrapping_mul(interval);
            self.write_single(uart, addr, 0xa8, [0x00, 0x07, 0x01, 0xf0]);
            self.write_single(uart, addr, 0x18, misc_single);
            for &value in core {
                self.write_single(uart, addr, 0x3c, value);
            }
            self.write_single(uart, addr, 0x3c, [0x80, 0x00, 0x82, 0xaa]);
        }

        match chip {
            Chip::Bm1370 => {
                self.write_all(uart, 0xb9, [0x00, 0x00, 0x44, 0x80]);
                self.write_all(uart, 0x54, [0x00, 0x00, 0x00, 0x02]); // analog mux
                self.write_all(uart, 0xb9, [0x00, 0x00, 0x44, 0x80]);
                self.write_all(uart, 0x3c, [0x80, 0x00, 0x8d, 0xee]);
            }
            Chip::Bm1373 => {
                self.write_all(uart, 0x54, [0x00, 0x00, 0x00, 0x02]); // analog mux
            }
            _ => {}
        }

        self.set_frequency(uart, frequency as f32);
        self.set_vr_frequency(uart, vr_frequency);
        // enable version rolling
        self.write_all(uart, 0xa4, [0x90, 0x00, 0xff, 0xff]);

        chips as u8
    }
}

/// `%.2f` without Rust's float formatting (~10 KB of code); NaN shows as 0.00.
fn mhz(f: f32) -> String {
    let centi = (f as f64 * 100.0).round() as i64;
    let sign = if centi < 0 { "-" } else { "" };
    let centi = centi.unsigned_abs();
    format!("{sign}{}.{:02}", centi / 100, centi % 100)
}

fn next_power_of_two(n: i32) -> i32 {
    let mut power = 1;
    while power < n {
        power <<= 1;
    }
    power
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mhz_formats_like_printf() {
        assert_eq!(mhz(525.0), "525.00");
        assert_eq!(mhz(493.75), "493.75");
        assert_eq!(mhz(0.004), "0.00");
        assert_eq!(mhz(-6.25), "-6.25");
        assert_eq!(mhz(f32::NAN), "0.00");
    }
}

pub(crate) mod api {
    use super::*;
    use core::ffi::c_void;

    /// `asic_uart_t`
    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct CUart {
        pub ctx: *mut c_void,
        pub send: unsafe extern "C" fn(*mut c_void, *const u8, usize),
        pub rx: unsafe extern "C" fn(*mut c_void, *mut u8, usize, u16) -> i32,
        pub clear: unsafe extern "C" fn(*mut c_void),
        pub delay_ms: unsafe extern "C" fn(*mut c_void, u32),
    }

    // SAFETY: thread-safe callbacks (serial.rs locks its sends); the
    // driver is shared between tasks as before.
    unsafe impl Send for CUart {}
    // SAFETY: as for Send; the callbacks are safe to call from several tasks
    unsafe impl Sync for CUart {}

    impl Uart for CUart {
        fn send(&mut self, data: &[u8]) {
            // SAFETY: a valid callback for ctx (asic_driver_new); data readable for its length
            unsafe { (self.send)(self.ctx, data.as_ptr(), data.len()) }
        }
        fn rx(&mut self, buf: &mut [u8], timeout_ms: u16) -> i32 {
            // SAFETY: a valid callback for ctx (asic_driver_new); buf writable for its length
            unsafe { (self.rx)(self.ctx, buf.as_mut_ptr(), buf.len(), timeout_ms) }
        }
        fn clear(&mut self) {
            // SAFETY: a valid callback for ctx (asic_driver_new)
            unsafe { (self.clear)(self.ctx) }
        }
        fn delay_ms(&mut self, ms: u32) {
            // SAFETY: a valid callback for ctx (asic_driver_new)
            unsafe { (self.delay_ms)(self.ctx, ms) }
        }
    }

    /// `asic_driver_t`
    pub struct Handle {
        driver: Driver,
        uart: CUart,
    }

    impl Handle {
        /// A driver for `chip` (`asic_chip_t`) on `uart`; None for an unknown
        /// chip.
        pub fn new(chip: i32, uart: CUart) -> Option<Self> {
            Some(Handle {
                driver: Driver::new(Chip::from_raw(chip)?),
                uart,
            })
        }

        pub fn driver(&self) -> &Driver {
            &self.driver
        }

        /// Runs `f` with the driver and its UART.
        pub fn run<T>(&self, f: impl FnOnce(&Driver, &mut CUart) -> T) -> T {
            let mut uart = self.uart;
            f(&self.driver, &mut uart)
        }
    }

    // All functions below: `d` from `asic_driver_new()`.
}
