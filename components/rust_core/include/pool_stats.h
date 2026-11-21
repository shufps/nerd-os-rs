/*
 * Mining statistics of the pool manager (implemented in Rust,
 * components/rust_core/src/pool_stats.rs): shares, difficulties, best
 * difficulty, found blocks: the types of the snapshots the manager hands out.
 * Failover mode shares one set of counters for both pools; dual pool mode
 * counts per pool.
 */

#ifndef POOL_STATS_H
#define POOL_STATS_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Aggregates as the manager reported them (dual pool: sums, the pool with
 * the larger balance share for the difficulties, the larger session best). */
typedef struct {
    uint64_t accepted;
    uint64_t rejected;
    uint64_t best_session;
    uint64_t total_best;
    double network_difficulty;
    uint32_t pool_difficulty;
    uint32_t found_blocks;
    uint32_t total_found_blocks;
    uint64_t last_response_us; /* esp_timer_get_time() of the last share response, 0: none */
} pool_stats_summary_t;

/* One pool (failover: the shared counters). */
typedef struct {
    uint64_t accepted;
    uint64_t rejected;
    uint64_t best_session;
    double network_difficulty;
    uint32_t pool_difficulty;
} pool_stats_pool_t;

/* Read through pool_mgr_summary() and pool_mgr_pool_stats() (pool_manager.h). */

#ifdef __cplusplus
}
#endif

#endif /* POOL_STATS_H */
