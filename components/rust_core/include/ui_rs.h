/*
 * The display screens and their values (implemented in Rust,
 * components/rust_core/src/ui.rs). The display driver (main/displays) calls
 * these with its LVGL lock held and loads the screens it gets.
 */

#ifndef UI_RS_H
#define UI_RS_H

#include <stdbool.h>
#include <stdint.h>

#include "lvgl.h"

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
    UI_SCREEN_SPLASH1 = 0,
    UI_SCREEN_SPLASH2,
    UI_SCREEN_PORTAL,
    UI_SCREEN_MINING,
    UI_SCREEN_SETTINGS,
    UI_SCREEN_BTC,
    UI_SCREEN_GLOBAL,
    UI_SCREEN_LOG,
    UI_SCREEN_QR,
    UI_SCREEN_POWER_OFF,
} ui_screen_t;

/* creates the screens, loads the first splash screen; driver gets the
 * screen load events (disp_screen_loaded()) */
void ui_init(void *driver);
/* a screen, NULL if not created (log, QR, power-off are made on demand) */
lv_obj_t *ui_screen(int which);
/* empties a splash screen once it is no longer shown */
void ui_drop_splash(int which);
void ui_set_ssid(const char *ssid);
/* the "Connecting..." text of the second splash screen */
void ui_set_connect(const char *msg);
/* the values of all screens; pool: the pool shown (0/1); the BTC price and
 * global stats only while their screen is shown (and once at the start) */
void ui_update(int pool, bool btc_shown, bool global_shown);
/* pool, frequency, voltage and fan setting of the settings screen */
void ui_update_settings(int pool);
void ui_set_ip(const char *ip);
/* 0 WiFi, 1 Ethernet, 2 CAN */
void ui_set_net_icon(int kind);
void ui_show_error(const char *msg, uint32_t code);
void ui_hide_error(void);
void ui_show_found_block(void);
void ui_hide_image_overlay(void);
void ui_power_off_init(void);
/* the log screen's text (creates the screen) */
void ui_log(const char *msg);
/* the OTP QR screen from a qrcodegen buffer; false without it */
bool ui_create_qr(const uint8_t *qr, int size);

/* implemented by the display driver (display_api.cpp) */
void disp_screen_loaded(void *driver);

#ifdef __cplusplus
}
#endif

#endif /* UI_RS_H */
