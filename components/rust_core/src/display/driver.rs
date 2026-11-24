//! The display driver (port of `main/displays/displayDriver.cpp`): the
//! LilyGO T-Display-S3 panel (ST7789 on an Intel 8080 bus), LVGL with its
//! task, the two buttons and the screen state machine. The screens are in
//! [`crate::display::ui`]; the rest of the firmware uses [`crate::display`].
//!
//! LVGL is not thread-safe: the driver, with the [`Ui`] and the [`Lvgl`]
//! token, is behind one lock ([`DRIVER`]), which the LVGL task holds for
//! `lv_timer_handler()` and its state machine, and the other tasks for
//! their updates.
//!
//! Behaviour:
//! - the LVGL task's whole cycle (state machine, overlays, screen changes)
//!   runs under the lock;
//! - the power-off state switches the board off after the lock is released;
//! - the self test's log screen stays until the boot goes on (`Log` state);
//! - cancelling the OTP QR code with a button also ends its overlay.

use core::ffi::{c_void, CStr};
use std::ffi::CString;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Mutex, MutexGuard};

use crate::display::button::{self as btn, Button};
use crate::display::lvgl::{self as lv, Anim, Lvgl, Obj};
use crate::display::ui::{NetIcon, Screen, Ui};
use crate::display::Msg;
use crate::idf::raw;
use crate::log::log;

const TAG: &CStr = c"TDisplayS3";

const PIN_BUTTON_1: i32 = 14;
const PIN_BUTTON_2: i32 = 0;

const PIN_DATA: [i32; 8] = [39, 40, 41, 42, 45, 46, 47, 48];
const PIN_RD: i32 = 9;
const PIN_PWR: i32 = 15;
const PIN_PCLK: i32 = 8;
const PIN_CS: i32 = 6;
const PIN_DC: i32 = 7;
const PIN_RST: i32 = 5;
const PIN_BK_LIGHT: i32 = 38;

/// 60 FPS at 170x320
const PIXEL_CLOCK_HZ: u32 = 6_528_000;
const H_RES: i16 = 320;
const V_RES: i16 = 170;
/// the LVGL draw buffer (pixels): a quarter of the screen
const BUF_PX: usize = (H_RES as usize * V_RES as usize) / 4;
/// the LVGL time added per task cycle (as before, whatever the cycle took)
const TICK_MS: u32 = 30;
const LONGPRESS_MS: i64 = 5000;

const SPLASH1_MS: i64 = 3000;
#[cfg(any(board = "NERDQX", board = "Q1370", board = "Q1373"))]
const SPLASH2_MS: i64 = 5000;
#[cfg(not(any(board = "NERDQX", board = "Q1370", board = "Q1373")))]
const SPLASH2_MS: i64 = 3000;

/// the "Turning screen off..." countdown (s)
const AUTO_OFF_COUNTDOWN_S: i64 = 5;
/// both buttons held this long switch the board off (s)
const SHUTDOWN_COUNTDOWN_S: i64 = 5;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum State {
    Splash1,
    Splash2,
    Wait,
    Portal,
    Mining,
    Settings,
    Btc,
    Global,
    ShowQr,
    Identify,
    PowerOff,
    /// the self test's messages
    Log,
    Nop,
}

/// the state for `wait_for_splashs()`, without the lock
static STATE: AtomicU8 = AtomicU8::new(State::Nop as u8);
/// a screen load animation runs (buttons are ignored); cleared by the
/// screens' load event in the LVGL task, which holds the lock
static ANIMATION_RUNNING: AtomicBool = AtomicBool::new(false);

static DRIVER: Mutex<Option<Driver>> = Mutex::new(None);

struct Shutdown {
    bx: Obj,
    label: Obj,
    start_us: i64,
}

struct Driver {
    l: Lvgl,
    ui: Ui,
    state: State,
    state_start_us: i64,
    animations: bool,
    animation_cycles: i32,
    last_keypress_us: i64,
    on: bool,
    /// an error, QR or identify overlay: the screen stays on
    overlay: bool,
    /// the found-block image, closed by a button press
    found_block: bool,
    identify_ms: u32,
    auto_off: bool,
    /// the screen goes dark after this time without a key press
    auto_off_us: i64,
    show_found_block: bool,
    /// the "Turning screen off..." box and its start
    countdown: Option<(Obj, i64)>,
    shutdown: Option<Shutdown>,
    ignore_buttons_until_us: i64,
    buttons: [Button; 2],
}

