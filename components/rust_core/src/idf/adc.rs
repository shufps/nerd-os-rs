//! The NerdAxe family's core voltage ADC (port of
//! `main/boards/drivers/nerdaxe/adc.cpp`): ADC1 channel 1 at 12 dB
//! attenuation with the legacy driver and the `esp_adc_cal` calibration.

use super::idf_const;
#[cfg(not(target_os = "espidf"))]
use core::ffi::c_int;

idf_const!(CHANNEL: u32 = adc1_channel_t_ADC1_CHANNEL_1, 1);
idf_const!(ATTEN: u32 = adc_atten_t_ADC_ATTEN_DB_12, 3);
idf_const!(UNIT: u32 = adc_unit_t_ADC_UNIT_1, 0);
idf_const!(WIDTH: u32 = adc_bits_width_t_ADC_WIDTH_MAX - 1, 12);

#[cfg(target_os = "espidf")]
pub use crate::idf::raw::esp_adc_cal_characteristics_t as Characteristics;

/// `esp_adc_cal_characteristics_t` (host model)
#[cfg(not(target_os = "espidf"))]
#[derive(Clone, Copy, Debug)]
pub struct Characteristics {
    pub adc_num: u32,
    pub atten: u32,
    pub bit_width: u32,
    pub coeff_a: u32,
    pub coeff_b: u32,
    pub vref: u32,
    pub low_curve: *const u32,
    pub high_curve: *const u32,
    pub version: u8,
}

#[cfg(target_os = "espidf")]
mod sys {
    pub use crate::idf::raw::{
        adc1_config_channel_atten, adc1_config_width, adc1_get_raw, esp_adc_cal_characterize,
        esp_adc_cal_raw_to_voltage,
    };
}

#[cfg(not(target_os = "espidf"))]
mod sys {
    use super::*;
    pub unsafe fn adc1_config_channel_atten(_: u32, _: u32) -> c_int {
        -1
    }
    pub unsafe fn adc1_config_width(_: u32) -> c_int {
        -1
    }
    pub unsafe fn adc1_get_raw(_: u32) -> c_int {
        -1
    }
    pub unsafe fn esp_adc_cal_characterize(
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: *mut Characteristics,
    ) -> u32 {
        0
    }
    pub unsafe fn esp_adc_cal_raw_to_voltage(_: u32, _: *const Characteristics) -> u32 {
        0
    }
}

struct Chars(std::sync::Mutex<Characteristics>);
// SAFETY: the curve pointers point into static read-only calibration tables, and all
// access goes through the mutex.
unsafe impl Sync for Chars {}

// all zero until init()
// SAFETY: integers and raw pointers, for which all zero (null) is valid.
static CHARS: Chars = Chars(std::sync::Mutex::new(unsafe { core::mem::zeroed() }));

fn chars() -> std::sync::MutexGuard<'static, Characteristics> {
    CHARS.0.lock().unwrap_or_else(|e| e.into_inner())
}

/// Channel attenuation and calibration.
pub fn init() {
    let mut c = chars();
    // SAFETY: plain configuration calls; `c` is the locked characteristics, written by the call.
    unsafe {
        sys::adc1_config_channel_atten(CHANNEL, ATTEN);
        sys::esp_adc_cal_characterize(UNIT, ATTEN, WIDTH, 0, &mut *c);
    }
}

/// The core voltage in mV.
pub fn vcore_mv() -> u16 {
    let c = chars();
    // SAFETY: plain ADC calls; `c` is the locked characteristics, only read.
    unsafe {
        sys::adc1_config_width(WIDTH);
        sys::esp_adc_cal_raw_to_voltage(sys::adc1_get_raw(CHANNEL) as u32, &*c) as u16
    }
}

// firmware only
#[cfg(target_os = "espidf")]
mod ffi {}
