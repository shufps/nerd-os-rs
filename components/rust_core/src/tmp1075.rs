//! TMP1075 temperature sensors (port of `main/boards/drivers/TMP1075.cpp`):
//! the temperature register of the sensor at 0x48 + index, 0.0625 °C per
//! bit in the upper 12 bits; 0 on a bus error or an invalid reading.

use core::ffi::{c_int, CStr};

use crate::idf::{i2c, ESP_OK};
use crate::log::log;

const TAG: &CStr = c"TMP1075.c";
const ADDR: u8 = 0x48;
const TEMP_REG: u8 = 0x00;

/// The temperature of a raw register value; None if invalid.
pub fn celsius(raw: u16) -> Option<f32> {
    (raw <= 0x7ff0).then_some((raw >> 4) as f32 * 0.0625)
}

/// The temperature of sensor `index` (0x48 + index); 0 on a bus error or
/// an invalid reading.
pub fn read_temperature(index: c_int) -> f32 {
    let mut raw = 0u16;
    if i2c::read_word_be_acked(ADDR.wrapping_add(index as u8), TEMP_REG, &mut raw) != ESP_OK {
        log!(Info, TAG, "Failed to read temperature from TMP1075");
        return 0.0;
    }
    match celsius(raw) {
        Some(t) => t,
        None => {
            log!(Info, TAG, "Invalid TMP1075 reading: {raw:04x}");
            0.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion() {
        assert_eq!(celsius(0x1900), Some(25.0));
        assert_eq!(celsius(0x7ff0), Some(127.9375));
        assert_eq!(celsius(0x7ff1), None);
        assert_eq!(celsius(0xe700), None);
    }
}