// SAFETY: the LVGL objects (and the token) are only used with DRIVER's lock
// held, from whichever task holds it
unsafe impl Send for Driver {}

fn driver() -> MutexGuard<'static, Option<Driver>> {
    DRIVER.lock().unwrap_or_else(|e| e.into_inner())
}

/// Runs `f` on the driver with the LVGL lock; nothing without a display.
fn with<T: Default>(f: impl FnOnce(&mut Driver) -> T) -> T {
    driver().as_mut().map_or_else(T::default, f)
}

fn now_us() -> i64 {
    crate::cutil::now_us()
}

fn gpio_level(pin: i32, level: bool) {
    // SAFETY: plain value arguments
    unsafe { raw::gpio_set_level(pin, level as u32) };
}

/// A screen load animation ended (the screens' load event).
pub(crate) fn screen_loaded() {
    ANIMATION_RUNNING.store(false, Ordering::Release);
}

impl Driver {
    fn turn_off(&mut self) -> bool {
        if !self.on {
            return false;
        }
        gpio_level(PIN_BK_LIGHT, false);
        gpio_level(PIN_PWR, false);
        log!(Info, TAG, "Screen off");
        self.on = false;
        true
    }

    fn turn_on(&mut self) -> bool {
        if self.on {
            return false;
        }
        gpio_level(PIN_PWR, true);
        gpio_level(PIN_BK_LIGHT, true);
        log!(Info, TAG, "Screen on");
        self.on = true;
        true
    }

    fn load_settings(&mut self) {
        self.auto_off = crate::config::cfg::auto_screen_off();
        self.auto_off_us = crate::config::cfg::screen_off_secs() as i64 * 1_000_000;
        self.show_found_block = crate::config::cfg::show_block_found();
        // the setting was switched off: the screen comes on
        if !self.auto_off {
            self.turn_on();
        }
    }

    /// A black box with a white text in the middle of the active screen.
    fn message_box(&self, text: &CStr) -> Option<(Obj, Obj)> {
        let l = &self.l;
        let bx = Obj::active_screen(l)?.container(l);
        bx.size(l, 200, 100)
            .bg_color(l, lv::BLACK)
            .border(l, 0, None)
            .align(l, lv::CENTER, 0, 0);
        let label = bx.label(l);
        label
            .set_text(l, text)
            .text_color(l, lv::WHITE)
            .align(l, lv::CENTER, 0, 0);
        Some((bx, label))
    }

    // --- auto off

    fn hide_countdown(&mut self) {
        if let Some((bx, _)) = self.countdown.take() {
            bx.delete(&self.l);
        }
    }

    fn check_auto_off(&mut self) {
        if !self.on {
            return;
        }
        let now = now_us();
        if now - self.last_keypress_us > self.auto_off_us {
            if self.countdown.is_none() {
                let bx = self.message_box(c"Turning screen off...").map(|(bx, _)| bx);
                match bx {
                    Some(bx) => self.countdown = Some((bx, now)),
                    None => return,
                }
            }
            let start = self.countdown.as_ref().map_or(now, |c| c.1);
            if (now - start) / 1_000_000 > AUTO_OFF_COUNTDOWN_S {
                self.hide_countdown();
                self.turn_off();
            }
        } else {
            self.hide_countdown();
        }
    }

    // --- shutdown countdown (both buttons held)

    fn start_shutdown_countdown(&mut self) {
        if self.shutdown.is_some() {
            return;
        }
        if let Some((bx, label)) = self.message_box(c"Shutdown in 5") {
            self.overlay = true;
            self.shutdown = Some(Shutdown {
                bx,
                label,
                start_us: now_us(),
            });
        }
    }

