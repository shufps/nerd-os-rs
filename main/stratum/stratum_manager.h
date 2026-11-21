#pragma once

#include "pool_manager.h"
#include "pool_policy.h"
#include "utils.h"

#define DIFF_STRING_SIZE 12

/**
 * @brief The pool manager: pool selection, connections, failover or dual
 * pool mining. Implemented in Rust (pool_manager.h); this class keeps the
 * C++ call sites.
 */
class StratumManager {
  public:
    enum Selected
    {
        PRIMARY = 0,
        SECONDARY = 1
    };

    enum PoolMode
    {
        FAILOVER = 0,
        DUAL = 1
    };

    explicit StratumManager(PoolMode mode) { pool_mgr_create(mode == PoolMode::DUAL); }

    // the manager task (starts the stratum and ping tasks)
    static void taskWrapper(void *pvParameters) { pool_mgr_task(pvParameters); }

    // Submit shares to the pool
    // version_rolled = full rolled version (base | rolled bits)
    // version_base   = original block template version
    void submitShare(int pool, const char *jobid, const char *extranonce_2, const uint32_t ntime, const uint32_t nonce,
                     const uint32_t version_rolled, const uint32_t version_base)
    {
        pool_mgr_submit_share(pool, jobid, extranonce_2, ntime, nonce, version_rolled, version_base);
    }

    void checkForBestDiff(int pool, double diff, uint32_t nbits) { pool_mgr_check_best_diff(pool, diff, nbits); }
    void checkForFoundBlock(int pool, double diff, uint32_t nbits) { pool_mgr_check_found_block(pool, diff, nbits); }

    bool isConnected(int pool) { return pool_mgr_is_connected(pool); }
    bool isAnyConnected() { return isConnected(PRIMARY) || isConnected(SECONDARY); }
    int getNumConnectedPools() { return !!isConnected(PRIMARY) + !!isConnected(SECONDARY); }

    void reconnectAll() { pool_mgr_reconnect_all(); }

    int getPoolMode() const { return pool_mgr_is_dual() ? DUAL : FAILOVER; }
    bool isDualPool() const { return pool_mgr_is_dual(); }
    bool isFallback() const { return !pool_mgr_is_dual(); }
    bool isInitialized() { return pool_mgr_is_initialized(); }

    // the values of the manager object (http_json.h)
    void getManagerInfo(manager_info_t *info) { pool_mgr_info(info); }

    // reloads the pool configs (and the dual pool balance) and reconnects
    // the pools whose settings changed
    void loadSettings() { pool_mgr_load_settings(); }

    void resetSessionStats() { pool_mgr_reset_session_stats(); }
    // Reset the persisted all-time stats (best difficulty, found blocks)
    void resetAllTimeStats() { pool_mgr_reset_all_time_stats(); }

    // failover: mining on the secondary pool
    bool isUsingFallback() { return pool_mgr_using_fallback(); }

    // the pool for the next job
    int getNextActivePool() { return pool_mgr_next_active_pool(); }

    uint32_t selectAsicDiff(int pool, uint32_t poolDiff) { return pool_mgr_select_asic_diff(pool, poolDiff); }

    // for per-pool views: the pool shown for pool (failover: the selected one)
    int displayPool(int pool) { return pool_mgr_display_pool(pool); }
    // the host of a pool, copied (the config may be reloaded meanwhile)
    void getPoolHost(int pool, char *buf, size_t len) { pool_mgr_pool_host(pool, buf, len); }
    int getPoolPort(int pool) { return pool_mgr_pool_port(pool); }
    // share of the hashrate (percent) a pool currently gets; dual pool only
    int getActivePoolBalance(int pool) { return pool_mgr_active_balance(pool); }

    int getPoolErrors() { return pool_mgr_pool_errors(); }

    // statistics: one consistent snapshot each (aggregated over both pools in
    // dual pool mode)
    pool_stats_summary_t getStats()
    {
        pool_stats_summary_t s;
        pool_mgr_summary(&s);
        return s;
    }
    pool_stats_pool_t getPoolStats(int pool)
    {
        pool_stats_pool_t s;
        pool_mgr_pool_stats(pool, &s);
        return s;
    }
    uint64_t getSharesAccepted() { return getStats().accepted; }
    uint64_t getSharesRejected() { return getStats().rejected; }
    uint64_t getSharesAccepted(int pool) { return getPoolStats(pool).accepted; }
    uint64_t getSharesRejected(int pool) { return getPoolStats(pool).rejected; }
    uint32_t getTotalFoundBlocks() { return getStats().total_found_blocks; }
    uint32_t getFoundBlocks() { return getStats().found_blocks; }
    uint64_t getBestDiff() { return getStats().total_best; }
    uint64_t getBestSessionDiff() { return getStats().best_session; }
    uint32_t getPoolDifficulty() { return getStats().pool_difficulty; }
    double getNetworkDifficulty() { return getStats().network_difficulty; }

    // the all-time best difficulty as "1.23M" into buf
    void getBestDiffString(char *buf, size_t len) { suffixString(getBestDiff(), buf, len, 0); }
};

// ping of the pool the v1 API shows (0 without ping)
inline double get_last_ping_rtt() { return pool_mgr_ping_rtt(); }
inline double get_recent_ping_loss() { return pool_mgr_ping_loss(); }
