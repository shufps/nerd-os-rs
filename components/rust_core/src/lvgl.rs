//! A small safe layer over LVGL 8.3 (the ESP-IDF component, C) for the
//! display code: the objects, labels, images and canvases the screens use.
//!
//! LVGL is not thread-safe: every call needs the display's LVGL lock. An
//! [`Lvgl`] token stands for that lock being held, and every operation takes
//! it. Object handles ([`Obj`]) are not `Copy`; deleting an object consumes
//! its handle. Children are deleted with their parent, so the display code
//! keeps no handles to the children of objects it deletes. Fonts and images
//! are `'static` C data (the converted assets).
//!
//! Colors: `CONFIG_LV_COLOR_DEPTH=16` with `CONFIG_LV_COLOR_16_SWAP`, so an
//! `lv_color_t` holds RGB565 with its bytes swapped ([`color`]).

use core::ffi::{c_void, CStr};
use core::marker::PhantomData;
use core::ptr::NonNull;

use crate::idf::raw;

pub use raw::{lv_font_t as Font, lv_img_dsc_t as Image};

/// The LVGL lock is held for the lifetime of this token.
pub struct Lvgl(PhantomData<*mut ()>);

impl Lvgl {
    /// # Safety
    /// The caller holds the display's LVGL lock until the token is dropped,
    /// and creates no second token meanwhile.
    pub unsafe fn assume_locked() -> Self {
        Lvgl(PhantomData)
    }
}

/// An LVGL color from 0xRRGGBB (`lv_color_hex()` for 16-bit swapped colors).
pub const fn color(rgb: u32) -> raw::lv_color_t {
    let (r, g, b) = ((rgb >> 16) & 0xff, (rgb >> 8) & 0xff, rgb & 0xff);
    let rgb565 = ((r >> 3) << 11 | (g >> 2) << 5 | (b >> 3)) as u16;
    raw::lv_color_t {
        full: rgb565.swap_bytes(),
    }
}

pub const WHITE: raw::lv_color_t = color(0xffffff);
pub const BLACK: raw::lv_color_t = color(0x000000);

/// `LV_SIZE_CONTENT`
pub const SIZE_CONTENT: raw::lv_coord_t =
    (2001 | (1 << raw::_LV_COORD_TYPE_SHIFT)) as raw::lv_coord_t;

/// `lv_pct(p)` for 0..=1000
pub const fn pct(p: i16) -> raw::lv_coord_t {
    p | (1 << raw::_LV_COORD_TYPE_SHIFT) as i16
}

/// `LV_PART_MAIN | LV_STATE_DEFAULT`
const MAIN: u32 = raw::LV_PART_MAIN | raw::LV_STATE_DEFAULT;

pub type Align = u8;
pub use raw::{
    LV_ALIGN_BOTTOM_MID as BOTTOM_MID, LV_ALIGN_CENTER as CENTER, LV_ALIGN_LEFT_MID as LEFT_MID,
    LV_ALIGN_RIGHT_MID as RIGHT_MID, LV_ALIGN_TOP_LEFT as TOP_LEFT, LV_ALIGN_TOP_MID as TOP_MID,
};
pub type TextAlign = u8;
pub use raw::{
    LV_TEXT_ALIGN_CENTER as TEXT_CENTER, LV_TEXT_ALIGN_LEFT as TEXT_LEFT,
    LV_TEXT_ALIGN_RIGHT as TEXT_RIGHT,
};

/// An LVGL object (screen, label, image, canvas, container).
pub struct Obj(NonNull<raw::lv_obj_t>);

// SAFETY: LVGL objects may be used from any task as long as the LVGL lock is
// held, which every method requires (the `Lvgl` token)
unsafe impl Send for Obj {}

fn obj(p: *mut raw::lv_obj_t) -> Obj {
    // LVGL aborts on an allocation failure (LV_ASSERT_MALLOC), so a created
    // object is never NULL
    Obj(NonNull::new(p).expect("LVGL object"))
}

impl Obj {
    fn p(&self) -> *mut raw::lv_obj_t {
        self.0.as_ptr()
    }

    /// The raw object.
    pub fn as_ptr(&self) -> *mut raw::lv_obj_t {
        self.p()
    }

    /// A new screen (no parent), not scrollable.
    pub fn screen(_: &Lvgl) -> Obj {
        // SAFETY: lock held (token); NULL parent creates a screen
        let s = obj(unsafe { raw::lv_obj_create(core::ptr::null_mut()) });
        // SAFETY: as above, a live object
        unsafe { raw::lv_obj_clear_flag(s.p(), raw::LV_OBJ_FLAG_SCROLLABLE) };
        s
    }

    /// A plain object (container) on `self`.
    pub fn container(&self, _: &Lvgl) -> Obj {
        // SAFETY: lock held (token); `self` is a live object
        obj(unsafe { raw::lv_obj_create(self.p()) })
    }

    pub fn label(&self, _: &Lvgl) -> Obj {
        // SAFETY: as container()
        obj(unsafe { raw::lv_label_create(self.p()) })
    }

