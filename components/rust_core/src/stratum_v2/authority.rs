//! The pool's authority public key from the config (port of
//! `StratumTaskV2::loadAuthorityPubkey()` with libbase58's `b58tobin()`):
//! base58check of a 2-byte version (1, little endian) and the 32-byte
//! x-only key.
//!
//! The checksum is verified, so a mistyped key with the right length and
//! version is rejected here instead of failing the handshake later with a
//! signature error.

use core::ffi::c_int;

use crate::mining::sha256d;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// not base58
    Encoding,
    /// decoded length (38 expected)
    Length(usize),
    Checksum,
    /// the version (1 expected)
    Version(u16),
}

impl KeyError {
    /// `SV2_KEY_*`
    pub fn code(self) -> c_int {
        match self {
            KeyError::Encoding => 1,
            KeyError::Length(_) => 2,
            KeyError::Checksum => 3,
            KeyError::Version(_) => 4,
        }
    }
}

/// The 32-byte key of a base58check authority key.
pub fn authority_key(b58: &str) -> Result<[u8; 32], KeyError> {
    let raw = bs58::decode(b58)
        .into_vec()
        .map_err(|_| KeyError::Encoding)?;
    if raw.len() != 38 {
        return Err(KeyError::Length(raw.len()));
    }
    let (payload, check) = raw.split_at(34);
    if sha256d(payload)[..4] != *check {
        return Err(KeyError::Checksum);
    }
    let version = u16::from_le_bytes([payload[0], payload[1]]);
    if version != 1 {
        return Err(KeyError::Version(version));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&payload[2..]);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    // a pool's published key (checksum and version valid)
    const KEY: &str = "9bCoFxTszKCuffyywH5uS5o6WcU4vsjTH2axxc7wE86y2HhvULU";

    fn encode(version: u16, key: &[u8; 32], good_checksum: bool) -> String {
        let mut p = version.to_le_bytes().to_vec();
        p.extend_from_slice(key);
        let mut c = sha256d(&p)[..4].to_vec();
        if !good_checksum {
            c[0] ^= 1;
        }
        p.extend_from_slice(&c);
        bs58::encode(p).into_string()
    }

    #[test]
    fn valid() {
        let k = authority_key(KEY).unwrap();
        assert_eq!(encode(1, &k, true), KEY);
        let k = [7u8; 32];
        assert_eq!(authority_key(&encode(1, &k, true)), Ok(k));
    }

    #[test]
    fn invalid() {
        let k = [9u8; 32];
        assert_eq!(
            authority_key(&encode(1, &k, false)),
            Err(KeyError::Checksum)
        );
        assert_eq!(
            authority_key(&encode(2, &k, true)),
            Err(KeyError::Version(2))
        );
        assert_eq!(authority_key("0OIl"), Err(KeyError::Encoding));
        assert_eq!(authority_key(""), Err(KeyError::Length(0)));
        assert_eq!(authority_key(&KEY[1..]), Err(KeyError::Length(37)));
        // one changed character
        let mut typo = KEY.to_string();
        typo.replace_range(10..11, "A");
        assert_eq!(authority_key(&typo), Err(KeyError::Checksum));
    }
}
