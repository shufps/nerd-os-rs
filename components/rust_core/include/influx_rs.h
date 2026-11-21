/*
 * InfluxDB v2 telemetry (implemented in Rust, components/rust_core/src/influx.rs):
 * the power, temperature and fan samples are averaged between writes; every
 * 15 s one line-protocol record is written, after the bucket is checked (or
 * created) and the last totals are loaded. The HTTP requests are a hook.
 */

#ifndef INFLUX_RS_H
#define INFLUX_RS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Telemetry samples (power management task), averaged per record. */
void influx_task_set_temperature(float temp, float temp2);
void influx_task_set_pwr(float vin, float iin, float pin, float vout, float iout, float pout);
void influx_set_fan(float pwm0, float rpm0, float pwm1, float rpm1);

#ifdef __cplusplus
}
#endif

#endif /* INFLUX_RS_H */