    /// True once the countdown ran out (and the state is power off).
    fn update_shutdown_countdown(&mut self) -> bool {
        let Some(s) = &self.shutdown else {
            return false;
        };
        let elapsed = (now_us() - s.start_us) / 1_000_000;
        let remaining = (SHUTDOWN_COUNTDOWN_S - elapsed).max(0);
        s.label
            .set_text_str(&self.l, &format!("Shutdown in {remaining}"));
        if elapsed >= SHUTDOWN_COUNTDOWN_S {
            self.hide_shutdown_countdown();
            return self.enter(State::PowerOff, now_us());
        }
        false
    }

    fn hide_shutdown_countdown(&mut self) {
        if let Some(s) = self.shutdown.take() {
            s.bx.delete(&self.l);
        }
        self.overlay = false;
    }

    // --- screens

    fn load(&mut self, which: Screen, anim: Anim, time: u32) {
        if let Some(s) = self.ui.screen(which) {
            ANIMATION_RUNNING.store(true, Ordering::Release);
            s.load(&self.l, anim, time, 0);
        }
    }

    /// Empties a splash screen; the boxes on it go with it.
    fn drop_splash(&mut self, which: Screen) {
        if let Some(scr) = self.ui.screen(which) {
            let l = &self.l;
            if self
                .countdown
                .as_ref()
                .is_some_and(|(bx, _)| bx.is_child_of(l, scr))
            {
                self.countdown = None;
            }
            if self
                .shutdown
                .as_ref()
                .is_some_and(|s| s.bx.is_child_of(l, scr))
            {
                self.shutdown = None;
                self.overlay = false;
            }
        }
        self.ui.drop_splash(&self.l, which);
    }

    /// `enterState()`; true if the board is to be switched off (after the
    /// lock is released).
    fn enter(&mut self, s: State, now: i64) -> bool {
        if self.state == s {
            return false;
        }
        let previous = self.state;
        self.state = s;
        self.state_start_us = now;
        STATE.store(s as u8, Ordering::Release);

        match s {
            State::Nop | State::Log => {}
            State::Splash1 => {
                log!(Info, TAG, "enter state splash1");
                self.animations = true;
            }
            State::Splash2 => {
                log!(Info, TAG, "enter state splash2");
                self.animations = true;
                self.load(Screen::Splash2, Anim::FadeOn, 500);
                self.drop_splash(Screen::Splash1);
            }
            State::Wait => {
                // splash 2 stays until the system task shows the portal or
                // the mining screen
                log!(Info, TAG, "enter state wait");
            }
            State::Portal => {
                log!(Info, TAG, "enter state portal");
                self.drop_splash(Screen::Splash2);
                self.animations = true;
                self.load(Screen::Portal, Anim::FadeOn, 500);
            }
            State::Mining => {
                log!(Info, TAG, "enter state mining");
                self.drop_splash(Screen::Splash2);
                self.animations = true;
                if previous == State::Global {
                    self.load(Screen::Mining, Anim::MoveLeft, 350);
                } else {
                    self.load(Screen::Mining, Anim::FadeOn, 500);
                }
            }
            State::Settings => {
                log!(Info, TAG, "enter state settings screen");
                self.animations = true;
                self.load(Screen::Settings, Anim::MoveLeft, 350);
            }
            State::Btc => {
                log!(Info, TAG, "enter state btc screen");
                self.animations = true;
                self.load(Screen::Btc, Anim::MoveLeft, 350);
            }
            State::Global => {
                log!(Info, TAG, "enter state global stats");
                self.animations = true;
                self.load(Screen::Global, Anim::MoveLeft, 350);
            }
            State::ShowQr => {
                log!(Info, TAG, "enter qr state");
                self.animations = true;
                self.load(Screen::Qr, Anim::FadeOn, 500);
            }
            State::Identify => {
                log!(
                    Info,
                    TAG,
                    "enter state identify (blink {}ms)",
                    self.identify_ms
                );
            }
            State::PowerOff => {
                self.ui.power_off(&self.l);
                self.animations = false;
                self.load(Screen::PowerOff, Anim::None, 0);
                return true;
            }
        }
        false
    }

