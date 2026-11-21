/*
 * Settings request bodies of the HTTP API (implemented in Rust,
 * components/rust_core/src/http_settings.rs): the JSON body is parsed with
 * ArduinoJson's rules into typed writes, which the caller applies to the
 * config (main/http_server/settings_apply.cpp).
 */

#ifndef HTTP_SETTINGS_H
#define HTTP_SETTINGS_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* One config setter each. index: fan channel (FAN_*) or pool (STRATUM_*,
 * SV2_*, COINBASE_*: 0 primary, 1 fallback), 0 otherwise. */
typedef enum {
    SETTING_WIFI_SSID = 0,           /* text */
    SETTING_WIFI_PASS = 1,           /* text */
    SETTING_HOSTNAME = 2,            /* text */
    SETTING_ASIC_VOLTAGE = 3,        /* u16, > 0 */
    SETTING_ASIC_FREQUENCY = 4,      /* u16, > 0 */
    SETTING_ASIC_JOB_INTERVAL = 5,   /* u16, > 0 */
    SETTING_STRATUM_DIFFICULTY = 6,  /* u32 */
    SETTING_FLIP_SCREEN = 7,         /* bool */
    SETTING_OVERHEAT_TEMP = 8,       /* u16 */
    SETTING_INVERT_SCREEN = 9,       /* bool */
    SETTING_FAN_POLARITY = 10,       /* bool */
    SETTING_TEMP_CONTROL_MODE = 11,  /* u16 */
    SETTING_FAN_SPEED = 12,          /* u16 */
    SETTING_AUTO_SCREEN_OFF = 13,    /* bool */
    SETTING_STRATUM_KEEPALIVE = 14,  /* bool */
    SETTING_PID_TARGET_TEMP = 15,    /* u16 */
    SETTING_PID_P = 16,              /* u16, hundredths */
    SETTING_PID_I = 17,              /* u16, hundredths */
    SETTING_PID_D = 18,              /* u16, hundredths */
    SETTING_VR_FREQUENCY = 19,       /* u32 */
    SETTING_FAN_MODE = 20,           /* u16 [ch] */
    SETTING_FAN_MANUAL_SPEED = 21,   /* u16 [ch] */
    SETTING_FAN_OVERHEAT_TEMP = 22,  /* u16 [ch] */
    SETTING_FAN_PID_TARGET_TEMP = 23, /* u16 [ch] */
    SETTING_FAN_PID_P = 24,          /* u16 [ch], hundredths */
    SETTING_FAN_PID_I = 25,          /* u16 [ch], hundredths */
    SETTING_FAN_PID_D = 26,          /* u16 [ch], hundredths */
    SETTING_POOL_MODE = 27,          /* u16 */
    SETTING_STRATUM_URL = 28,        /* text [pool] */
    SETTING_STRATUM_USER = 29,       /* text [pool] */
    SETTING_STRATUM_PASS = 30,       /* text [pool] */
    SETTING_STRATUM_PORT = 31,       /* u16 [pool] */
    SETTING_STRATUM_ENONCE_SUBSCRIBE = 32, /* bool [pool] */
    SETTING_STRATUM_TLS = 33,        /* bool [pool] */
    SETTING_STRATUM_PROTOCOL = 34,   /* u16 [pool] */
    SETTING_SV2_AUTHORITY_PUBKEY = 35, /* text [pool] */
    SETTING_SV2_CHANNEL_TYPE = 36,   /* u16 [pool] */
    SETTING_MEMPOOL_CUSTOM = 37,     /* bool */
    SETTING_MEMPOOL_URL = 38,        /* text */
    SETTING_FAN_PID_USE_MAX = 39,    /* bool (NerdAxe, NerdAxeGamma only) */
    SETTING_CAN_ENABLED = 40,        /* bool */
    SETTING_POOL_BALANCE = 41,       /* u16 */
    SETTING_SV2_REQUIRE_AUTH = 42,   /* bool [pool] */
    SETTING_COINBASE_VERIFY_MODE = 43, /* u16 [pool] */
    SETTING_COINBASE_MAX_FEE = 44,   /* u16 [pool], tenths of a percent */
    SETTING_COINBASE_VERIFY_FORCE = 45, /* bool [pool] */
    SETTING_INFLUX_ENABLED = 46,     /* bool */
    SETTING_INFLUX_URL = 47,         /* text */
    SETTING_INFLUX_PORT = 48,        /* u16 */
    SETTING_INFLUX_TOKEN = 49,       /* text */
    SETTING_INFLUX_BUCKET = 50,      /* text */
    SETTING_INFLUX_ORG = 51,         /* text */
    SETTING_INFLUX_PREFIX = 52,      /* text */
    SETTING_DISCORD_WEBHOOK = 53,    /* text */
    SETTING_DISCORD_WATCHDOG = 54,   /* bool */
    SETTING_DISCORD_BLOCK_FOUND = 55, /* bool */
    SETTING_DISCORD_BEST_DIFF = 56,  /* bool */
    SETTING_DISCORD_COINBASE_VERIFY = 57, /* bool */
    SETTING_SHOW_BLOCK_FOUND = 58,   /* bool */
    /* CAN node commands (http_can_node_body), not config: */
    SETTING_CAN_FREQUENCY = 60,      /* u16 */
    SETTING_CAN_VOLTAGE = 61,        /* u16 */
    SETTING_CAN_FAN = 62,            /* [ch] mode | speed << 8 | target temp << 16 | overheat << 24 */
    SETTING_CAN_DISPLAY = 63,        /* flip | auto screen off << 1 */
} setting_t;

/* Receives the writes in the order the fields are handled; num holds
 * numbers and bools (0/1), text strings (NULL for numbers). */
typedef struct {
    void *ctx;
    void (*set)(void *ctx, uint16_t setting, int index, uint32_t num, const char *text);
} settings_sink_t;

/* PATCH /api/v2/can/nodes/{id}: the SETTING_CAN_* commands. */
bool http_can_node_body(const char *body, size_t len, const settings_sink_t *sink);
/* PATCH /api/v2/otp: 0 ok (*enabled set), 1 invalid JSON, 2 no bool "enabled". */
int http_otp_body(const char *body, size_t len, bool *enabled);

#ifdef __cplusplus
}
#endif

#endif /* HTTP_SETTINGS_H */
