/*
 * The pool manager and the stratum tasks (implemented in Rust,
 * components/rust_core/src/pool_manager.rs): pool selection (failover or
 * dual pool), the connections (Stratum v1 over TCP/TLS, Stratum v2 over
 * Noise), share submission, statistics, coinbase verification and the ping
 * tasks. Thread-safe; the calls before
 * pool_mgr_create() return neutral values.
 */

#ifndef POOL_MANAGER_H
#define POOL_MANAGER_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "http_json.h"
#include "pool_coinbase.h"
#include "pool_stats.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Creates the manager (once); dual: dual pool mode, else failover. */
void pool_mgr_create(bool dual);
bool pool_mgr_exists(void);
/* The manager task (starts the stratum and ping tasks); never returns. */
void pool_mgr_task(void *arg);
bool pool_mgr_is_dual(void);
bool pool_mgr_is_initialized(void);

/* version_rolled: the full rolled version, version_base: the job's version */
void pool_mgr_submit_share(int pool, const char *job_id, const char *extranonce2, uint32_t ntime, uint32_t nonce,
                           uint32_t version_rolled, uint32_t version_base);
/* best and found block bookkeeping of a nonce: persists, sends the alerts */
void pool_mgr_check_best_diff(int pool, double diff, uint32_t nbits);
void pool_mgr_check_found_block(int pool, double diff, uint32_t nbits);
/* the pool for the next job; -1 before the manager exists */
int pool_mgr_next_active_pool(void);
uint32_t pool_mgr_select_asic_diff(int pool, uint32_t pool_diff);

bool pool_mgr_is_connected(int pool);
void pool_mgr_reconnect_all(void);
/* reloads the pool configs (and the balance), reconnects changed pools */
void pool_mgr_load_settings(void);
void pool_mgr_reset_session_stats(void);
void pool_mgr_reset_all_time_stats(void);
void pool_mgr_info(manager_info_t *info);

/* the host ("-" if none), copied under the manager lock */
void pool_mgr_pool_host(int pool, char *buf, size_t len);
int pool_mgr_pool_port(int pool);
/* dual pool: percent of the hashrate the pool gets */
int pool_mgr_active_balance(int pool);
/* the pool a per-pool view shows (failover: the selected one) */
int pool_mgr_display_pool(int pool);
bool pool_mgr_using_fallback(void);
int pool_mgr_pool_errors(void);

/* the statistics: aggregated, and of one pool (zeros before the manager
 * exists) */
void pool_mgr_summary(pool_stats_summary_t *out);
void pool_mgr_pool_stats(int pool, pool_stats_pool_t *out);

/* ping of the pool the v1 API shows: round trip (ms), loss (0..1) */
double pool_mgr_ping_rtt(void);
double pool_mgr_ping_loss(void);

#ifdef __cplusplus
}
#endif

#endif /* POOL_MANAGER_H */
