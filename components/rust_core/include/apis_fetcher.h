/*
 * The mempool API fetcher (implemented in Rust,
 * components/rust_core/src/apis_fetcher.rs): price, block height, network
 * hashrate/difficulty and fees for the display.
 */

#ifndef APIS_FETCHER_H
#define APIS_FETCHER_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* the task entry (one fetch, then every 60 s while enabled) */
void apis_task(void *arg);
void apis_enable_fetching(void);
uint32_t apis_get_price(void);
uint32_t apis_get_block_height(void);
uint32_t apis_get_blocks_to_halving(void);
uint32_t apis_get_halving_percent(void);
uint64_t apis_get_net_hash(void);
uint64_t apis_get_net_difficulty(void);
uint32_t apis_get_lowest_fee(void);
uint32_t apis_get_mid_fee(void);
uint32_t apis_get_fastest_fee(void);

#ifdef __cplusplus
}
#endif

#endif /* APIS_FETCHER_H */
