/*
 * The JSON documents of the HTTP GET endpoints (implemented in Rust,
 * components/rust_core/src/http_json.rs): the handlers collect the values
 * into these snapshots, the documents are built byte-identical to the former
 * ArduinoJson code. The layouts must match the Rust structs.
 */

#ifndef HTTP_JSON_H
#define HTTP_JSON_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* one pool of the stratum manager object */
typedef struct {
    bool connected;
    uint64_t accepted;
    uint64_t rejected;
    uint64_t best_session;
    double network_difficulty;
    uint32_t pool_difficulty;
    bool pool_diff_err;
    double ping_rtt; /* 0 without a ping task */
    double ping_loss;
    const char *verify_blocked; /* NULL: not blocked */
    int protocol;
    bool encrypted;
} manager_pool_t;

/* the stratum manager object (getManagerInfoJson) */
typedef struct {
    uint16_t config_pool_mode;
    int pool_mode;
    uint16_t pool_balance;
    uint64_t total_best_diff;
    bool fallback;
    bool using_fallback;
    bool dual;
    int selected;
    manager_pool_t pools[2];
    /* failover: the shared counters */
    uint64_t accepted;
    uint64_t rejected;
    uint64_t best_session;
    double network_difficulty;
    uint32_t pool_difficulty;
} manager_info_t;

/* one fan channel */
typedef struct {
    const char *label;
    int mode;
    uint16_t manual_speed;
    uint16_t overheat_temp;
    uint16_t rpm;
    uint16_t speed_perc;
    uint16_t target_temp;
    uint16_t p, i, d;
} fan_info_t;

/* GET /api/system/info */
typedef struct {
    int asic_count;
    int small_core_count;
    const char *device_model;
    const char *hostip;
    const char *mac_addr;
    int wifi_rssi;
    float power;
    float max_power;
    float min_power;
    float max_voltage;
    float min_voltage;
    float current; /* mA */
    float min_current_a;
    float max_current_a;
    float temp;
    float vr_temp;
    float vr_temp_int;
    uint64_t hashrate_timestamp;
    bool shutdown;
    float hashrate;
    double hashrate_1m;
    double hashrate_10m;
    double hashrate_1h;
    double hashrate_1d;
    int core_voltage;
    int default_core_voltage;
    float vout;
    uint16_t manual_fan_speed;
    int fan_count;
    fan_info_t fans[2];
    double last_ping_rtt;
    double recent_ping_loss;
    uint64_t duplicate_hw_nonces;
    uint32_t pool_difficulty;
    double network_difficulty;
    uint32_t found_blocks;
    uint32_t total_found_blocks;
    uint64_t shares_accepted;
    uint64_t shares_rejected;
    uint64_t best_diff;
    uint64_t best_session_diff;
    manager_info_t manager;
    const float *asic_temps; /* asic_count entries */
    bool history_requested;
    uint64_t history_start;
    uint64_t history_span_ms;
    uint64_t history_current;
    uint32_t history_limit;
    uint64_t now_ms;
    bool pid_available;
    uint16_t pid_target_temp; /* channel 0 */
    uint16_t pid_p, pid_i, pid_d;
    const char *hostname;
    const char *ssid;
    const char *stratum_url;
    uint16_t stratum_port;
    const char *stratum_user;
    bool stratum_enonce_subscribe;
    bool stratum_tls;
    const char *fallback_stratum_url;
    uint16_t fallback_stratum_port;
    const char *fallback_stratum_user;
    bool fallback_stratum_enonce_subscribe;
    bool fallback_stratum_tls;
    uint16_t stratum_protocol;
    uint16_t fallback_stratum_protocol;
    const char *sv2_authority_pubkey;
    const char *fallback_sv2_authority_pubkey;
    uint16_t sv2_channel_type;
    uint16_t fallback_sv2_channel_type;
    float voltage; /* mV */
    int frequency;
    int default_frequency;
    int job_interval;
    uint32_t stratum_difficulty;
    uint16_t overheat_temp;
    bool flip_screen;
    bool invert_screen;
    bool auto_screen_off;
    bool invert_fan_polarity;
    bool stratum_keep;
    uint32_t vr_frequency;
    uint32_t default_vr_frequency;
    bool otp;
    const char *asic_model;
    int64_t uptime_seconds;
    const char *last_reset_reason;
    const char *wifi_status;
    uint64_t free_heap;
    uint64_t free_heap_int;
    const char *version;
    const char *running_partition;
    const char *default_theme;
} system_info_t;

/* GET /api/system/asic */
typedef struct {
    const char *asic_model;
    const char *device_model;
    int asic_count;
    int default_frequency;
    int default_voltage;
    int abs_max_frequency;
    int abs_min_voltage;
    int abs_max_voltage;
    int eco_frequency;
    int eco_voltage;
    const char *swarm_color;
    const uint32_t *frequency_options;
    size_t frequency_count;
    const uint32_t *voltage_options;
    size_t voltage_count;
} asic_info_t;

/* one pool's coinbase data of the dashboard */
typedef struct {
    uint32_t block_height;
    double network_difficulty;
    const char *scriptsig;
    uint64_t total_value_satoshis;
    uint64_t user_value_satoshis;
    bool verification_ok;
    uint32_t fail_count;
    uint32_t check_count;
    uint16_t verify_mode;
    uint16_t max_fee; /* tenths of a percent */
    bool force;
} dash_coinbase_t;

