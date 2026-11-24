//! The display screens (port of `main/displays/ui.cpp` and the update
//! functions of `displayDriver.cpp`): the splash, portal, mining, settings,
//! BTC and global stats screens, the log, QR and power-off screens, the
//! error and image overlays, and the values shown on them, read directly
//! from the Rust modules. The display driver (`display/driver.rs`: panel,
//! LVGL task, state machine, buttons) owns the [`Ui`] under its LVGL lock and
//! loads the screens it gets from here.
//!
//! Differences to the C++ code:
//! - the handles of objects that are deleted with their screen (an overlay
//!   on a splash screen) are dropped with it instead of dangling, and the
//!   "Connecting..." label of the second splash screen keeps none;
//! - the network icon is changed under the LVGL lock.

use core::ffi::{c_int, CStr};

use crate::display::lvgl::{self as lv, color, Canvas, Font, Image, Lvgl, Obj};
use crate::display::text;
use crate::idf::raw;
use qrcodegen::{QrCode, QrCodeEcc, QrSegment, Version};

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

/// A screen load animation ended (LVGL event, in the LVGL task).
unsafe extern "C" fn on_screen_loaded(_: *mut raw::lv_event_t) {
    crate::display::driver::screen_loaded();
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
pub(crate) struct Ui {
    theme: Theme,
    splash1: Option<Obj>,
    splash2: Option<Obj>,
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
    let i = crate::board::api::info();
    if asic {
        i.asic_model
    } else {
        i.device_model
    }
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
        // a fixed text (no handle kept: it goes with the splash screen)
        label(
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

    let Some(qr) = encode_qr(DONATE_ADDR, 6) else {
        return;
    };
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

/// `text` as a QR code of at most `max_version` (medium error correction).
fn encode_qr(text: &CStr, max_version: u8) -> Option<QrCode> {
    QrCode::encode_segments_advanced(
        &QrSegment::make_segments(&text.to_string_lossy()),
        QrCodeEcc::Medium,
        Version::MIN,
        Version::new(max_version),
        None,
        true,
    )
    .ok()
}

/// White background, black modules with a quiet zone of `quiet` modules.
fn draw_qr(l: &Lvgl, c: &mut Canvas, qr: &QrCode, quiet: i32, scale: i32) {
    let w = c.width();
    c.rect(l, 0, 0, w, w, lv::WHITE);
    let n = qr.size();
    for y in 0..n {
        for x in 0..n {
            if qr.get_module(x, y) {
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
    pub(crate) fn update(&mut self, l: &Lvgl, pool: c_int, btc_shown: bool, global_shown: bool) {
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

        use crate::apis_fetcher as api;
        let v = api::values();
        if btc_shown || self.btc_price == 0 {
            self.btc_price = v.price_usd;
            self.btc
                .price
                .set_text_str(l, &format!("{}$", self.btc_price));
        }
        if global_shown || self.block_height == 0 {
            self.block_height = v.block_height;
            let g = &self.global;
            g.block.set_text_str(l, &self.block_height.to_string());
            g.blocks_to_halving
                .set_text_str(l, &api::blocks_to_halving(v.block_height).to_string());
            g.halving_percent
                .set_text_str(l, &format!("{}%", api::halving_percent(v.block_height)));
            g.hashrate.set_text_str(l, &v.net_hash.to_string());
            g.difficulty
                .set_text_str(l, &format!("{}T", v.net_difficulty));
            g.low_fee.set_text_str(l, &v.hour_fee.to_string());
            g.mid_fee.set_text_str(l, &v.half_hour_fee.to_string());
            g.high_fee.set_text_str(l, &v.fastest_fee.to_string());
        }

        let vcore = (crate::board::api::board_power_value(3) * 1000.0) as i32 as u16;
        self.mining.vcore.set_text_str(l, &format!("{vcore}mV"));
    }

    /// `updateCurrentSettings()`: pool, frequency, voltage and fan setting.
    pub(crate) fn update_settings(&mut self, l: &Lvgl, pool: c_int) {
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
        let b = crate::board::api::info();
        s.freq.set_text_str(l, &b.asic_frequency.to_string());
        s.vcore.set_text_str(l, &b.asic_voltage_mv.to_string());
        s.fan.set_text_str(
            l,
            &text::fan_setting(b.fan_mode[0], crate::cfg::fan_speed()),
        );
    }

    pub(crate) fn set_ip(&mut self, l: &Lvgl, ip: &CStr) {
        self.mining.ip.set_text(l, ip);
        self.settings.ip.set_text(l, ip);
    }

    // --- overlays

    pub(crate) fn hide_error(&mut self, l: &Lvgl) {
        if let Some(o) = self.err_overlay.take() {
            o.delete(l);
        }
    }

    pub(crate) fn show_error(&mut self, l: &Lvgl, msg: &CStr, code: u32) {
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

    pub(crate) fn hide_image(&mut self, l: &Lvgl) {
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
    pub(crate) fn drop_splash(&mut self, l: &Lvgl, which: Screen) {
        let scr = match which {
            Screen::Splash1 => self.splash1.take(),
            Screen::Splash2 => self.splash2.take(),
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

    pub(crate) fn screen(&self, which: Screen) -> Option<&Obj> {
        Some(match which {
            Screen::Splash1 => self.splash1.as_ref()?,
            Screen::Splash2 => self.splash2.as_ref()?,
            Screen::Portal => &self.portal,
            Screen::Mining => &self.mining_scr,
            Screen::Settings => &self.settings_scr,
            Screen::Btc => &self.btc_scr,
            Screen::Global => &self.global_scr,
            Screen::Log => &self.log.as_ref()?.0,
            Screen::Qr => &self.qr.as_ref()?.0,
            Screen::PowerOff => self.power_off.as_ref()?,
        })
    }

    pub(crate) fn power_off(&mut self, l: &Lvgl) {
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

    pub(crate) fn log(&mut self, l: &Lvgl, msg: &CStr) {
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
    fn qr(&mut self, l: &Lvgl, qr: &QrCode) -> bool {
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
        draw_qr(l, canvas, qr, QUIET, scale);
        true
    }
}

/// The screens the driver loads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Splash1,
    Splash2,
    Portal,
    Mining,
    Settings,
    Btc,
    Global,
    Log,
    Qr,
    PowerOff,
}

impl Ui {
    /// Creates the screens (with the default theme) and loads the first
    /// splash screen.
    pub(crate) fn new_loaded(l: &Lvgl) -> Ui {
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
        let ui = Ui::new(l);
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
                s.tint_images(l, tint, 100);
            }
        }
        ui
    }

    pub(crate) fn set_ssid(&mut self, l: &Lvgl, ssid: &CStr) {
        self.ssid.set_text(l, ssid);
    }

    pub(crate) fn set_net_icon(&mut self, l: &Lvgl, icon: NetIcon) {
        let img = match icon {
            NetIcon::Eth => asset!(ui_img_eth_png),
            NetIcon::Can => asset!(ui_img_can_png),
            NetIcon::Wifi => asset!(ui_img_wifi_png),
        };
        self.mining.net.set_src(l, img);
    }

    pub(crate) fn show_found_block(&mut self, l: &Lvgl) {
        self.show_image(l, asset!(ui_img_found_block_png))
    }

    /// The OTP QR screen; false without a code.
    pub(crate) fn create_qr(&mut self, l: &Lvgl, qr: Option<&QrCode>) -> bool {
        let Some(qr) = qr else {
            crate::log::log!(Error, c"ui", "No QR to draw");
            return false;
        };
        self.qr(l, qr)
    }
}

/// The network icon of the mining screen.
#[derive(Clone, Copy)]
pub(crate) enum NetIcon {
    Wifi,
    Eth,
    Can,
}