    pub fn image(&self, _: &Lvgl) -> Obj {
        // SAFETY: as container()
        obj(unsafe { raw::lv_img_create(self.p()) })
    }

    pub fn canvas(&self, _: &Lvgl) -> Obj {
        // SAFETY: as container()
        obj(unsafe { raw::lv_canvas_create(self.p()) })
    }

    /// The active screen of the default display.
    pub fn active_screen(_: &Lvgl) -> Option<ObjRef> {
        // SAFETY: lock held (token)
        let p = unsafe { raw::lv_disp_get_scr_act(raw::lv_disp_get_default()) };
        NonNull::new(p).map(ObjRef)
    }

    /// Deletes the object and its children.
    pub fn delete(self, _: &Lvgl) {
        // SAFETY: lock held; the handle is consumed, so it cannot be used again
        unsafe { raw::lv_obj_del(self.p()) };
    }

    /// Whether `self` is a direct child of `parent`.
    pub fn is_child_of(&self, _: &Lvgl, parent: &Obj) -> bool {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_get_parent(self.p()) == parent.p() }
    }

    /// Deletes the children (the object stays).
    pub fn clean(&self, _: &Lvgl) {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_clean(self.p()) };
    }

    // --- geometry and flags

    pub fn size(&self, _: &Lvgl, w: raw::lv_coord_t, h: raw::lv_coord_t) -> &Self {
        // SAFETY: lock held; a live object
        unsafe {
            raw::lv_obj_set_width(self.p(), w);
            raw::lv_obj_set_height(self.p(), h);
        }
        self
    }

    pub fn pos(&self, _: &Lvgl, x: raw::lv_coord_t, y: raw::lv_coord_t) -> &Self {
        // SAFETY: lock held; a live object
        unsafe {
            raw::lv_obj_set_x(self.p(), x);
            raw::lv_obj_set_y(self.p(), y);
        }
        self
    }

    /// `lv_obj_set_align()`
    pub fn set_align(&self, _: &Lvgl, a: u32) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_align(self.p(), a as Align) };
        self
    }

    /// `lv_obj_align()`: alignment and offset.
    pub fn align(&self, _: &Lvgl, a: u32, x: raw::lv_coord_t, y: raw::lv_coord_t) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_align(self.p(), a as Align, x, y) };
        self
    }

    pub fn add_flag(&self, _: &Lvgl, f: u32) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_add_flag(self.p(), f) };
        self
    }

    pub fn clear_flag(&self, _: &Lvgl, f: u32) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_clear_flag(self.p(), f) };
        self
    }

    // --- styles (main part, default state)

    pub fn text_color(&self, _: &Lvgl, c: raw::lv_color_t) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_style_text_color(self.p(), c, MAIN) };
        self
    }

    pub fn text_opa(&self, _: &Lvgl, o: u8) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_style_text_opa(self.p(), o, MAIN) };
        self
    }

    pub fn text_align(&self, _: &Lvgl, a: u32) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_style_text_align(self.p(), a as TextAlign, MAIN) };
        self
    }

    pub fn font(&self, _: &Lvgl, f: &'static Font) -> &Self {
        // SAFETY: lock held; a live object; the font is static data
        unsafe { raw::lv_obj_set_style_text_font(self.p(), f, MAIN) };
        self
    }

    pub fn bg_color(&self, _: &Lvgl, c: raw::lv_color_t) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_style_bg_color(self.p(), c, MAIN) };
        self
    }

    pub fn bg_opa(&self, _: &Lvgl, o: u8) -> &Self {
        // SAFETY: lock held; a live object
        unsafe { raw::lv_obj_set_style_bg_opa(self.p(), o, MAIN) };
        self
    }

    pub fn border(&self, _: &Lvgl, width: raw::lv_coord_t, c: Option<raw::lv_color_t>) -> &Self {
        // SAFETY: lock held; a live object
        unsafe {
            raw::lv_obj_set_style_border_width(self.p(), width, MAIN);
            if let Some(c) = c {
                raw::lv_obj_set_style_border_color(self.p(), c, MAIN);
            }
        }
        self
    }

    /// `lv_obj_set_style_img_recolor()` and its opacity.
    pub fn img_recolor(&self, _: &Lvgl, c: raw::lv_color_t, opa: u8) -> &Self {
        // SAFETY: lock held; a live object
        unsafe {
            raw::lv_obj_set_style_img_recolor(self.p(), c, 0);
            raw::lv_obj_set_style_img_recolor_opa(self.p(), opa, 0);
        }
        self
    }

    // --- labels

    /// The label text (copied by LVGL).
    pub fn set_text(&self, _: &Lvgl, t: &CStr) -> &Self {
        // SAFETY: lock held; a live label; LVGL copies the string
        unsafe { raw::lv_label_set_text(self.p(), t.as_ptr()) };
        self
    }

    /// The label text from a Rust string (cut at a NUL byte).
    pub fn set_text_str(&self, l: &Lvgl, t: &str) -> &Self {
        let t = crate::cutil::until_nul(t.as_bytes());
        let c = std::ffi::CString::new(t).unwrap_or_default();
        self.set_text(l, &c)
    }

    pub fn long_mode_wrap(&self, _: &Lvgl) -> &Self {
        // SAFETY: lock held; a live label
        unsafe { raw::lv_label_set_long_mode(self.p(), raw::LV_LABEL_LONG_WRAP as u8) };
        self
    }

    // --- images

    pub fn set_src(&self, _: &Lvgl, img: &'static Image) -> &Self {
        // SAFETY: lock held; a live image; the descriptor is static data
        unsafe { raw::lv_img_set_src(self.p(), img as *const Image as *const c_void) };
        self
    }

    /// Recolors every image child (the generic theme's tint).
    pub fn tint_images(&self, _: &Lvgl, c: raw::lv_color_t, opa: u8) {
        // SAFETY: lock held; children of a live object, only their style is
        // changed (no handles kept)
        unsafe {
            for i in 0..raw::lv_obj_get_child_cnt(self.p()) {
                let child = raw::lv_obj_get_child(self.p(), i as i32);
                if !child.is_null() && raw::lv_obj_check_type(child, &raw::lv_img_class) {
                    raw::lv_obj_set_style_img_recolor(child, c, 0);
                    raw::lv_obj_set_style_img_recolor_opa(child, opa, 0);
                }
            }
        }
    }

    /// Loads this screen with an animation (`lv_scr_load_anim()`); the old
    /// screen is kept.
    pub fn load(&self, _: &Lvgl, anim: Anim, time: u32, delay: u32) {
        let a = match anim {
            Anim::None => raw::lv_scr_load_anim_t_LV_SCR_LOAD_ANIM_NONE,
            Anim::FadeOn => raw::lv_scr_load_anim_t_LV_SCR_LOAD_ANIM_FADE_ON,
            Anim::MoveLeft => raw::lv_scr_load_anim_t_LV_SCR_LOAD_ANIM_MOVE_LEFT,
        };
        // SAFETY: lock held; a live screen, not deleted afterwards
        unsafe { raw::lv_scr_load_anim(self.p(), a, time, delay, false) };
    }

    // --- events

    /// `cb` for `LV_EVENT_SCREEN_LOADED` of this screen.
    pub fn on_screen_loaded(&self, _: &Lvgl, cb: unsafe extern "C" fn(*mut raw::lv_event_t)) {
        // SAFETY: lock held; a live object; no user data
        unsafe {
            raw::lv_obj_add_event_cb(
                self.p(),
                Some(cb),
                raw::lv_event_code_t_LV_EVENT_SCREEN_LOADED,
                core::ptr::null_mut(),
            )
        };
    }
}