    /// Button 1 switches the screen on, button 2 toggles it; true if it was
    /// switched (the press does nothing else).
    fn led_control(&mut self, b1: bool, b2: bool) -> bool {
        if b1 {
            return self.turn_on();
        }
        if b2 {
            if !self.on {
                return self.turn_on();
            }
            return self.turn_off();
        }
        false
    }

    /// `updateState()`; true if the board is to be switched off.
    fn update_state(&mut self, now: i64, b1: bool, b2: bool, both_long: bool) -> bool {
        let ms = (now - self.state_start_us) / 1000;
        if both_long {
            return self.enter(State::PowerOff, now);
        }
        match self.state {
            State::Nop | State::Wait | State::PowerOff | State::Log => {}
            State::Splash1 => {
                if ms >= SPLASH1_MS {
                    self.enter(State::Splash2, now);
                }
            }
            State::Splash2 => {
                if ms >= SPLASH2_MS {
                    self.enter(State::Wait, now);
                }
            }
            State::Portal => {}
            State::Mining => {
                if !self.led_control(b1, b2) && b1 {
                    crate::services::apis_fetcher::enable_fetching();
                    self.enter(State::Settings, now);
                }
            }
            State::Settings => {
                if !self.led_control(b1, b2) && b1 {
                    self.enter(State::Btc, now);
                    crate::services::apis_fetcher::enable_fetching();
                }
            }
            State::Btc => {
                if !self.led_control(b1, b2) && b1 {
                    self.enter(State::Global, now);
                }
            }
            State::Global => {
                if !self.led_control(b1, b2) && b1 {
                    self.enter(State::Mining, now);
                }
            }
            State::ShowQr => {
                if b1 || b2 {
                    // aborts the enrollment (and ends the overlay, as the
                    // web UI's cancel does)
                    crate::security::otp::disable_enrollment();
                    self.overlay = false;
                    self.enter(State::Mining, now);
                }
            }
            State::Identify => {
                if b1 || b2 || ms >= self.identify_ms as i64 {
                    self.overlay = false;
                    self.turn_on();
                    self.enter(State::Mining, now);
                } else if (ms / 500) % 2 == 0 {
                    // blink: 500 ms on, 500 ms off
                    self.turn_on();
                } else {
                    self.turn_off();
                }
            }
        }
        false
    }

    /// `processButtons()`: (button 1 pressed, button 2 pressed, both long
    /// pressed), and whether the shutdown countdown ran out.
    fn process_buttons(&mut self, now: i64) -> (bool, bool, bool, bool) {
        let ms = now / 1000;
        // SAFETY: plain value arguments (input pins)
        let levels = unsafe {
            [
                raw::gpio_get_level(PIN_BUTTON_1) != 0,
                raw::gpio_get_level(PIN_BUTTON_2) != 0,
            ]
        };
        for (b, high) in self.buttons.iter_mut().zip(levels) {
            b.update(high, ms);
        }
        let mut e1 = self.buttons[0].event();
        let mut e2 = self.buttons[1].event();
        // simulated short presses (development builds only)
        let sim = crate::display::shot::take_press();
        if sim & 1 != 0 {
            e1 |= btn::SHORTPRESS;
        }
        if sim & 2 != 0 {
            e2 |= btn::SHORTPRESS;
        }
        let both_pressed = e1 & btn::PRESSED != 0 && e2 & btn::PRESSED != 0;
        if e1 & btn::PRESSED != 0 || e2 & btn::PRESSED != 0 {
            self.last_keypress_us = now;
        }
        let clear = |d: &mut Driver| d.buttons.iter_mut().for_each(Button::clear);

        // nothing within 200 ms after the shutdown countdown was let go
        if now_us() < self.ignore_buttons_until_us {
            clear(self);
            return (false, false, false, false);
        }

        // a press closes the found-block image (its own flag: errors come
        // and go independently and must not keep it open)
        let short = (e1 | e2) & btn::SHORTPRESS != 0;
        if short && self.found_block {
            self.ui.hide_image(&self.l);
            self.found_block = false;
            clear(self);
            return (false, false, false, false);
        }

        if both_pressed {
            let off = if self.shutdown.is_none() {
                self.start_shutdown_countdown();
                false
            } else {
                self.update_shutdown_countdown()
            };
            return (false, false, false, off);
        }
        if self.shutdown.is_some() {
            self.hide_shutdown_countdown();
            self.ignore_buttons_until_us = now_us() + 200_000;
            clear(self);
            return (false, false, false, false);
        }

        let (mut b1, mut b2, mut both_long) = (false, false, false);
        if e1 & btn::LONGPRESS != 0 && e2 & btn::LONGPRESS != 0 {
            both_long = true;
            clear(self);
        } else {
            if e1 & btn::SHORTPRESS != 0 {
                self.last_keypress_us = now;
                b1 = true;
                self.buttons[0].clear();
            }
            if e2 & btn::SHORTPRESS != 0 {
                self.last_keypress_us = now;
                b2 = true;
                self.buttons[1].clear();
            }
        }
        (b1, b2, both_long, false)
    }

