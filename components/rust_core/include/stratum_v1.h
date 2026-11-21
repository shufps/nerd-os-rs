/*
 * Stratum v1 codec (implemented in Rust, components/rust_core/src/stratum_v1):
 * parsing of pool messages, request formatting, line framing and the
 * connection loop. The socket is the C++ transport (stratum_io_t).
 */

#ifndef STRATUM_V1_H
#define STRATUM_V1_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "mining_rs.h"

#ifdef __cplusplus
extern "C" {
#endif

#define MAX_MERKLE_BRANCHES 32
#define HASH_SIZE 32

typedef enum
{
    STRATUM_UNKNOWN,
    MINING_NOTIFY,
    MINING_SET_DIFFICULTY,
    MINING_SET_VERSION_MASK,
    MINING_SET_EXTRANONCE,
    STRATUM_RESULT,
    STRATUM_RESULT_SETUP,
    STRATUM_RESULT_VERSION_MASK,
    STRATUM_RESULT_SUBSCRIBE,
    CLIENT_RECONNECT
} stratum_method;

/* Strings are malloc()ed; free them with free(). */
typedef struct
{
    char *job_id;
    uint8_t _prev_block_hash[HASH_SIZE];
    char *coinbase_1;
    char *coinbase_2;
    uint8_t _merkle_branches[MAX_MERKLE_BRANCHES][HASH_SIZE];
    size_t n_merkle_branches;
    uint32_t version;
    uint32_t version_mask;
    uint32_t target;
    uint32_t ntime;
    uint32_t difficulty;
} mining_notify;

typedef struct
{
    char *extranonce_str;
    int extranonce_2_len;

    int64_t message_id;
    // Indicates the type of request the message represents.
    stratum_method method;

    // mining.notify
    int should_abandon_work;
    mining_notify *mining_notification;
    // mining.set_difficulty
    uint32_t new_difficulty;
    // mining.set_version_mask
    uint32_t version_mask;
    // result
    bool response_success;
} StratumApiV1Message;

/* The layout must match rust_core (src/stratum_v1/mod.rs). */
#if UINTPTR_MAX == 0xFFFFFFFFu
#define STRATUM_V1_SIZES_OK (sizeof(mining_notify) == 1092 && sizeof(StratumApiV1Message) == 40)
#else
#define STRATUM_V1_SIZES_OK (sizeof(mining_notify) == 1112 && sizeof(StratumApiV1Message) == 56)
#endif
#ifdef __cplusplus
static_assert(STRATUM_V1_SIZES_OK, "stratum_v1.h layout must match rust_core");
#else
_Static_assert(STRATUM_V1_SIZES_OK, "stratum_v1.h layout must match rust_core");
#endif

/*
 * Session of one connection: assigns message IDs to requests and remembers
 * which request each pending ID belongs to, so responses are classified by
 * what was sent (unknown IDs: 1..5 setup, above that share results).
 * Thread-safe: the connection task and the share-submitting task share it.
 *
 * The request functions write the request (with trailing '\n') plus NUL into
 * buf and return its length, or 0 if it does not fit into buf_len bytes.
 */
typedef struct stratum_v1_session stratum_v1_session_t;

stratum_v1_session_t *stratum_v1_session_new(void);
void stratum_v1_session_free(stratum_v1_session_t *session);

size_t stratum_v1_session_subscribe(stratum_v1_session_t *session, char *buf, size_t buf_len, const char *device,
                                    const char *asic, const char *version);
size_t stratum_v1_session_configure(stratum_v1_session_t *session, char *buf, size_t buf_len);
size_t stratum_v1_session_authorize(stratum_v1_session_t *session, char *buf, size_t buf_len, const char *user,
                                    const char *password);
size_t stratum_v1_session_suggest_difficulty(stratum_v1_session_t *session, char *buf, size_t buf_len,
                                             uint32_t difficulty);
size_t stratum_v1_session_extranonce_subscribe(stratum_v1_session_t *session, char *buf, size_t buf_len);

/*
 * Parses one JSON-RPC line into *message, which the caller zeroes before.
 * Returns false for invalid or unusable messages; message->method may still
 * be set then (e.g. STRATUM_RESULT_SUBSCRIBE for a bad subscribe response).
 * In both cases the caller frees message->extranonce_str,
 * message->mining_notification and its strings.
 */
bool stratum_v1_session_parse(stratum_v1_session_t *session, const char *line, StratumApiV1Message *message);

/* Frees the strings and the mining_notify of a parsed message and clears the
 * pointers; NULL is ignored. */
void stratum_v1_message_free(StratumApiV1Message *message);

/*
 * Line framing for the byte stream from the pool: receive into
 * stratum_v1_lines_space(), report the bytes with stratum_v1_lines_commit(),
 * then take complete lines (without "\r\n") with stratum_v1_lines_take().
 */
typedef struct stratum_v1_lines stratum_v1_lines_t;

stratum_v1_lines_t *stratum_v1_lines_new(void);
void stratum_v1_lines_free(stratum_v1_lines_t *lines);

/*
 * Job construction: extranonce2 (the low extranonce2_len bytes of the
 * counter, as hex), coinbase transaction, merkle root and the hash fields of
 * a bm_job in the byte orders of construct_bm_job().
 */
typedef struct
{
    mining_job_hashes_t hashes;
    char extranonce2[2 * 16 + 1]; // hex, NUL-terminated; sent with mining.submit
} stratum_v1_job_t;

/* false for missing strings or an extranonce2_len outside 1..=16. */
bool stratum_v1_build_job(const mining_notify *notify, const char *extranonce1, int extranonce2_len,
                          uint32_t extranonce2, stratum_v1_job_t *job);

/*
 * The socket of the connection functions below (the C++ transport of the
 * former connection loop; used by the reference tests).
 */
typedef struct {
    void *ctx;
    int (*send)(void *ctx, const uint8_t *data, size_t len); // bytes sent, 0: no progress, <0: -errno
    int (*recv)(void *ctx, uint8_t *buf, size_t len);        // bytes received, 0: peer closed, <0: -errno
    bool (*is_connected)(void *ctx);
    void (*delay_ms)(void *ctx, uint32_t ms);
} stratum_io_t;

/* Sends all of data (logged as "tx"); false on an error or when the socket
 * makes no progress for 3 attempts. */
bool stratum_v1_send_all(const stratum_io_t *io, const uint8_t *data, size_t len);

/* Formats a mining.submit in the session and sends it; version: the rolled
 * version bits (delta). false if it does not fit (1024 bytes) or the send
 * failed. Safe to call while another task receives. */
bool stratum_v1_submit(const stratum_io_t *io, stratum_v1_session_t *session, const char *user, const char *job_id,
                       const char *extranonce2, uint32_t ntime, uint32_t nonce, uint32_t version);

/* Blocks until a complete line arrived (returned as a malloc()ed string the
 * caller frees) or the connection failed (NULL). */
char *stratum_v1_receive_line(const stratum_io_t *io, stratum_v1_lines_t *lines);

#ifdef __cplusplus
}
#endif

#endif /* STRATUM_V1_H */
