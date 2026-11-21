//! The display screens (port of `main/displays/ui.cpp` and the update
//! functions of `displayDriver.cpp`): the splash, portal, mining, settings,
//! BTC and global stats screens, the log, QR and power-off screens, the
//! error and image overlays, and the values shown on them, read directly
//! from the Rust modules. The display driver (C++: panel, LVGL task, state
//! machine, buttons) calls in through `ui_rs.h`, always with its LVGL lock
//! held, and loads the screens it gets from here.
//!
//! Differences to the C++ code:
//! - the handles of objects that are deleted with their screen (the
//!   "Connecting..." label of the second splash screen, an overlay on a
//!   splash screen) are dropped with it instead of dangling;
//! - the network icon is changed under the LVGL lock.

use core::ffi::{c_char, c_int, CStr};
use std::sync::Mutex;

use crate::display_text as text;
use crate::idf::raw;
use crate::lvgl::{self as lv, color, Canvas, Font, Image, Lvgl, Obj};

/// Where the display code links the converted assets (C arrays).
macro_rules! asset {
    ($name:ident) => {{
        // SAFETY: a `const` LVGL font/image descriptor defined in C (the
        // converted assets), immutable static data
        unsafe { &$name }
    }};
}

#[allow(non_upper_case_globals)]
extern "C" {
    static ui_font_DigitalNumbers16: Font;
    static ui_font_DigitalNumbers28: Font;
    static ui_font_OpenSansBold13: Font;
    static ui_font_OpenSansBold14: Font;
    static ui_font_OpenSansBold24: Font;
    static ui_font_OpenSansBold45: Font;
    static ui_font_vt323_21: Font;
    static ui_font_vt323_35: Font;
    static ui_img_eth_png: Image;
    static ui_img_wifi_png: Image;
    static ui_img_can_png: Image;
    static ui_img_safe_png: Image;
    static ui_img_found_block_png: Image;
}

// the background images of all boards; only the built board's are used
#[allow(dead_code)]
extern "C" {
    static ui_img_NerdOctaxeGamma_initscreen2_png: Image;
    static ui_img_NerdOctaxeGamma_miningscreen2_png: Image;
    static ui_img_NerdOctaxeGamma_portalscreen_png: Image;
    static ui_img_NerdOctaxeGamma_btcscreen_png: Image;
    static ui_img_NerdOctaxeGamma_settingsscreen_png: Image;
    static ui_img_NerdOctaxeGamma_splashscreen2_png: Image;
    static ui_img_NerdOctaxeGamma_globalStats_png: Image;
    static ui_img_NerdQaxePlus_initscreen2_png: Image;
    static ui_img_NerdQaxePlus_miningscreen2_png: Image;
    static ui_img_NerdQaxePlus_portalscreen_png: Image;
    static ui_img_NerdQaxePlus_btcscreen_png: Image;
    static ui_img_NerdQaxePlus_settingsscreen_png: Image;
    static ui_img_NerdQaxePlus_splashscreen2_png: Image;
    static ui_img_NerdQaxePlus_globalStats_png: Image;
    static ui_img_NerdEko_initscreen2_png: Image;
    static ui_img_NerdEko_miningscreen2_png: Image;
    static ui_img_NerdEko_portalscreen_png: Image;
    static ui_img_NerdEko_btcscreen_png: Image;
    static ui_img_NerdEko_settingsscreen_png: Image;
    static ui_img_NerdEko_splashscreen2_png: Image;
    static ui_img_NerdEko_globalStats_png: Image;
    static ui_img_NerdQaxePlus2_initscreen2_png: Image;
    static ui_img_NerdQaxePlus2_miningscreen2_png: Image;
    static ui_img_NerdQaxePlus2_portalscreen_png: Image;
    static ui_img_NerdQaxePlus2_btcscreen_png: Image;
    static ui_img_NerdQaxePlus2_settingsscreen_png: Image;
    static ui_img_NerdQaxePlus2_splashscreen2_png: Image;
    static ui_img_NerdQaxePlus2_globalStats_png: Image;
    static ui_img_NerdAxe_initscreen2_png: Image;
    static ui_img_NerdAxe_miningscreen2_png: Image;
    static ui_img_NerdAxe_portalscreen_png: Image;
    static ui_img_NerdAxe_btcscreen_png: Image;
    static ui_img_NerdAxe_settingsscreen_png: Image;
    static ui_img_NerdAxe_splashscreen2_png: Image;
    static ui_img_NerdAxe_globalStats_png: Image;
    static ui_img_NerdAxeGamma_initscreen2_png: Image;
    static ui_img_NerdAxeGamma_miningscreen2_png: Image;
    static ui_img_NerdAxeGamma_portalscreen_png: Image;
    static ui_img_NerdAxeGamma_btcscreen_png: Image;
    static ui_img_NerdAxeGamma_settingsscreen_png: Image;
    static ui_img_NerdAxeGamma_splashscreen2_png: Image;
    static ui_img_NerdAxeGamma_globalStats_png: Image;
    static ui_img_NerdAxeGaia_initscreen2_png: Image;
    static ui_img_NerdAxeGaia_miningscreen2_png: Image;
    static ui_img_NerdAxeGaia_portalscreen_png: Image;
    static ui_img_NerdAxeGaia_btcscreen_png: Image;
    static ui_img_NerdAxeGaia_settingsscreen_png: Image;
    static ui_img_NerdAxeGaia_splashscreen2_png: Image;
    static ui_img_NerdAxeGaia_globalStats_png: Image;
    static ui_img_NerdQX_initscreen2_png: Image;
    static ui_img_NerdQX_miningscreen2_png: Image;
    static ui_img_NerdQX_portalscreen_png: Image;
    static ui_img_NerdQX_btcscreen_png: Image;
    static ui_img_NerdQX_settingsscreen_png: Image;
    static ui_img_NerdQX_splashscreen2_png: Image;
    static ui_img_NerdQX_globalStats_png: Image;
    static ui_img_NerdOctaxePlus_initscreen2_png: Image;
    static ui_img_NerdOctaxePlus_miningscreen2_png: Image;
    static ui_img_NerdOctaxePlus_portalscreen_png: Image;
    static ui_img_NerdOctaxePlus_btcscreen_png: Image;
    static ui_img_NerdOctaxePlus_settingsscreen_png: Image;
    static ui_img_NerdOctaxePlus_splashscreen2_png: Image;
    static ui_img_NerdOctaxePlus_globalStats_png: Image;
    static ui_img_Generic_initscreen2_png: Image;
    static ui_img_Generic_miningscreen2_png: Image;
    static ui_img_Generic_portalscreen_png: Image;
    static ui_img_Generic_btcscreen_png: Image;
    static ui_img_Generic_settingsscreen_png: Image;
    static ui_img_Generic_splashscreen2_png: Image;
    static ui_img_Generic_globalStats_png: Image;
    static ui_img_NerdHaxeGamma_initscreen2_png: Image;
    static ui_img_NerdHaxeGamma_miningscreen2_png: Image;
    static ui_img_NerdHaxeGamma_portalscreen_png: Image;
    static ui_img_NerdHaxeGamma_btcscreen_png: Image;
    static ui_img_NerdHaxeGamma_settingsscreen_png: Image;
    static ui_img_NerdHaxeGamma_splashscreen2_png: Image;
    static ui_img_NerdHaxeGamma_globalStats_png: Image;
}

