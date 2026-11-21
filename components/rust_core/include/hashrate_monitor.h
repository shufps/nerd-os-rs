/*
 * Hashrate from the chips' nonce (0x90) and hardware error (0x4c) counters
 * (implemented in Rust, components/rust_core/src/hashrate.rs): rate per
 * chip in GH/s, the total through a 5-tap median and a smoothing step.
 * Thread-safe.
 */

#ifndef HASHRATE_MONITOR_H
#define HASHRATE_MONITOR_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void hashrate_monitor_init(int asic_count);
/* starts the measurement task (src/tasks.rs); false without initialized ASICs */
bool hashrate_monitor_start(void);
/* counter replies of chip asic (the replies of other chips are ignored) */
void hashrate_monitor_on_counter(int asic, uint32_t counter);
void hashrate_monitor_on_errors(int asic, uint32_t counter);

float hashrate_monitor_chip(int asic);
float hashrate_monitor_chip_errors(int asic);
float hashrate_monitor_total(void);
float hashrate_monitor_smoothed(void);
/* the CAN slaves' hashrate */
void hashrate_monitor_set_external(float ghs);
float hashrate_monitor_external(void);

#ifdef __cplusplus
}
#endif

#endif /* HASHRATE_MONITOR_H */
