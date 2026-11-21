//! A copy of the display content for development (cargo feature
//! `screenshot`, CMake option `RUST_CORE_SCREENSHOT`): LVGL renders into
//! partial buffers, so every flushed area is copied into a shadow frame
//! buffer in PSRAM, which `GET /api/debug/screen` returns as a BMP. Without
//! the feature the flush hook does nothing and the route does not exist.
//! `POST /api/debug/press?btn=1|2` simulates a short button press, so every
//! screen can be reached for a screenshot.
//! Not for release builds: the display can show the OTP enrollment QR code.

/// The T-Display-S3 panel (landscape).
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 170;

#[cfg(feature = "screenshot")]
mod imp {
    use super::{HEIGHT, WIDTH};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use crate::idf::httpd::Request;
    use crate::idf::EspErr;

    /// RGB565 as LVGL hands it to the panel (bytes swapped:
    /// `CONFIG_LV_COLOR_16_SWAP`).
    static SHADOW: Mutex<Vec<u16>> = Mutex::new(Vec::new());

    pub fn copy(x1: i32, y1: i32, x2: i32, y2: i32, px: &[u16]) {
        let mut s = SHADOW.lock().unwrap_or_else(|e| e.into_inner());
        if s.is_empty() {
            s.resize(WIDTH * HEIGHT, 0);
        }
        let w = (x2 - x1 + 1) as usize;
        for (row, y) in (y1..=y2).enumerate() {
            let (y, x1) = (y as usize, x1 as usize);
            if y >= HEIGHT || x1 >= WIDTH {
                continue;
            }
            let n = w.min(WIDTH - x1);
            s[y * WIDTH + x1..][..n].copy_from_slice(&px[row * w..][..n]);
        }
    }

    /// A 24-bit bottom-up BMP of the shadow buffer.
    pub(super) fn bmp() -> Vec<u8> {
        let s = SHADOW.lock().unwrap_or_else(|e| e.into_inner());
        let row = (WIDTH * 3 + 3) & !3;
        let data = row * HEIGHT;
        let mut b = Vec::with_capacity(54 + data);
        b.extend_from_slice(b"BM");
        b.extend_from_slice(&((54 + data) as u32).to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&54u32.to_le_bytes());
        b.extend_from_slice(&40u32.to_le_bytes());
        b.extend_from_slice(&(WIDTH as i32).to_le_bytes());
        b.extend_from_slice(&(HEIGHT as i32).to_le_bytes());
        b.extend_from_slice(&1u16.to_le_bytes());
        b.extend_from_slice(&24u16.to_le_bytes());
        b.extend_from_slice(&[0; 4]);
        b.extend_from_slice(&(data as u32).to_le_bytes());
        b.extend_from_slice(&[0; 16]);
        for y in (0..HEIGHT).rev() {
            for x in 0..WIDTH {
                let p = s.get(y * WIDTH + x).copied().unwrap_or(0).swap_bytes();
                let (r, g, bl) = ((p >> 11) & 0x1f, (p >> 5) & 0x3f, p & 0x1f);
                b.extend_from_slice(&[
                    ((bl << 3) | (bl >> 2)) as u8,
                    ((g << 2) | (g >> 4)) as u8,
                    ((r << 3) | (r >> 2)) as u8,
                ]);
            }
            b.resize(b.len() + row - WIDTH * 3, 0);
        }
        b
    }

    /// simulated presses, taken by the display task
    pub static PRESSES: AtomicU32 = AtomicU32::new(0);

    pub fn press(req: &mut Request) -> EspErr {
        let q = req.query(32);
        let bit = match q.as_deref().map(|q| q.to_bytes()) {
            Some(b"btn=1") => 1,
            Some(b"btn=2") => 2,
            _ => return req.send_err(crate::idf::httpd::err::E400_BAD_REQUEST, Some(c"btn=1|2")),
        };
        PRESSES.fetch_or(bit, Ordering::AcqRel);
        req.send(b"ok")
    }

    pub fn handler(req: &mut Request) -> EspErr {
        req.set_type(c"image/bmp");
        req.set_hdr(c"Cache-Control", c"no-store");
        req.send(&bmp())
    }
}

#[cfg(feature = "screenshot")]
pub use imp::{handler, press};

/// The simulated presses since the last call (bit 0: button 1, bit 1:
/// button 2); always 0 without the feature.
pub fn take_press() -> u32 {
    #[cfg(feature = "screenshot")]
    return imp::PRESSES.swap(0, std::sync::atomic::Ordering::AcqRel);
    #[cfg(not(feature = "screenshot"))]
    0
}

/// Every area LVGL flushes to the panel (`px`: (x2-x1+1)*(y2-y1+1)
/// pixels).
///
/// # Safety
/// `px` holds the area's pixels (or is NULL).
pub unsafe fn shadow_flush(x1: i32, y1: i32, x2: i32, y2: i32, px: *const u16) {
    #[cfg(feature = "screenshot")]
    if !px.is_null() && x2 >= x1 && y2 >= y1 {
        let n = ((x2 - x1 + 1) * (y2 - y1 + 1)) as usize;
        // SAFETY: the area's pixels (caller contract)
        let px = unsafe { core::slice::from_raw_parts(px, n) };
        imp::copy(x1, y1, x2, y2, px);
    }
    #[cfg(not(feature = "screenshot"))]
    let _ = (x1, y1, x2, y2, px);
}

#[cfg(all(test, feature = "screenshot"))]
mod tests {
    use super::*;

    #[test]
    fn areas_land_in_place_and_the_bmp_has_the_panel_size() {
        // one red pixel (RGB565 0xf800, bytes swapped as LVGL sends them)
        let px = [0xf800u16.swap_bytes(); 4];
        imp::copy(10, 20, 11, 21, &px);
        let b = imp::bmp();
        assert_eq!(&b[..2], b"BM");
        let row = (WIDTH * 3 + 3) & !3;
        assert_eq!(b.len(), 54 + row * HEIGHT);
        // (10, 20) from the top: BMP rows are bottom-up, BGR
        let off = 54 + (HEIGHT - 1 - 20) * row + 10 * 3;
        assert_eq!(&b[off..off + 3], &[0, 0, 255]);
    }
}
