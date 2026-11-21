/*
 * Discord alerts (implemented in Rust, components/rust_core/src/discord.rs
 * and alerts.rs): settings, sender task, HTTPS POST and the messages.
 */

#ifndef DISCORD_H
#define DISCORD_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* reads the alert settings and starts the sender task */
void discord_start(void);
/* queue an alert; false if it is disabled or the queue is full */
bool discord_send_test(void);
bool discord_send_watchdog(void);
bool discord_send_block_found(double diff, double network_diff);
bool discord_send_best_diff(double diff, double network_diff);
bool discord_send_coinbase_verify_failed(int pool, uint64_t user_sats, uint64_t total_sats, int mode);

#ifdef __cplusplus
}
#endif

#endif /* DISCORD_H */