/// The donation address of the generic splash screen (`CONFIG_DONATE_ADDR`).
const DONATE_ADDR: &CStr = c"bc1q7n70rumyv6lvu8avpml0c3uggvssfu52egum3q";

/// The background images of a board.
struct Theme {
    init: &'static Image,
    mining: &'static Image,
    portal: &'static Image,
    btc: &'static Image,
    settings: &'static Image,
    splash: &'static Image,
    global: &'static Image,
    /// the Q1370/Q1373 theme: tinted, with device name and donation QR
    generic: bool,
}

// SAFETY: only references to immutable static image descriptors (the
// converted assets), which are never written
unsafe impl Send for Theme {}

/// The theme of the board the firmware is built for (as `board.cpp`); only
/// that board's images are linked (`cfg(board)`, set by build.rs).
fn theme() -> Theme {
    #[cfg(board = "NERDOCTAXEGAMMA")]
    let t = Theme {
        init: asset!(ui_img_NerdOctaxeGamma_initscreen2_png),
        mining: asset!(ui_img_NerdOctaxeGamma_miningscreen2_png),
        portal: asset!(ui_img_NerdOctaxeGamma_portalscreen_png),
        btc: asset!(ui_img_NerdOctaxeGamma_btcscreen_png),
        settings: asset!(ui_img_NerdOctaxeGamma_settingsscreen_png),
        splash: asset!(ui_img_NerdOctaxeGamma_splashscreen2_png),
        global: asset!(ui_img_NerdOctaxeGamma_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDQAXEPLUS")]
    let t = Theme {
        init: asset!(ui_img_NerdQaxePlus_initscreen2_png),
        mining: asset!(ui_img_NerdQaxePlus_miningscreen2_png),
        portal: asset!(ui_img_NerdQaxePlus_portalscreen_png),
        btc: asset!(ui_img_NerdQaxePlus_btcscreen_png),
        settings: asset!(ui_img_NerdQaxePlus_settingsscreen_png),
        splash: asset!(ui_img_NerdQaxePlus_splashscreen2_png),
        global: asset!(ui_img_NerdQaxePlus_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDEKO")]
    let t = Theme {
        init: asset!(ui_img_NerdEko_initscreen2_png),
        mining: asset!(ui_img_NerdEko_miningscreen2_png),
        portal: asset!(ui_img_NerdEko_portalscreen_png),
        btc: asset!(ui_img_NerdEko_btcscreen_png),
        settings: asset!(ui_img_NerdEko_settingsscreen_png),
        splash: asset!(ui_img_NerdEko_splashscreen2_png),
        global: asset!(ui_img_NerdEko_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDQAXEPLUS2")]
    let t = Theme {
        init: asset!(ui_img_NerdQaxePlus2_initscreen2_png),
        mining: asset!(ui_img_NerdQaxePlus2_miningscreen2_png),
        portal: asset!(ui_img_NerdQaxePlus2_portalscreen_png),
        btc: asset!(ui_img_NerdQaxePlus2_btcscreen_png),
        settings: asset!(ui_img_NerdQaxePlus2_settingsscreen_png),
        splash: asset!(ui_img_NerdQaxePlus2_splashscreen2_png),
        global: asset!(ui_img_NerdQaxePlus2_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDAXE")]
    let t = Theme {
        init: asset!(ui_img_NerdAxe_initscreen2_png),
        mining: asset!(ui_img_NerdAxe_miningscreen2_png),
        portal: asset!(ui_img_NerdAxe_portalscreen_png),
        btc: asset!(ui_img_NerdAxe_btcscreen_png),
        settings: asset!(ui_img_NerdAxe_settingsscreen_png),
        splash: asset!(ui_img_NerdAxe_splashscreen2_png),
        global: asset!(ui_img_NerdAxe_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDAXEGAMMA")]
    let t = Theme {
        init: asset!(ui_img_NerdAxeGamma_initscreen2_png),
        mining: asset!(ui_img_NerdAxeGamma_miningscreen2_png),
        portal: asset!(ui_img_NerdAxeGamma_portalscreen_png),
        btc: asset!(ui_img_NerdAxeGamma_btcscreen_png),
        settings: asset!(ui_img_NerdAxeGamma_settingsscreen_png),
        splash: asset!(ui_img_NerdAxeGamma_splashscreen2_png),
        global: asset!(ui_img_NerdAxeGamma_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDAXEGAIA")]
    let t = Theme {
        init: asset!(ui_img_NerdAxeGaia_initscreen2_png),
        mining: asset!(ui_img_NerdAxeGaia_miningscreen2_png),
        portal: asset!(ui_img_NerdAxeGaia_portalscreen_png),
        btc: asset!(ui_img_NerdAxeGaia_btcscreen_png),
        settings: asset!(ui_img_NerdAxeGaia_settingsscreen_png),
        splash: asset!(ui_img_NerdAxeGaia_splashscreen2_png),
        global: asset!(ui_img_NerdAxeGaia_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDQX")]
    let t = Theme {
        init: asset!(ui_img_NerdQX_initscreen2_png),
        mining: asset!(ui_img_NerdQX_miningscreen2_png),
        portal: asset!(ui_img_NerdQX_portalscreen_png),
        btc: asset!(ui_img_NerdQX_btcscreen_png),
        settings: asset!(ui_img_NerdQX_settingsscreen_png),
        splash: asset!(ui_img_NerdQX_splashscreen2_png),
        global: asset!(ui_img_NerdQX_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDOCTAXEPLUS")]
    let t = Theme {
        init: asset!(ui_img_NerdOctaxePlus_initscreen2_png),
        mining: asset!(ui_img_NerdOctaxePlus_miningscreen2_png),
        portal: asset!(ui_img_NerdOctaxePlus_portalscreen_png),
        btc: asset!(ui_img_NerdOctaxePlus_btcscreen_png),
        settings: asset!(ui_img_NerdOctaxePlus_settingsscreen_png),
        splash: asset!(ui_img_NerdOctaxePlus_splashscreen2_png),
        global: asset!(ui_img_NerdOctaxePlus_globalStats_png),
        generic: false,
    };
    #[cfg(board = "NERDHAXEGAMMA")]
    let t = Theme {
        init: asset!(ui_img_NerdHaxeGamma_initscreen2_png),
        mining: asset!(ui_img_NerdHaxeGamma_miningscreen2_png),
        portal: asset!(ui_img_NerdHaxeGamma_portalscreen_png),
        btc: asset!(ui_img_NerdHaxeGamma_btcscreen_png),
        settings: asset!(ui_img_NerdHaxeGamma_settingsscreen_png),
        splash: asset!(ui_img_NerdHaxeGamma_splashscreen2_png),
        global: asset!(ui_img_NerdHaxeGamma_globalStats_png),
        generic: false,
    };
    // Q1370/Q1373 (and host builds)
    #[cfg(not(any(
        board = "NERDOCTAXEGAMMA",
        board = "NERDQAXEPLUS",
        board = "NERDEKO",
        board = "NERDQAXEPLUS2",
        board = "NERDAXE",
        board = "NERDAXEGAMMA",
        board = "NERDAXEGAIA",
        board = "NERDQX",
        board = "NERDOCTAXEPLUS",
        board = "NERDHAXEGAMMA"
    )))]
    let t = Theme {
        init: asset!(ui_img_Generic_initscreen2_png),
        mining: asset!(ui_img_Generic_miningscreen2_png),
        portal: asset!(ui_img_Generic_portalscreen_png),
        btc: asset!(ui_img_Generic_btcscreen_png),
        settings: asset!(ui_img_Generic_settingsscreen_png),
        splash: asset!(ui_img_Generic_splashscreen2_png),
        global: asset!(ui_img_Generic_globalStats_png),
        generic: true,
    };
    t
}

/// A label as the screens define them: position, alignment, initial text,
/// color, text alignment and font.
struct L {
    x: i16,
    y: i16,
    align: u32,
    text: &'static CStr,
    color: u32,
    talign: u32,
    font: &'static Font,
}

fn label(l: &Lvgl, parent: &Obj, s: &L) -> Obj {
    let o = parent.label(l);
    o.size(l, lv::SIZE_CONTENT, lv::SIZE_CONTENT)
        .pos(l, s.x, s.y)
        .set_align(l, s.align)
        .set_text(l, s.text)
        .text_color(l, color(s.color))
        .text_opa(l, 255)
        .text_align(l, s.talign)
        .font(l, s.font);
    o
}

/// The full-screen background image.
fn background(l: &Lvgl, screen: &Obj, img: &'static Image) {
    let o = screen.image(l);
    o.set_src(l, img)
        .size(l, lv::SIZE_CONTENT, lv::SIZE_CONTENT)
        .set_align(l, lv::CENTER)
        .add_flag(l, raw::LV_OBJ_FLAG_ADV_HITTEST)
        .clear_flag(l, raw::LV_OBJ_FLAG_SCROLLABLE);
}

/// Forgets the image cache entry of an image no longer shown.
fn uncache(_: &Lvgl, img: &'static Image) {
    // SAFETY: lock held (token); the source is static data
    unsafe { raw::lv_img_cache_invalidate_src(img as *const Image as *const core::ffi::c_void) };
}

extern "C" {
    /// the display driver: a screen load animation ended (display_api.cpp)
    fn disp_screen_loaded(driver: *mut core::ffi::c_void);
}

/// The display driver that created the screens (`ui_init()`).
static DRIVER: core::sync::atomic::AtomicPtr<core::ffi::c_void> =
    core::sync::atomic::AtomicPtr::new(core::ptr::null_mut());

unsafe extern "C" fn on_screen_loaded(_: *mut raw::lv_event_t) {
    let d = DRIVER.load(core::sync::atomic::Ordering::Acquire);
    if !d.is_null() {
        // SAFETY: the driver that created the screens (it is never freed);
        // runs in the LVGL task, under its lock
        unsafe { disp_screen_loaded(d) };
    }
}

struct Mining {
    vinput: Obj,
    vcore: Obj,
    current: Obj,
    power: Obj,
    efficiency: Obj,
    temp: Obj,
    time: Obj,
    ip: Obj,
    best_diff: Obj,
    hashrate: Obj,
    rpm: Obj,
    net: Obj,
}

struct Settings {
    ip: Obj,
    best_diff: Obj,
    pool_nr: Obj,
    vcore: Obj,
    freq: Obj,
    fan: Obj,
    pool: Obj,
    hashrate: Obj,
    shares: Obj,
    port: Obj,
}

struct Btc {
    price: Obj,
    hashrate: Obj,
    temp: Obj,
}

struct Global {
    halving_percent: Obj,
    block: Obj,
    blocks_to_halving: Obj,
    difficulty: Obj,
    hashrate: Obj,
    low_fee: Obj,
    mid_fee: Obj,
    high_fee: Obj,
}

/// The screens and the objects whose values change.
struct Ui {
    theme: Theme,
    splash1: Option<Obj>,
    splash2: Option<Obj>,
    /// "Connecting..." on the second splash screen
    connect: Option<Obj>,
    portal: Obj,
    ssid: Obj,
    mining_scr: Obj,
    mining: Mining,
    settings_scr: Obj,
    settings: Settings,
    btc_scr: Obj,
    btc: Btc,
    global_scr: Obj,
    global: Global,
    log: Option<(Obj, Obj)>,
    qr: Option<(Obj, Canvas)>,
    power_off: Option<Obj>,
    err_overlay: Option<Obj>,
    image_overlay: Option<Obj>,
    /// shown once, then refreshed only while visible (`m_btcPrice`)
    btc_price: u32,
    /// as `btc_price` (`m_blockHeight`)
    block_height: u32,
}

static UI: Mutex<Option<Ui>> = Mutex::new(None);

fn f13() -> &'static Font {
    asset!(ui_font_OpenSansBold13)
}
fn f14() -> &'static Font {
    asset!(ui_font_OpenSansBold14)
}
fn f24() -> &'static Font {
    asset!(ui_font_OpenSansBold24)
}
fn f45() -> &'static Font {
    asset!(ui_font_OpenSansBold45)
}
fn montserrat10() -> &'static Font {
    // SAFETY: LVGL's built-in font, immutable static data
    unsafe { &raw::lv_font_montserrat_10 }
}

const GREY: u32 = 0xDEDADE;
const GREY2: u32 = 0xDEDEDE;

fn board_model(asic: bool) -> &'static CStr {
    let i = crate::board::ffi::info();
    // SAFETY: profile strings of the board info (c"..." literals)
    unsafe { crate::board::ffi::static_text(if asic { i.asic_model } else { i.device_model }) }
}

impl Ui {
    fn new(l: &Lvgl) -> Ui {
        let theme = theme();

        // the first splash screen
        let splash1 = Obj::screen(l);
        background(l, &splash1, theme.init);
        uncache(l, theme.splash);
        if theme.generic {
            let d = splash1.label(l);
            d.set_text(l, board_model(false))
                .text_color(l, lv::WHITE)
                .font(l, f45());
            d.align(l, lv::CENTER, 0, 0);
        }

        // the second splash screen
        let splash2 = Obj::screen(l);
        background(l, &splash2, theme.splash);
        let connect = label(
            l,
            &splash2,
            &L {
                x: -31,
                y: -40,
                align: lv::RIGHT_MID,
                text: c"Connecting...",
                color: GREY,
                talign: lv::TEXT_RIGHT,
                font: f13(),
            },
        );
        uncache(l, theme.init);
        if theme.generic {
            generic_splash2(l, &splash2);
        }

        // the access point portal
        let portal = Obj::screen(l);
        background(l, &portal, theme.portal);
        let ssid = label(
            l,
            &portal,
            &L {
                x: 75,
                y: 52,
                align: lv::LEFT_MID,
                text: c"NERDAXE_XXXX",
                color: GREY,
                talign: lv::TEXT_LEFT,
                font: f13(),
            },
        );
        portal.on_screen_loaded(l, on_screen_loaded);
        uncache(l, theme.init);

        let (mining_scr, mining) = mining_screen(l, &theme);
        let (settings_scr, settings) = settings_screen(l, &theme);
        let (btc_scr, btc) = btc_screen(l, &theme);
        let (global_scr, global) = global_screen(l, &theme);

        Ui {
            theme,
            splash1: Some(splash1),
            splash2: Some(splash2),
            connect: Some(connect),
            portal,
            ssid,
            mining_scr,
            mining,
            settings_scr,
            settings,
            btc_scr,
            btc,
            global_scr,
            global,
            log: None,
            qr: None,
            power_off: None,
            err_overlay: None,
            image_overlay: None,
            btc_price: 0,
            block_height: 0,
        }
    }
}

/// The generic theme's second splash screen: device name and donation QR.
fn generic_splash2(l: &Lvgl, splash2: &Obj) {
    let d = splash2.label(l);
    d.set_text(l, board_model(false))
        .text_color(l, lv::WHITE)
        .font(l, f45());
    d.align(l, lv::LEFT_MID, 8, -15);

    let Some(buf) = encode_qr(DONATE_ADDR, 6) else {
        return;
    };
    let qr = Qr(&buf);
    let n = qr.size();
    const QUIET: i32 = 3;
    let scale = (100 / (n + 2 * QUIET)).max(2);
    let w = ((n + 2 * QUIET) * scale) as i16;
    let Some(mut canvas) = Canvas::new(l, splash2, w) else {
        return;
    };
    canvas.obj().align(l, lv::RIGHT_MID, -24, 30);
    draw_qr(l, &mut canvas, &qr, QUIET, scale);
    // the canvas lives with its screen (its buffer is never freed)
}

/// A QR code in `qrcodegen`'s buffer format.
struct Qr<'a>(&'a [u8]);

impl Qr<'_> {
    fn size(&self) -> i32 {
        // SAFETY: a qrcodegen buffer (the encoder's output)
        unsafe { qrcodegen_getSize(self.0.as_ptr()) }
    }
    fn module(&self, x: i32, y: i32) -> bool {
        // SAFETY: as size(); the library checks the coordinates
        unsafe { qrcodegen_getModule(self.0.as_ptr(), x, y) }
    }
}

extern "C" {
    fn qrcodegen_getSize(qrcode: *const u8) -> c_int;
    fn qrcodegen_getModule(qrcode: *const u8, x: c_int, y: c_int) -> bool;
    #[allow(clippy::too_many_arguments)]
    fn qrcodegen_encodeText(
        text: *const c_char,
        temp: *mut u8,
        qrcode: *mut u8,
        ecl: c_int,
        min_version: c_int,
        max_version: c_int,
        mask: c_int,
        boost_ecl: bool,
    ) -> bool;
}

/// `text` as a QR code (qrcodegen buffer) of at most `max_version`
/// (medium error correction).
fn encode_qr(text: &CStr, max_version: i32) -> Option<Vec<u8>> {
    // qrcodegen_BUFFER_LEN_FOR_VERSION(n) = ((n*4+17)^2 + 7) / 8 + 1
    let side = (max_version * 4 + 17) as usize;
    let len = (side * side).div_ceil(8) + 1;
    let mut tmp = vec![0u8; len];
    let mut qr = vec![0u8; len];
    // SAFETY: both buffers hold the length for `max_version`; `text` is
    // NUL-terminated. ECC medium = 1, VERSION_MIN = 1, MASK_AUTO = -1
    let ok = unsafe {
        qrcodegen_encodeText(
            text.as_ptr(),
            tmp.as_mut_ptr(),
            qr.as_mut_ptr(),
            1,
            1,
            max_version,
            -1,
            true,
        )
    };
    ok.then_some(qr)
}

/// White background, black modules with a quiet zone of `quiet` modules.
fn draw_qr(l: &Lvgl, c: &mut Canvas, qr: &Qr<'_>, quiet: i32, scale: i32) {
    let w = c.width();
    c.rect(l, 0, 0, w, w, lv::WHITE);
    let n = qr.size();
    for y in 0..n {
        for x in 0..n {
            if qr.module(x, y) {
                let (px, py) = ((quiet + x) * scale, (quiet + y) * scale);
                c.rect(
                    l,
                    px as i16,
                    py as i16,
                    scale as i16,
                    scale as i16,
                    lv::BLACK,
                );
            }
        }
    }
}

fn mining_screen(l: &Lvgl, t: &Theme) -> (Obj, Mining) {
    let s = Obj::screen(l);
    background(l, &s, t.mining);
    let dn28 = asset!(ui_font_DigitalNumbers28);
    let dn16 = asset!(ui_font_DigitalNumbers16);
    let lb = |spec: L| label(l, &s, &spec);
    let vinput = lb(L {
        x: 234,
        y: -34,
        align: lv::LEFT_MID,
        text: c"0V",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let vcore = lb(L {
        x: 234,
        y: -12,
        align: lv::LEFT_MID,
        text: c"0mV",
        color: GREY2,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let current = lb(L {
        x: 234,
        y: 10,
        align: lv::LEFT_MID,
        text: c"0.000mA",
        color: GREY2,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let power = lb(L {
        x: 234,
        y: 32,
        align: lv::LEFT_MID,
        text: c"0W",
        color: GREY2,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let efficiency = lb(L {
        x: -43,
        y: 61,
        align: lv::RIGHT_MID,
        text: c"0.0",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: dn16,
    });
    let temp = lb(L {
        x: -139,
        y: 24,
        align: lv::RIGHT_MID,
        text: c"20",
        color: GREY,
        talign: lv::TEXT_RIGHT,
        font: f14(),
    });
    let time = lb(L {
        x: -190,
        y: 0,
        align: lv::RIGHT_MID,
        text: c"0d 0h 0m",
        color: 0xDEEE00,
        talign: lv::TEXT_RIGHT,
        font: f13(),
    });
    let ip = lb(L {
        x: -16,
        y: -77,
        align: lv::CENTER,
        text: c"0.0.0.0",
        color: 0x000000,
        talign: lv::TEXT_CENTER,
        font: montserrat10(),
    });
    let best_diff = lb(L {
        x: 34,
        y: 21,
        align: lv::LEFT_MID,
        text: c"0M",
        color: 0x000000,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let hashrate = lb(L {
        x: -208,
        y: 59,
        align: lv::RIGHT_MID,
        text: c"0,0",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: dn28,
    });
    let rpm = lb(L {
        x: 20,
        y: -9,
        align: lv::CENTER,
        text: c"0",
        color: 0x000000,
        talign: lv::TEXT_LEFT,
        font: f13(),
    });
    let asic = lb(L {
        x: 111,
        y: -66,
        align: lv::CENTER,
        text: c"",
        color: 0x000000,
        talign: lv::TEXT_CENTER,
        font: f14(),
    });
    asic.set_text(l, board_model(true));
    s.on_screen_loaded(l, on_screen_loaded);

    let net = s.image(l);
    net.set_src(l, asset!(ui_img_wifi_png))
        .size(l, 14, 14)
        .pos(l, 41 + 7, -83 + 7)
        .set_align(l, lv::CENTER)
        .clear_flag(l, raw::LV_OBJ_FLAG_SCROLLABLE);

    if t.generic {
        let d = s.label(l);
        d.set_text(l, board_model(false))
            .text_color(l, lv::WHITE)
            .font(l, f24());
        d.align(l, lv::LEFT_MID, 8, -25);
    }
    (
        s,
        Mining {
            vinput,
            vcore,
            current,
            power,
            efficiency,
            temp,
            time,
            ip,
            best_diff,
            hashrate,
            rpm,
            net,
        },
    )
}

fn settings_screen(l: &Lvgl, t: &Theme) -> (Obj, Settings) {
    let s = Obj::screen(l);
    background(l, &s, t.settings);
    let dn28 = asset!(ui_font_DigitalNumbers28);
    let lb = |spec: L| label(l, &s, &spec);
    let ip = lb(L {
        x: -16,
        y: -77,
        align: lv::CENTER,
        text: c"--",
        color: 0x000000,
        talign: lv::TEXT_CENTER,
        font: montserrat10(),
    });
    let best_diff = lb(L {
        x: 34,
        y: 21,
        align: lv::LEFT_MID,
        text: c"--",
        color: 0x000000,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let pool_nr = lb(L {
        x: 210,
        y: -46,
        align: lv::LEFT_MID,
        text: c"",
        color: 0xffffff,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let vcore = lb(L {
        x: 43,
        y: -45,
        align: lv::LEFT_MID,
        text: c"--",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let freq = lb(L {
        x: 43,
        y: -25,
        align: lv::LEFT_MID,
        text: c"--",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let fan = lb(L {
        x: 43,
        y: -5,
        align: lv::LEFT_MID,
        text: c"AUTO",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let pool = lb(L {
        x: 169,
        y: -9,
        align: lv::LEFT_MID,
        text: c"--",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f13(),
    });
    let hashrate = lb(L {
        x: -208,
        y: 59,
        align: lv::RIGHT_MID,
        text: c"--",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: dn28,
    });
    let shares = lb(L {
        x: -23,
        y: 58,
        align: lv::RIGHT_MID,
        text: c"0/0",
        color: 0x000000,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let port = lb(L {
        x: 211,
        y: 13,
        align: lv::LEFT_MID,
        text: c"--",
        color: GREY,
        talign: lv::TEXT_LEFT,
        font: f13(),
    });
    s.on_screen_loaded(l, on_screen_loaded);
    (
        s,
        Settings {
            ip,
            best_diff,
            pool_nr,
            vcore,
            freq,
            fan,
            pool,
            hashrate,
            shares,
            port,
        },
    )
}

fn btc_screen(l: &Lvgl, t: &Theme) -> (Obj, Btc) {
    let s = Obj::screen(l);
    s.bg_color(l, lv::BLACK).bg_opa(l, 255);
    background(l, &s, t.btc);
    let lb = |spec: L| label(l, &s, &spec);
    let price = lb(L {
        x: 30,
        y: 47,
        align: lv::LEFT_MID,
        text: c"0$",
        color: 0xFFFFFF,
        talign: lv::TEXT_LEFT,
        font: f45(),
    });
    let hashrate = lb(L {
        x: 223,
        y: -63,
        align: lv::LEFT_MID,
        text: c"500,0",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: f24(),
    });
    let temp = lb(L {
        x: 261,
        y: -18,
        align: lv::LEFT_MID,
        text: c"24",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: f24(),
    });
    s.on_screen_loaded(l, on_screen_loaded);
    (
        s,
        Btc {
            price,
            hashrate,
            temp,
        },
    )
}

fn global_screen(l: &Lvgl, t: &Theme) -> (Obj, Global) {
    let s = Obj::screen(l);
    background(l, &s, t.global);
    let lb = |spec: L| label(l, &s, &spec);
    let halving_percent = lb(L {
        x: -64,
        y: 36,
        align: lv::CENTER,
        text: c"95%",
        color: 0xC6C6C5,
        talign: lv::TEXT_LEFT,
        font: f14(),
    });
    let block = lb(L {
        x: -37,
        y: 67,
        align: lv::RIGHT_MID,
        text: c"881.557",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: f24(),
    });
    let blocks_to_halving = lb(L {
        x: -97,
        y: 68,
        align: lv::CENTER,
        text: c"210.000",
        color: 0xC6C6C5,
        talign: lv::TEXT_CENTER,
        font: f24(),
    });
    let difficulty = lb(L {
        x: -40,
        y: -11,
        align: lv::RIGHT_MID,
        text: c"81T",
        color: 0xC6C6C5,
        talign: lv::TEXT_RIGHT,
        font: f24(),
    });
    let hashrate = lb(L {
        x: -39,
        y: 30,
        align: lv::RIGHT_MID,
        text: c"751,45",
        color: 0x000000,
        talign: lv::TEXT_RIGHT,
        font: f24(),
    });
    let low_fee = lb(L {
        x: 47,
        y: -64,
        align: lv::CENTER,
        text: c"2",
        color: 0xFFFFFF,
        talign: lv::TEXT_CENTER,
        font: f13(),
    });
    let mid_fee = lb(L {
        x: 89,
        y: -64,
        align: lv::CENTER,
        text: c"200",
        color: 0xFFFFFF,
        talign: lv::TEXT_CENTER,
        font: f13(),
    });
    let high_fee = lb(L {
        x: 138,
        y: -64,
        align: lv::CENTER,
        text: c"1000",
        color: 0xFFFFFF,
        talign: lv::TEXT_CENTER,
        font: f13(),
    });
    s.on_screen_loaded(l, on_screen_loaded);
    (
        s,
        Global {
            halving_percent,
            block,
            blocks_to_halving,
            difficulty,
            hashrate,
            low_fee,
            mid_fee,
            high_fee,
        },
    )
}

// ---------------------------------------------------------------------------
// the values

impl Ui {
    /// `updateGlobalState()`: the values of all screens; the BTC price and
    /// the global stats only while their screen is shown (and once at the
    /// start).
    fn update(&mut self, l: &Lvgl, pool: c_int, btc_shown: bool, global_shown: bool) {
        use crate::power_mgmt;
        let temp = text::degrees(power_mgmt::get(3));
        self.mining.temp.set_text_str(l, &temp);
        self.btc.temp.set_text_str(l, &temp);
        self.mining
            .rpm
            .set_text_str(l, &power_mgmt::fan_get(0, 0).to_string());
        let power = power_mgmt::get(0);
        self.mining.power.set_text_str(l, &text::watts(power));
        self.mining
            .current
            .set_text_str(l, &format!("{}mA", power_mgmt::get(2) as i32));
        self.mining
            .vinput
            .set_text_str(l, &format!("{}mV", power_mgmt::get(1) as i32));

        let up = (crate::cutil::now_us() - crate::system::start_us()) / 1_000_000;
        self.mining.time.set_text_str(l, &text::uptime(up));

        let m = crate::pool_manager::get();
        if let Some(m) = m {
            let (acc, rej) = if m.is_dual() {
                let p = m.pool_stats(pool);
                (p.accepted, p.rejected)
            } else {
                let s = m.summary();
                (s.accepted, s.rejected)
            };
            self.settings
                .shares
                .set_text_str(l, &text::fit20(format!("{acc}/{rej}")));
            let best = crate::suffix::suffix_string(m.summary().total_best, 0);
            self.mining.best_diff.set_text_str(l, &best);
            self.settings.best_diff.set_text_str(l, &best);
        }

        let ghs = crate::system::current_hashrate();
        let hr = text::hashrate(ghs);
        self.mining.hashrate.set_text_str(l, &hr);
        if let Some(m) = m {
            if m.is_dual() {
                let active = ghs * m.active_balance(pool) as f32 / 100.0;
                self.settings
                    .hashrate
                    .set_text_str(l, &text::hashrate(active));
            } else {
                self.settings.hashrate.set_text_str(l, &hr);
            }
        }
        self.btc.hashrate.set_text_str(l, &hr);
        self.mining
            .efficiency
            .set_text_str(l, &text::efficiency(power, ghs));
        self.mining.power.set_text_str(l, &text::watts(power));

        use crate::apis_fetcher::ffi as api;
        if btc_shown || self.btc_price == 0 {
            self.btc_price = api::apis_get_price();
            self.btc
                .price
                .set_text_str(l, &format!("{}$", self.btc_price));
        }
        if global_shown || self.block_height == 0 {
            self.block_height = api::apis_get_block_height();
            let g = &self.global;
            g.block.set_text_str(l, &self.block_height.to_string());
            g.blocks_to_halving
                .set_text_str(l, &api::apis_get_blocks_to_halving().to_string());
            g.halving_percent
                .set_text_str(l, &format!("{}%", api::apis_get_halving_percent()));
            g.hashrate
                .set_text_str(l, &api::apis_get_net_hash().to_string());
            g.difficulty
                .set_text_str(l, &format!("{}T", api::apis_get_net_difficulty()));
            g.low_fee
                .set_text_str(l, &api::apis_get_lowest_fee().to_string());
            g.mid_fee
                .set_text_str(l, &api::apis_get_mid_fee().to_string());
            g.high_fee
                .set_text_str(l, &api::apis_get_fastest_fee().to_string());
        }

        let vcore = (crate::board::ffi::board_power_value(3) * 1000.0) as i32 as u16;
        self.mining.vcore.set_text_str(l, &format!("{vcore}mV"));
    }

    /// `updateCurrentSettings()`: pool, frequency, voltage and fan setting.
    fn update_settings(&mut self, l: &Lvgl, pool: c_int) {
        let Some(m) = crate::pool_manager::get() else {
            return;
        };
        let s = &self.settings;
        if m.is_dual() {
            let host = String::from_utf8_lossy(m.pool_host(pool).as_bytes()).into_owned();
            s.pool.set_text_str(l, &text::fit20(host));
            s.port.set_text_str(l, &m.pool_port(pool).to_string());
            s.pool_nr.set_text_str(l, &(pool + 1).to_string());
        } else {
            let sel = m.display_pool(pool);
            let host = String::from_utf8_lossy(m.pool_host(sel).as_bytes()).into_owned();
            let mut host = host;
            host.truncate(127);
            s.pool.set_text_str(l, &host);
            s.port.set_text_str(l, &m.pool_port(sel).to_string());
        }
        let b = crate::board::ffi::info();
        s.freq.set_text_str(l, &b.asic_frequency.to_string());
        s.vcore.set_text_str(l, &b.asic_voltage_mv.to_string());
        s.fan.set_text_str(
            l,
            &text::fan_setting(b.fan_mode[0], crate::cfg::fan_speed()),
        );
    }

    fn set_ip(&mut self, l: &Lvgl, ip: &CStr) {
        self.mining.ip.set_text(l, ip);
        self.settings.ip.set_text(l, ip);
    }

    // --- overlays

    fn hide_error(&mut self, l: &Lvgl) {
        if let Some(o) = self.err_overlay.take() {
            o.delete(l);
        }
    }

    fn show_error(&mut self, l: &Lvgl, msg: &CStr, code: u32) {
        self.hide_error(l);
        let Some(scr) = Obj::active_screen(l) else {
            return;
        };
        let c = scr.container(l);
        c.size(l, 278, 80)
            .align(l, lv::CENTER, 0, -20)
            .clear_flag(l, raw::LV_OBJ_FLAG_SCROLLABLE);
        c.bg_color(l, color(0x111111))
            .border(l, 2, Some(color(0xe60000)));
        let red = color(0xe60000);
        let e = c.label(l);
        e.size(l, lv::SIZE_CONTENT, lv::SIZE_CONTENT)
            .pos(l, 0, 0)
            .set_align(l, lv::TOP_MID)
            .set_text(l, msg)
            .text_color(l, red)
            .text_opa(l, 255)
            .font(l, asset!(ui_font_vt323_35))
            .text_align(l, lv::TEXT_CENTER);
        let k = c.label(l);
        k.size(l, lv::SIZE_CONTENT, lv::SIZE_CONTENT)
            .pos(l, 0, 0)
            .set_align(l, lv::BOTTOM_MID)
            .set_text_str(l, &format!("Guru Meditation #{:08X}", code))
            .text_color(l, red)
            .text_opa(l, 255)
            .font(l, asset!(ui_font_vt323_21))
            .text_align(l, lv::TEXT_CENTER);
        self.err_overlay = Some(c);
    }

    fn hide_image(&mut self, l: &Lvgl) {
        if let Some(o) = self.image_overlay.take() {
            o.delete(l);
        }
    }

    fn show_image(&mut self, l: &Lvgl, img: &'static Image) {
        self.hide_image(l);
        let Some(scr) = Obj::active_screen(l) else {
            return;
        };
        let c = scr.container(l);
        c.size(l, lv::SIZE_CONTENT, lv::SIZE_CONTENT)
            .align(l, lv::CENTER, 0, 0)
            .clear_flag(l, raw::LV_OBJ_FLAG_SCROLLABLE);
        c.bg_opa(l, raw::LV_OPA_TRANSP as u8).border(l, 0, None);
        let i = c.image(l);
        i.set_src(l, img).align(l, lv::CENTER, 0, 0);
        self.image_overlay = Some(c);
    }

    /// Empties a splash screen (its children are deleted): the handles of
    /// objects on it go first.
    fn drop_splash(&mut self, l: &Lvgl, which: c_int) {
        let scr = match which {
            SCREEN_SPLASH1 => self.splash1.take(),
            SCREEN_SPLASH2 => {
                self.connect = None;
                self.splash2.take()
            }
            _ => None,
        };
        let Some(scr) = scr else {
            return;
        };
        if self
            .err_overlay
            .as_ref()
            .is_some_and(|o| o.is_child_of(l, &scr))
        {
            self.err_overlay = None;
        }
        if self
            .image_overlay
            .as_ref()
            .is_some_and(|o| o.is_child_of(l, &scr))
        {
            self.image_overlay = None;
        }
        scr.clean(l);
        // the empty screen object stays allocated, as before (dropping the
        // handle does not delete it)
    }

    fn screen(&self, which: c_int) -> Option<&Obj> {
        Some(match which {
            SCREEN_SPLASH1 => self.splash1.as_ref()?,
            SCREEN_SPLASH2 => self.splash2.as_ref()?,
            SCREEN_PORTAL => &self.portal,
            SCREEN_MINING => &self.mining_scr,
            SCREEN_SETTINGS => &self.settings_scr,
            SCREEN_BTC => &self.btc_scr,
            SCREEN_GLOBAL => &self.global_scr,
            SCREEN_LOG => &self.log.as_ref()?.0,
            SCREEN_QR => &self.qr.as_ref()?.0,
            SCREEN_POWER_OFF => self.power_off.as_ref()?,
            _ => return None,
        })
    }

    fn power_off(&mut self, l: &Lvgl) {
        if self.power_off.is_some() {
            return;
        }
        let s = Obj::screen(l);
        s.bg_color(l, lv::BLACK).bg_opa(l, 255);
        let i = s.image(l);
        i.set_src(l, asset!(ui_img_safe_png))
            .align(l, lv::CENTER, 0, 0);
        s.on_screen_loaded(l, on_screen_loaded);
        self.power_off = Some(s);
    }

    fn log(&mut self, l: &Lvgl, msg: &CStr) {
        let (_, label) = self.log.get_or_insert_with(|| {
            let s = Obj::screen(l);
            s.bg_color(l, lv::BLACK).bg_opa(l, 255);
            let t = s.label(l);
            t.set_text(l, c"")
                .text_color(l, lv::WHITE)
                .font(l, f14())
                .size(l, lv::pct(100), lv::SIZE_CONTENT)
                .text_align(l, lv::TEXT_LEFT)
                .align(l, lv::TOP_LEFT, 0, 0);
            (s, t)
        });
        label.set_text(l, msg);
    }

    /// The OTP enrollment QR screen; false if it could not be made.
    fn qr(&mut self, l: &Lvgl, qr: Qr<'_>) -> bool {
        const QUIET: i32 = 4;
        let n = qr.size();
        if n <= 0 {
            return false;
        }
        let scale = (160 / (n + 2 * QUIET)).max(2);
        let w = ((n + 2 * QUIET) * scale) as i16;
        if self.qr.is_none() {
            let s = Obj::screen(l);
            s.bg_color(l, lv::BLACK).bg_opa(l, 255);
            let Some(canvas) = Canvas::new(l, &s, w) else {
                crate::log::log!(Error, c"ui", "QR canvas alloc failed: {w}x{w}");
                // the screen object stays allocated (as before), without a
                // canvas; dropping the handle does not delete it
                return false;
            };
            canvas.obj().align(l, lv::RIGHT_MID, -10, 0);
            let t = s.label(l);
            t.set_text(
                l,
                c"Scan this QR code with your Authenticator App.\n\nPress any button to cancel.",
            )
            .text_color(l, lv::WHITE)
            .font(l, f14())
            .text_align(l, lv::TEXT_LEFT)
            .size(l, 320 / 2 - 20, lv::SIZE_CONTENT)
            .long_mode_wrap(l)
            .text_align(l, lv::TEXT_CENTER)
            .align(l, lv::LEFT_MID, 10, 0);
            s.on_screen_loaded(l, on_screen_loaded);
            self.qr = Some((s, canvas));
        }
        let (_, canvas) = self.qr.as_mut().expect("set above");
        draw_qr(l, canvas, &qr, QUIET, scale);
        true
    }
}

// the screens of ui_screen() (ui_rs.h)
const SCREEN_SPLASH1: c_int = 0;
const SCREEN_SPLASH2: c_int = 1;
const SCREEN_PORTAL: c_int = 2;
const SCREEN_MINING: c_int = 3;
const SCREEN_SETTINGS: c_int = 4;
const SCREEN_BTC: c_int = 5;
const SCREEN_GLOBAL: c_int = 6;
const SCREEN_LOG: c_int = 7;
const SCREEN_QR: c_int = 8;
const SCREEN_POWER_OFF: c_int = 9;

/// The UI under its own lock, with the LVGL lock the caller holds.
fn with_ui<T: Default>(f: impl FnOnce(&mut Ui, &Lvgl) -> T) -> T {
    // SAFETY: every ui_* function is called with the display's LVGL lock
    // held (ui_rs.h contract); the token does not outlive this call
    let l = unsafe { Lvgl::assume_locked() };
    let mut ui = UI.lock().unwrap_or_else(|e| e.into_inner());
    ui.as_mut().map_or_else(T::default, |ui| f(ui, &l))
}

/// # Safety (all)
/// Called with the display's LVGL lock held; strings NUL-terminated.
pub(crate) mod ffi {
    use super::*;

    fn cstr<'a>(p: *const c_char) -> &'a CStr {
        if p.is_null() {
            c""
        } else {
            // SAFETY: NUL-terminated (caller contract)
            unsafe { CStr::from_ptr(p) }
        }
    }

    /// Creates the screens and loads the first splash screen; `driver`
    /// gets the screen load events (`disp_screen_loaded()`).
    #[no_mangle]
    pub extern "C" fn ui_init(driver: *mut core::ffi::c_void) {
        DRIVER.store(driver, core::sync::atomic::Ordering::Release);
        // SAFETY: called by the driver before its LVGL task runs, or with its
        // LVGL lock held
        let l = unsafe { Lvgl::assume_locked() };
        // SAFETY: lock held (token); the default display exists
        unsafe {
            let d = raw::lv_disp_get_default();
            let t = raw::lv_theme_default_init(
                d,
                raw::lv_palette_main(raw::lv_palette_t_LV_PALETTE_BLUE),
                raw::lv_palette_main(raw::lv_palette_t_LV_PALETTE_RED),
                false,
                &raw::lv_font_montserrat_14,
            );
            raw::lv_disp_set_theme(d, t);
        }
        let ui = Ui::new(&l);
        // SAFETY: lock held; a live screen
        unsafe { raw::lv_disp_load_scr(ui.splash1.as_ref().expect("new").as_ptr()) };
        if ui.theme.generic {
            let tint = color(0xffffff);
            for s in [
                ui.splash1.as_ref(),
                ui.splash2.as_ref(),
                Some(&ui.portal),
                Some(&ui.mining_scr),
                Some(&ui.settings_scr),
                Some(&ui.btc_scr),
                Some(&ui.global_scr),
            ]
            .into_iter()
            .flatten()
            {
                s.tint_images(&l, tint, 100);
            }
        }
        *UI.lock().unwrap_or_else(|e| e.into_inner()) = Some(ui);
    }

    /// A screen (`UI_SCREEN_*`), NULL if not created.
    #[no_mangle]
    pub extern "C" fn ui_screen(which: c_int) -> *mut raw::lv_obj_t {
        let ui = UI.lock().unwrap_or_else(|e| e.into_inner());
        ui.as_ref()
            .and_then(|u| u.screen(which))
            .map_or(core::ptr::null_mut(), Obj::as_ptr)
    }

    #[no_mangle]
    pub extern "C" fn ui_drop_splash(which: c_int) {
        with_ui(|u, l| u.drop_splash(l, which))
    }

    /// # Safety
    /// See the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_set_ssid(ssid: *const c_char) {
        with_ui(|u, l| {
            u.ssid.set_text(l, cstr(ssid));
        })
    }

    /// # Safety
    /// See the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_set_connect(msg: *const c_char) {
        with_ui(|u, l| {
            if let Some(c) = &u.connect {
                c.set_text(l, cstr(msg));
            }
        })
    }

    #[no_mangle]
    pub extern "C" fn ui_update(pool: c_int, btc_shown: bool, global_shown: bool) {
        with_ui(|u, l| u.update(l, pool, btc_shown, global_shown))
    }

    #[no_mangle]
    pub extern "C" fn ui_update_settings(pool: c_int) {
        with_ui(|u, l| u.update_settings(l, pool))
    }

    /// # Safety
    /// See the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_set_ip(ip: *const c_char) {
        with_ui(|u, l| u.set_ip(l, cstr(ip)))
    }

    /// 0 WiFi, 1 Ethernet, 2 CAN
    #[no_mangle]
    pub extern "C" fn ui_set_net_icon(kind: c_int) {
        with_ui(|u, l| {
            let img = match kind {
                1 => asset!(ui_img_eth_png),
                2 => asset!(ui_img_can_png),
                _ => asset!(ui_img_wifi_png),
            };
            u.mining.net.set_src(l, img);
        })
    }

    /// # Safety
    /// See the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_show_error(msg: *const c_char, code: u32) {
        with_ui(|u, l| u.show_error(l, cstr(msg), code))
    }

    #[no_mangle]
    pub extern "C" fn ui_hide_error() {
        with_ui(|u, l| u.hide_error(l))
    }

    #[no_mangle]
    pub extern "C" fn ui_show_found_block() {
        with_ui(|u, l| u.show_image(l, asset!(ui_img_found_block_png)))
    }

    #[no_mangle]
    pub extern "C" fn ui_hide_image_overlay() {
        with_ui(|u, l| u.hide_image(l))
    }

    #[no_mangle]
    pub extern "C" fn ui_power_off_init() {
        with_ui(|u, l| u.power_off(l))
    }

    /// # Safety
    /// See the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_log(msg: *const c_char) {
        with_ui(|u, l| u.log(l, cstr(msg)))
    }

    /// The OTP QR screen from a qrcodegen buffer; false without it.
    ///
    /// # Safety
    /// `qr` NULL or a qrcodegen buffer; see the module.
    #[no_mangle]
    pub unsafe extern "C" fn ui_create_qr(qr: *const u8, size: c_int) -> bool {
        if qr.is_null() || size <= 0 {
            crate::log::log!(Error, c"ui", "No QR to draw");
            return false;
        }
        // qrcodegen buffer length for the code's version
        let len = ((size * size) as usize).div_ceil(8) + 1;
        // SAFETY: a qrcodegen buffer of a `size` modules code (caller contract)
        let buf = unsafe { core::slice::from_raw_parts(qr, len) };
        with_ui(|u, l| u.qr(l, Qr(buf)))
    }
}
