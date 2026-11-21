/*
 * The board (implemented in Rust, components/rust_core/src/board.rs): the
 * constants and settings, the hardware of all NerdQAxe and NerdAxe boards
 * (regulator, fans, temperature sensors, ethernet and CAN options) and the
 * power sequencing. Wrapped by the inline class in main/boards/board.h.
 */

#ifndef BOARD_RS_H
#define BOARD_RS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "board_profile.h"

#ifdef __cplusplus
extern "C" {
#endif

/* the constants and settings the getters read */
typedef struct {
    const char *device_model;
    const char *mining_agent;
    const char *asic_model;
    const char *swarm_color;
    const char *default_theme;
    const char *fan_labels[2];
    int version;
    int asic_count;
    int chips_detected;
    int num_temp_sensors;
    int num_fans;
    int job_interval_ms;
    int asic_frequency;
    int asic_voltage_mv;
    int default_frequency;
    int default_voltage_mv;
    int abs_max_frequency;
    int abs_min_voltage_mv;
    int abs_max_voltage_mv;
    int eco_frequency;
    int eco_voltage_mv;
    int fan_mode[2];
    board_pid_t pid[2];
    uint32_t vr_frequency;
    uint32_t default_vr_frequency;
    uint32_t min_difficulty;
    uint32_t min_difficulty_dual_pool;
    uint32_t max_difficulty;
    float vr_max_temp;
    float fan_perc;
    float max_pin;
    float min_pin;
    float max_vin;
    float min_vin;
    float min_current_a;
    float max_current_a;
    bool fan_invert;
    bool flip_screen;
    bool has_hash_counter;
    bool initialized;
    bool buck_initialized;
    bool shutdown;
    const uint32_t *frequencies;
    size_t frequency_count;
    const uint32_t *voltages;
    size_t voltage_count;
} board_info_t;

/* creates the board BOARD_* (detections, profile); false for an unknown id */
bool board_create(int id);
/* the board's asic_chip_t, and its driver (asic_driver_t) */
int board_asic_chip(void);

void board_info(board_info_t *out);
size_t board_info_sizeof(void);
bool board_is_initialized(void);
bool board_is_shutdown(void);
/* the NerdAxe family (NerdAxe, NerdAxeGamma, NerdAxeGaia) */
bool board_is_axe(void);

bool board_init(void);
bool board_init_asics(void);
void board_load_settings(void);
void board_shutdown(void);
bool board_set_voltage(float volts);
bool board_set_asic_frequency(float mhz);
void board_set_vr_frequency(uint32_t hz);
/* the limit checks with their log message */
bool board_validate_voltage_logged(float volts);
bool board_validate_frequency_logged(float mhz);

void board_set_fan_speed_ch(int ch, float perc);
void board_set_fan_speed(float perc);
uint16_t board_fan_rpm(int ch);
void board_set_fan_polarity(bool invert);

float board_temperature(int index);
float board_vr_temp(void);
float board_vr_temp_internal(void);
/* 0 vin, 1 iin, 2 pin, 3 vout, 4 iout, 5 pout */
float board_power_value(int what);
void board_request_buck_telemetry(void);
void board_request_chip_temps(void);
/* Board::Error, the PMBus status into *status */
int board_fault(uint32_t *status);

bool board_has_ethernet(void);
/* SCLK, MOSI, MISO, CS of an ethernet HAT; false without */
bool board_eth_pins(int out[4]);
void board_detect_ethernet(void);
bool board_has_can_extension(void);
bool board_is_can_slave(void);
int board_can_tx_pin(void);
int board_can_rx_pin(void);

#ifdef __cplusplus
}
#endif

#endif /* BOARD_RS_H */
