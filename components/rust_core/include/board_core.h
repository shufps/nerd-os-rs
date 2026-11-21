/*
 * Board-independent logic of the Board classes (implemented in Rust,
 * components/rust_core/src/board_core.rs): frequency/voltage limits, the
 * chip temperatures, the TPS536x7 PMBus fault decoding.
 */

#ifndef BOARD_CORE_H
#define BOARD_CORE_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* the absolute limits of a board (0: no limit) */
typedef struct {
    int abs_max_frequency;
    int abs_min_voltage_mv;
    int abs_max_voltage_mv;
} board_limits_t;

/* the ASIC chip temperatures (0 for an unknown chip) */
void board_chip_temp_set(int nr, float temp);
float board_chip_temp(int nr);
float board_chip_temp_max(void);

#ifdef __cplusplus
}
#endif

#endif /* BOARD_CORE_H */
