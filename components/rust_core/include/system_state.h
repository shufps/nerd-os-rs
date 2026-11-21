/*
 * The system module (implemented in Rust, components/rust_core/src/system.rs):
 * the device state the HTTP API and the display read, the board error, the
 * current hashrate and the system task (display updates and history
 * samples every 5 s). Wrapped by the inline class in main/system.h.
 * Thread-safe; the changing strings are copied out.
 */

#ifndef SYSTEM_STATE_H
#define SYSTEM_STATE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* start time, host name (from the config), the history */
void system_init(int asic_count);
/* the task entry (xTaskCreatePSRAM) */
void system_task(void *arg);
/* esp_timer_get_time() at init */
int64_t system_start_us(void);

void system_set_wifi_status(const char *status); /* at most 19 characters */
void system_set_ssid(const char *ssid);          /* at most 32 characters */
/* copies into buf (len bytes, NUL-terminated) */
void system_wifi_status(char *buf, size_t len);
void system_ip(char *buf, size_t len);
/* fixed after system_init() */
const char *system_hostname(void);

void system_set_ap_state(bool on);
bool system_ap_state(void);
void system_set_startup_done(void);
bool system_startup_done(void);

/* reads and logs the reset reason; the esp_reset_reason_t */
int system_read_reset_reason(void);
/* "SYSTEM.RESET_..." (static) */
const char *system_reset_reason(void);

/* GH/s: the ASIC counters if the board has them, else the 10 min average */
float system_current_hashrate(void);

/* Board::Error; set: a hardware fault, replaces any other error;
 * set_if_none: only without an error; clear: only if it is `error` */
void system_set_error(int error, uint32_t code);
void system_set_error_if_none(int error, uint32_t code);
void system_clear_error(int error);
/* the error, its code into *code (if not NULL) */
int system_error(uint32_t *code);

#ifdef __cplusplus
}
#endif

#endif /* SYSTEM_STATE_H */