    fn handle_message(&mut self, now: i64) {
        let Some(m) = crate::display::take_msg() else {
            return;
        };
        match m {
            Msg::ShowQr => {
                if !crate::security::otp::enrollment_active() {
                    log!(Error, TAG, "no otp enrollment active");
                    return;
                }
                let qr = crate::security::otp_device::qr();
                if self.ui.create_qr(&self.l, qr.as_ref()) {
                    self.enter(State::ShowQr, now);
                }
                self.overlay = true;
            }
            Msg::HideQr => {
                self.overlay = false;
                self.enter(State::Mining, now);
            }
            Msg::Identify(ms) => {
                self.identify_ms = ms;
                self.overlay = true;
                self.enter(State::Identify, now);
            }
        }
    }

    /// `handleLvglTick()`: the LVGL timers; the time to sleep (ms).
    fn tick(&mut self) -> u32 {
        // SAFETY: lock held (`self`)
        let wait = unsafe {
            raw::lv_tick_inc(TICK_MS);
            raw::lv_timer_handler()
        };
        if self.animations {
            // ~200 FPS for a while
            self.animation_cycles += 1;
            if self.animation_cycles > 80 {
                self.animations = false;
                self.animation_cycles = 0;
            }
            return wait.min(5);
        }
        if wait > 0 && wait < 50 {
            wait
        } else {
            50
        }
    }

    /// One cycle of the task after the LVGL timers; true if the board is to
    /// be switched off.
    fn step(&mut self, now: i64) -> bool {
        let (mut b1, mut b2, mut both_long, off) = self.process_buttons(now);
        if ANIMATION_RUNNING.load(Ordering::Acquire) {
            (b1, b2, both_long) = (false, false, false);
        }
        self.handle_message(now);
        if self.overlay || self.found_block {
            self.turn_on();
        } else if self.auto_off {
            self.check_auto_off();
        }
        self.update_state(now, b1, b2, both_long) || off
    }
}

// ---------------------------------------------------------------------------
// panel and LVGL

/// The panel finished a transfer (ISR): LVGL may reuse the buffer.
unsafe extern "C" fn flush_ready(
    _: raw::esp_lcd_panel_io_handle_t,
    _: *mut raw::esp_lcd_panel_io_event_data_t,
    ctx: *mut c_void,
) -> bool {
    // SAFETY: `ctx` is the registered display driver (never freed)
    unsafe { raw::lv_disp_flush_ready(ctx.cast()) };
    false
}

/// LVGL rendered an area: to the panel (in the LVGL task, lock held).
unsafe extern "C" fn flush(
    drv: *mut raw::lv_disp_drv_t,
    area: *const raw::lv_area_t,
    px: *mut raw::lv_color_t,
) {
    // SAFETY: LVGL passes its registered driver and the area it rendered
    let (panel, a) = unsafe { ((*drv).user_data as raw::esp_lcd_panel_handle_t, *area) };
    let (x1, y1, x2, y2) = (a.x1 as i32, a.y1 as i32, a.x2 as i32, a.y2 as i32);
    // SAFETY: `px` holds the area's pixels (LVGL's draw buffer)
    unsafe {
        crate::display::shot::shadow_flush(x1, y1, x2, y2, px as *const u16);
        raw::esp_lcd_panel_draw_bitmap(panel, x1, y1, x2 + 1, y2 + 1, px as *const c_void);
    }
}

