/*
 * The display as C functions for the Rust system task (implemented in C++ by
 * main/displays/display_api.cpp; the display stays C++/LVGL).
 */

#ifndef DISPLAY_API_H
#define DISPLAY_API_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* the display settings (flip, brightness, ...) from the config */
void disp_load_settings(void);
void disp_wait_for_splashs(void);
/* the access point page with its SSID */
void disp_portal_screen(const char *ssid);
void disp_update_ip(const char *ip);
void disp_set_network_icon(bool eth);
void disp_set_can_icon(void);
void disp_mining_screen(void);
void disp_show_found_block(void);
void disp_show_error(const char *text, uint32_t code);
void disp_hide_error(void);
/* the values of all pages, then a refresh; toggle: the pool shown (0/1) */
void disp_update(int toggle);

/* implemented in Rust (display_shot.rs): every flushed area, for the
 * development screenshot route (a no-op without it) */
void disp_shadow_flush(int x1, int y1, int x2, int y2, const uint16_t *px);
/* simulated short presses (bit 0: button 1, bit 1: button 2) of the
 * development route, taken once; always 0 without it */
uint32_t disp_debug_take_press(void);

#ifdef __cplusplus
}
#endif

#endif /* DISPLAY_API_H */