/* GET /api/v2/dashboard */
typedef struct {
    int64_t uptime_seconds;
    bool shutdown;
    int board_error;
    uint16_t overheat_temp;
    /* fan channel 1's shutdown temperature (the VR) */
    uint16_t vr_overheat_temp;
    uint64_t hashrate_timestamp;
    float hashrate;
    double hashrate_1m;
    double hashrate_10m;
    double hashrate_1h;
    double hashrate_1d;
    uint64_t best_diff;
    uint64_t best_session_diff;
    uint64_t shares_accepted;
    uint64_t shares_rejected;
    int frequency;
    int asic_count;
    int small_core_count;
    const float *chip_hashrate; /* asic_count entries each */
    const float *chip_error_rate;
    const float *asic_temps;
    float watts;
    float min_power;
    float max_power;
    float voltage; /* mV */
    float min_voltage;
    float max_voltage;
    float current; /* mA */
    float min_current_a;
    float max_current_a;
    float vout;
    float asic_temp;
    float vr_temp;
    float vr_temp_int;
    int fan_count;
    uint16_t fan_speed[2];
    uint16_t fan_rpm[2];
    manager_info_t manager;
    const char *pool_host[2];
    int pool_port[2];
    const char *pool_user[2];
    bool has_can_extension;
    bool can_enabled;
    dash_coinbase_t coinbase[2];
    bool history_requested;
    uint64_t history_start;
    uint64_t history_span_ms;
    uint64_t history_current;
    uint32_t history_limit;
    uint64_t now_ms;
} dashboard_t;

/* GET /api/v2/system */
typedef struct {
    const char *device_model;
    const char *asic_model;
    const char *version;
    int64_t uptime_seconds;
    const char *last_reset_reason;
    const char *hostname;
    const char *ssid;
    const char *mac_addr;
    const char *ip_addr;
    const char *wifi_status;
    int wifi_rssi;
    uint64_t free_heap;
    uint64_t free_heap_int;
} v2_system_t;

/* GET /api/v2/identify */
typedef struct {
    const char *device_model;
    const char *default_theme;
    bool otp;
    bool ap_active;
    bool core_dump_download;
    bool can_enabled;
} v2_identify_t;

/* GET /api/alert/info */
typedef struct {
    bool has_webhook;
    bool watchdog;
    bool block_found;
    bool best_diff;
    bool coinbase_verify;
    bool show_block_found_screen;
} alert_info_t;

/* GET /api/influx/info */
typedef struct {
    const char *url;
    uint16_t port;
    const char *bucket;
    const char *org;
    const char *prefix;
    bool enabled;
} influx_info_t;

/* one scanned access point */
typedef struct {
    const char *ssid;
    int8_t rssi;
    int authmode;
} wifi_network_t;

/* one pool of GET /api/v2/settings */
typedef struct {
    const char *url;
    uint16_t port;
    const char *user;
    bool enonce_subscribe;
    bool tls;
    uint16_t protocol;
    const char *sv2_authority_pubkey;
    uint16_t sv2_channel_type;
    bool sv2_require_auth;
    uint16_t coinbase_verify_mode;
    uint16_t coinbase_max_fee; /* tenths of a percent */
    bool coinbase_verify_force;
} settings_pool_t;

/* GET /api/v2/settings */
typedef struct {
    const char *asic_model;
    const char *device_model;
    const char *version;
    bool otp;
    bool ap_active;
    bool has_can_extension;
    bool can_enabled;
    int frequency;
    int core_voltage;
    uint32_t vr_frequency;
    int default_frequency;
    int default_core_voltage;
    uint32_t default_vr_frequency;
    int eco_frequency;
    int eco_core_voltage;
    int abs_min_core_voltage;
    int abs_max_core_voltage;
    const uint32_t *frequency_options;
    size_t frequency_count;
    const uint32_t *voltage_options;
    size_t voltage_count;
    uint16_t pool_mode;
    uint16_t pool_balance;
    bool stratum_keep;
    int job_interval;
    uint32_t stratum_difficulty;
    settings_pool_t pools[2];
    bool pid_available;
    int fan_count;
    fan_info_t fans[2];
    bool invert_fan_polarity;
    bool has_pid_use_max; /* NerdAxe/NerdAxeGamma builds report pidUseMax */
    bool pid_use_max;
    const char *hostname;
    const char *ssid;
    bool mempool_custom;
    const char *mempool_url;
    bool flip_screen;
    bool invert_screen;
    bool auto_screen_off;
} v2_settings_t;

/* the master board's node of GET /api/v2/can/swarm (the slaves come from
 * the CAN master) */
typedef struct {
    const char *mac;
    const char *device_model;
    const char *version;
    bool shutdown;
    float hashrate; /* getCurrentHashrate(), 0 in a shutdown */
    float power;
    float temp;
    float vr_temp;
    float current; /* mA */
    float vout;
    int board_error;
    int free_heap_int;
    int frequency;
    int core_voltage;
    bool flip_screen;
    bool auto_screen_off;
    int asic_count;
    const float *asic_temps;
    int fan_count;
    fan_info_t fans[2]; /* label/p/i/d unused */
} can_master_node_t;

/* sizeof of the Rust structs: 0 manager_info_t, 1 fan_info_t,
 * 2 system_info_t, 3 asic_info_t, 4 dashboard_t, 5 v2_system_t,
 * 6 v2_identify_t, 7 alert_info_t, 8 influx_info_t, 9 wifi_network_t,
 * 10 settings_pool_t, 11 v2_settings_t, 12 can_master_node_t */
size_t http_json_sizeof(int which);

#ifdef __cplusplus
}
#endif

#endif /* HTTP_JSON_H */
