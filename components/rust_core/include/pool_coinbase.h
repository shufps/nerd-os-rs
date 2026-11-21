/*
 * Coinbase state of the pool manager (implemented in Rust,
 * components/rust_core/src/pool_coinbase.rs): per pool the extranonce, the
 * last decoded coinbase, the verification counters and the block reason.
 * Thread-safe. The caller sends the alert, reconnects and sets the board
 * fault flag as the returned outcome says.
 */

#ifndef POOL_COINBASE_H
#define POOL_COINBASE_H

#include <stdbool.h>
#include <stdint.h>

#include "coinbase_decoder.h"

#ifdef __cplusplus
extern "C" {
#endif

/* The verification settings of a pool (from the config). */
typedef struct {
    uint16_t mode;    /* 0 off, 1 address paid, 2 also pool fee <= max_fee */
    uint16_t max_fee; /* tenths of a percent */
    bool force;       /* block and reconnect a failing pool */
} pool_verify_config_t;

typedef struct {
    bool verified;     /* the verification ran */
    bool alert;        /* send the verification failed alert */
    bool reconnect;    /* the pool just got blocked: reconnect it */
    bool check_fault;  /* re-evaluate whether any configured pool is usable */
    bool parse_failed; /* the coinbase could not be decoded */
} pool_verify_outcome_t;

#ifdef __cplusplus
}
#endif

#endif /* POOL_COINBASE_H */