fn check(what: &str, e: raw::esp_err_t) -> Option<()> {
    if e == 0 {
        return Some(());
    }
    log!(Error, TAG, "{what} failed: {e}");
    None
}

/// Zeroed memory in internal RAM that is never freed (the panel driver's
/// ISR uses it).
fn leak_internal<T>() -> Option<*mut T> {
    // SAFETY: plain allocation; zeroed is valid for the bindgen C structs
    let p = unsafe {
        raw::heap_caps_calloc(
            1,
            core::mem::size_of::<T>(),
            raw::MALLOC_CAP_INTERNAL | raw::MALLOC_CAP_8BIT,
        )
    };
    (!p.is_null()).then_some(p.cast())
}

/// `initTDisplayS3()`: the panel and LVGL with its display driver.
fn init_panel() -> Option<()> {
    log!(Info, TAG, "Turn off LCD backlight");
    for pin in [PIN_BK_LIGHT, PIN_RD, PIN_PWR] {
        // SAFETY: plain value arguments (output pins)
        unsafe {
            raw::esp_rom_gpio_pad_select_gpio(pin as u32);
            raw::gpio_set_direction(pin, raw::gpio_mode_t_GPIO_MODE_OUTPUT);
        }
    }
    gpio_level(PIN_RD, true);
    gpio_level(PIN_BK_LIGHT, false);

    log!(Info, TAG, "Initialize Intel 8080 bus");
    let mut data = [-1; 16];
    data[..8].copy_from_slice(&PIN_DATA);
    let bus_config = raw::esp_lcd_i80_bus_config_t {
        dc_gpio_num: PIN_DC,
        wr_gpio_num: PIN_PCLK,
        clk_src: raw::soc_periph_lcd_clk_src_t_LCD_CLK_SRC_DEFAULT,
        data_gpio_nums: data,
        bus_width: 8,
        max_transfer_bytes: BUF_PX * 2,
        __bindgen_anon_1: raw::esp_lcd_i80_bus_config_t__bindgen_ty_1 {
            psram_trans_align: 64,
        },
        sram_trans_align: 4,
    };
    let mut bus = core::ptr::null_mut();
    // SAFETY: the config is a live local, `bus` receives the handle
    check("i80 bus", unsafe {
        raw::esp_lcd_new_i80_bus(&bus_config, &mut bus)
    })?;

    let drv: *mut raw::lv_disp_drv_t = leak_internal()?;
    let draw_buf: *mut raw::lv_disp_draw_buf_t = leak_internal()?;

    let mut dc = raw::esp_lcd_panel_io_i80_config_t__bindgen_ty_1::default();
    dc.set_dc_idle_level(0);
    dc.set_dc_cmd_level(0);
    dc.set_dc_dummy_level(0);
    dc.set_dc_data_level(1);
    let io_config = raw::esp_lcd_panel_io_i80_config_t {
        cs_gpio_num: PIN_CS,
        pclk_hz: PIXEL_CLOCK_HZ,
        trans_queue_depth: 20,
        on_color_trans_done: Some(flush_ready),
        user_ctx: drv.cast(),
        lcd_cmd_bits: 8,
        lcd_param_bits: 8,
        dc_levels: dc,
        flags: Default::default(),
    };
    let mut io = core::ptr::null_mut();
    // SAFETY: a bus from above, the config a live local
    check("panel io", unsafe {
        raw::esp_lcd_new_panel_io_i80(bus, &io_config, &mut io)
    })?;

    log!(Info, TAG, "Install LCD driver of st7789");
    let panel_config = raw::esp_lcd_panel_dev_config_t {
        reset_gpio_num: PIN_RST,
        __bindgen_anon_1: raw::esp_lcd_panel_dev_config_t__bindgen_ty_1 {
            rgb_ele_order: raw::lcd_rgb_element_order_t_LCD_RGB_ELEMENT_ORDER_RGB,
        },
        bits_per_pixel: 16,
        ..Default::default()
    };
    let mut panel = core::ptr::null_mut();
    // SAFETY: the io handle from above, the config a live local
    check("st7789", unsafe {
        raw::esp_lcd_new_panel_st7789(io, &panel_config, &mut panel)
    })?;

    let flip = crate::board::api::info().flip_screen;
    // SAFETY: the panel from above
    unsafe {
        raw::esp_lcd_panel_reset(panel);
        raw::esp_lcd_panel_init(panel);
        raw::esp_lcd_panel_invert_color(panel, true);
        raw::esp_lcd_panel_swap_xy(panel, true);
        if flip {
            raw::esp_lcd_panel_mirror(panel, false, true);
        } else {
            raw::esp_lcd_panel_mirror(panel, true, false);
        }
        // the gap is specific to the panel
        raw::esp_lcd_panel_set_gap(panel, 0, 35);
    }
    // SAFETY: as above
    check("display on", unsafe {
        raw::esp_lcd_panel_disp_on_off(panel, true)
    })?;

    log!(Info, TAG, "Turn on LCD backlight");
    gpio_level(PIN_PWR, true);
    gpio_level(PIN_BK_LIGHT, true);

    log!(Info, TAG, "Initialize LVGL library");
    // SAFETY: once, before any other LVGL call (the caller holds the lock)
    unsafe { raw::lv_init() };
    // SAFETY: plain allocation, never freed (LVGL draws into it)
    let buf = unsafe {
        raw::heap_caps_malloc(
            BUF_PX * core::mem::size_of::<raw::lv_color_t>(),
            raw::MALLOC_CAP_DMA | raw::MALLOC_CAP_8BIT,
        )
    };
    if buf.is_null() {
        log!(Error, TAG, "no memory for the draw buffer");
        return None;
    }
    log!(Info, TAG, "Register display driver to LVGL");
    // SAFETY: `draw_buf` and `drv` are zeroed, leaked (they live forever)
    // and only LVGL uses them from here on
    unsafe {
        raw::lv_disp_draw_buf_init(draw_buf, buf, core::ptr::null_mut(), BUF_PX as u32);
        raw::lv_disp_drv_init(drv);
        (*drv).hor_res = H_RES;
        (*drv).ver_res = V_RES;
        (*drv).flush_cb = Some(flush);
        (*drv).draw_buf = draw_buf;
        (*drv).user_data = panel.cast();
        raw::lv_disp_drv_register(drv);
    }
    Some(())
}

