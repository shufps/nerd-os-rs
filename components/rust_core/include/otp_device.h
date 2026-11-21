/*
 * OTP of the device (implemented in Rust, components/rust_core/src/
 * otp_device.rs and otp.rs): identity, enrollment QR code, the request
 * and login checks with the wall clock.
 */

#ifndef OTP_DEVICE_H
#define OTP_DEVICE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#include "otp_state.h"

#ifdef __cplusplus
extern "C" {
#endif

/* the device identity for the otpauth URI; loads the saved OTP state */
void otp_dev_init(const char *hostname, const char *mac, const char *model);
/* a new secret and its QR code: ESP_OK, ESP_ERR_NOT_FINISHED (active), ESP_FAIL */
int otp_dev_start_enrollment(void);
/* 0 ok, 1 blocked, 2 failed (headers X-OTP-Session / X-TOTP, NULL if absent) */
int otp_dev_request_check(const char *session, const char *totp, bool force);
int otp_dev_login_check(const char *totp);
/* a session token valid ttl seconds into out; its length, 0 on an error */
size_t otp_dev_mint_session(uint32_t ttl, char *out, size_t cap);

#ifdef __cplusplus
}
#endif

#endif /* OTP_DEVICE_H */
