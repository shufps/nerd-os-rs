/*
 * The configuration document (implemented in Rust,
 * components/rust_core/src/config.rs): a flat JSON object of setting name
 * to value, with ArduinoJson's conversions and serialization. NVS access and
 * the migration of the old per-setting keys are done by nvs_config.cpp.
 * Thread-safe.
 */

#ifndef CONFIG_STORE_H
#define CONFIG_STORE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Replaces the document with the stored JSON; false if it does not parse
 * (the document is empty then). */
bool config_load(const uint8_t *json, size_t len);
void config_mark_dirty(void);

/* A malloc()ed copy of the string value, else of def; NULL if neither. */
char *config_get_str(const char *key, const char *def);
/* value NULL stores null */
void config_set_str(const char *key, const char *value);
uint16_t config_get_u16(const char *key, uint16_t def);
void config_set_u16(const char *key, uint16_t value);
bool config_get_bool(const char *key, bool def);
void config_set_bool(const char *key, bool value);
uint64_t config_get_u64(const char *key, uint64_t def);
void config_set_u64(const char *key, uint64_t value);
/* the key exists and is not null */
bool config_has(const char *key);

/* If the document changed: write gets the JSON (the document stays locked
 * meanwhile); a successful write clears the changed flag. False if the
 * write failed. */
bool config_flush(bool (*write)(void *ctx, const uint8_t *json, size_t len), void *ctx);

/* NVS storage (src/config_nvs.rs): load the document or migrate the old
   keys, start the flush task; persist the changes (batched by the task) */
void config_nvs_init(bool single_fan, uint16_t overheat_default);
void config_request_flush(void);

#ifdef __cplusplus
}
#endif

#endif /* CONFIG_STORE_H */