/// The screen load animations the display uses.
#[derive(Clone, Copy)]
pub enum Anim {
    None,
    FadeOn,
    MoveLeft,
}

/// A borrowed handle to an object this code does not own (the active
/// screen): only for creating children on it.
pub struct ObjRef(NonNull<raw::lv_obj_t>);

impl ObjRef {
    pub fn container(&self, _: &Lvgl) -> Obj {
        // SAFETY: lock held; the active screen is live while the lock is held
        obj(unsafe { raw::lv_obj_create(self.0.as_ptr()) })
    }
}

/// A canvas and its buffer (`w`×`w` true-color pixels in PSRAM). The
/// buffer is never freed: the canvas object may outlive this value (it is
/// deleted with its screen), so it must not point to freed memory.
pub struct Canvas {
    obj: Obj,
    w: i16,
}

impl Canvas {
    /// A square canvas of `w` pixels on `parent`; None if the buffer cannot
    /// be allocated.
    pub fn new(l: &Lvgl, parent: &Obj, w: i16) -> Option<Canvas> {
        let n = (w as usize) * (w as usize);
        let mut buf = Vec::new();
        buf.try_reserve_exact(n).ok()?;
        buf.resize(n, BLACK);
        let buf: &'static mut [raw::lv_color_t] = Vec::leak(buf);
        let obj = parent.canvas(l);
        // SAFETY: lock held; the buffer is 'static (leaked) and holds w*w pixels
        unsafe {
            raw::lv_canvas_set_buffer(
                obj.p(),
                buf.as_mut_ptr() as *mut c_void,
                w,
                w,
                raw::LV_IMG_CF_TRUE_COLOR as u8,
            )
        };
        Some(Canvas { obj, w })
    }

    pub fn obj(&self) -> &Obj {
        &self.obj
    }

    pub fn width(&self) -> i16 {
        self.w
    }

    /// A filled rectangle without border.
    pub fn rect(&mut self, _: &Lvgl, x: i16, y: i16, w: i16, h: i16, c: raw::lv_color_t) {
        let mut d = raw::lv_draw_rect_dsc_t::default();
        // SAFETY: lock held; `d` a live local; draws into the canvas' static
        // buffer (LVGL clips to it)
        unsafe {
            raw::lv_draw_rect_dsc_init(&mut d);
            d.bg_color = c;
            d.border_opa = raw::LV_OPA_TRANSP as u8;
            raw::lv_canvas_draw_rect(self.obj.p(), x, y, w, h, &d);
        }
    }
}
