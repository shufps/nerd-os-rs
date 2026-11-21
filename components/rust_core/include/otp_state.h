/*
 * The OTP state (implemented in Rust, components/rust_core/src/otp.rs):
 * TOTP secret, session key and boot ID, enabled and enrollment state,
 * persisted in the configuration; TOTP check with replay protection,
 * session tokens, the otpauth URI of an enrollment. Thread-safe.
 */

#ifndef OTP_STATE_H
#define OTP_STATE_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

void otp_save(void);
bool otp_is_enabled(void);
void otp_set_enabled(bool enabled);
bool otp_enrollment_active(void);
void otp_disable_enrollment(void);

#ifdef __cplusplus
}
#endif

#endif /* OTP_STATE_H */