fn buttons_init() {
    for pin in [PIN_BUTTON_1, PIN_BUTTON_2] {
        // SAFETY: plain value arguments (input pins)
        unsafe {
            raw::esp_rom_gpio_pad_select_gpio(pin as u32);
            raw::gpio_set_direction(pin, raw::gpio_mode_t_GPIO_MODE_INPUT);
            raw::gpio_set_pull_mode(pin, raw::gpio_pull_mode_t_GPIO_PULLUP_ONLY);
        }
    }
}

/// The LVGL task: timers, buttons, messages, state machine.
unsafe extern "C" fn task(_: *mut c_void) {
    with(|d| {
        d.turn_on();
        d.last_keypress_us = now_us();
        d.enter(State::Splash1, now_us());
    });
    loop {
        let now = now_us();
        let sleep = with(Driver::tick);
        crate::idf::task::delay_ms(sleep);

        if crate::power_mgmt::is_shutdown() {
            if with(|d| d.enter(State::PowerOff, now)) {
                crate::power_mgmt::shutdown();
            }
            crate::idf::task::delay_ms(sleep);
            continue;
        }
        if with(|d| d.step(now)) {
            crate::power_mgmt::shutdown();
        }
    }
}

/// The functions for the rest of the firmware ([`crate::display`]).
pub mod api {
    use super::*;

