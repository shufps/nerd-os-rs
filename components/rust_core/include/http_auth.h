/*
 * Authentication gate of the HTTP API (implemented in Rust,
 * components/rust_core/src/http_auth.rs): TOTP codes, session tokens, the
 * failed-attempt rate limit and the private-network check. Secrets, replay
 * state and the session key stay in the OTP class / NVS.
 */

#ifndef HTTP_AUTH_H
#define HTTP_AUTH_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Failed attempts: 5 within 60 s block for 5 minutes (one limit for all
 * endpoints). auth_rate_limit_failure() returns false if it starts a block. */
bool auth_rate_limit_blocked(uint64_t now_ms);
bool auth_rate_limit_failure(uint64_t now_ms);

#ifdef __cplusplus
}
#endif

#endif /* HTTP_AUTH_H */
