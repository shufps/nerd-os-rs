/*
 * Pool selection and coinbase verification decisions of the stratum manager
 * (implemented in Rust, components/rust_core/src/pool.rs). The managers pass
 * the current state in and carry out the returned actions.
 */

#ifndef POOL_POLICY_H
#define POOL_POLICY_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* What the manager knows about both pools when an event happens. */
typedef struct {
    bool has_task[2];
    bool connected[2]; // isConnected(): the task exists and is connected
    bool verify_blocked[2];
} pool_state_t;

/* What the manager has to do, per pool; apply in this order. */
typedef struct {
    bool invalidate[2];          // create_job_invalidate() and clear the valid-notify flag
    bool disconnect[2];
    bool stop_timer[2];          // stop the reconnect timer
    bool start_timer[2];         // start the reconnect timer
    bool connect[2];
    bool reset_verify_counts[2]; // reset the verification counters
} pool_actions_t;

/* Why a pool is blocked by the coinbase verification. */
enum { POOL_BLOCK_NONE = 0, POOL_BLOCK_ADDRESS_NOT_FOUND = 1, POOL_BLOCK_FEE_EXCEEDED = 2 };

typedef struct {
    uint16_t mode;    // 0 off, 1 address must be paid, 2 also the fee below max_fee
    uint16_t max_fee; // in tenths of a percent
    bool force;       // block (and reconnect) the pool when the verification fails
    uint64_t user_value_satoshis;
    uint64_t total_value_satoshis;
    uint32_t block_height;
    bool was_ok;
    uint8_t blocked; // POOL_BLOCK_*
} pool_verify_input_t;

typedef struct {
    bool ok;
    bool counted;     // count the check (and a failure if !ok)
    bool alert;       // just turned from ok to failed: send the alert
    uint8_t blocked;  // new block reason, POOL_BLOCK_*
    bool reconnect;   // just got blocked: reconnect the pool
    bool check_fault; // re-evaluate whether any configured pool is usable
} pool_verify_result_t;

#ifdef __cplusplus
static_assert(sizeof(pool_state_t) == 6 && sizeof(pool_actions_t) == 12 && sizeof(pool_verify_input_t) == 32 &&
                  sizeof(pool_verify_result_t) == 6,
              "pool_policy.h layout must match rust_core");
#else
_Static_assert(sizeof(pool_state_t) == 6 && sizeof(pool_actions_t) == 12 && sizeof(pool_verify_input_t) == 32 &&
                   sizeof(pool_verify_result_t) == 6,
               "pool_policy.h layout must match rust_core");
#endif

#ifdef __cplusplus
}
#endif

#endif /* POOL_POLICY_H */