    /// Sets up the panel, LVGL, the screens and the LVGL task; on a second
    /// call (after the self test) the splash screens start over.
    pub fn init() {
        let mut g = driver();
        if let Some(d) = g.as_mut() {
            if let Some(s) = d.ui.screen(Screen::Splash1) {
                s.load(&d.l, Anim::None, 0, 0);
            }
            d.enter(State::Splash1, now_us());
            return;
        }
        log!(Info, c"INFO", "Setting Up TDisplayS3 Screen");
        buttons_init();
        if init_panel().is_none() {
            return;
        }
        // SAFETY: the lock (`g`) is held, and the token lives in the driver
        // behind it
        let l = unsafe { Lvgl::assume_locked() };
        let ui = Ui::new_loaded(&l);
        log!(Info, TAG, "Display LVGL animation");
        let mut d = Driver {
            l,
            ui,
            state: State::Nop,
            state_start_us: 0,
            animations: false,
            animation_cycles: 0,
            last_keypress_us: 0,
            on: false,
            overlay: false,
            found_block: false,
            identify_ms: 60_000,
            auto_off: false,
            auto_off_us: crate::config::cfg::SCREEN_OFF_DEFAULT_S as i64 * 1_000_000,
            show_found_block: true,
            countdown: None,
            shutdown: None,
            ignore_buttons_until_us: 0,
            buttons: [Button::new(LONGPRESS_MS), Button::new(LONGPRESS_MS)],
        };
        d.load_settings();
        *g = Some(d);
        STATE.store(State::Splash1 as u8, Ordering::Release);
        drop(g);
        // SAFETY: a static name; the task never returns
        let ok = unsafe {
            raw::xTaskCreatePinnedToCore(
                Some(task),
                c"lvgl Timer".as_ptr(),
                6000,
                core::ptr::null_mut(),
                4,
                core::ptr::null_mut(),
                1,
            )
        } == 1;
        if !ok {
            log!(Error, TAG, "no LVGL task");
        }
    }

    /// The settings (auto off, found-block image) from the config.
    pub fn load_settings() {
        with(Driver::load_settings)
    }

    /// Blocks until the splash screens are over.
    pub fn wait_for_splashs() {
        while STATE.load(Ordering::Acquire) < State::Wait as u8 {
            crate::idf::task::delay_ms(100);
        }
    }

    /// The access point screen with its SSID.
    pub fn portal_screen(ssid: &CStr) {
        with(|d| {
            d.ui.set_ssid(&d.l, ssid);
            d.enter(State::Portal, now_us());
        })
    }

    pub fn update_ip(ip: &CStr) {
        with(|d| d.ui.set_ip(&d.l, ip))
    }

    pub fn set_network_icon(eth: bool) {
        with(|d| {
            let icon = if eth { NetIcon::Eth } else { NetIcon::Wifi };
            d.ui.set_net_icon(&d.l, icon)
        })
    }

    pub fn set_can_icon() {
        with(|d| d.ui.set_net_icon(&d.l, NetIcon::Can))
    }

    pub fn mining_screen() {
        with(|d| {
            d.enter(State::Mining, now_us());
        })
    }

    pub fn show_found_block() {
        with(|d| {
            d.ui.hide_image(&d.l);
            if d.show_found_block {
                d.ui.show_found_block(&d.l);
                d.found_block = true;
            }
        })
    }

    /// The error overlay (replaces an open one).
    pub fn show_error(text: &CStr, code: u32) {
        with(|d| {
            d.ui.show_error(&d.l, text, code);
            d.overlay = true;
        })
    }

    pub fn hide_error() {
        with(|d| {
            d.ui.hide_error(&d.l);
            d.overlay = false;
        })
    }

    /// The values of all screens; `pool` the pool shown (0/1).
    pub fn update(pool: i32) {
        with(|d| {
            let (btc, global) = (d.state == State::Btc, d.state == State::Global);
            d.ui.update(&d.l, pool, btc, global);
            d.ui.update_settings(&d.l, pool);
        })
    }

    /// A text on the log screen (the self test).
    pub fn log(msg: &str) {
        let msg = CString::new(msg.replace('\0', "")).unwrap_or_default();
        with(|d| {
            d.ui.log(&d.l, &msg);
            d.animations = true;
            if let Some(s) = d.ui.screen(Screen::Log) {
                s.load(&d.l, Anim::None, 500, 0);
            }
            d.enter(State::Log, now_us());
        })
    }
}
