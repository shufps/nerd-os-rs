//! A pool's connection settings (port of `main/stratum/stratum_config.cpp`):
//! host, port, user, password, extranonce subscribe, TLS and protocol of the
//! primary or the fallback pool, with their Kconfig defaults; `reload()`
//! takes changed settings over. The SV2 authority key and "require
//! authentication" are read on connecting; they are here so that a change
//! reconnects the pool.

use core::ffi::c_int;
use std::ffi::CString;

use crate::config;
use crate::config::sdkconfig as k;

/// `StratumProtocol` `STRATUM_V2`
pub const PROTOCOL_V2: u16 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PoolConfig {
    pub primary: bool,
    pub host: CString,
    pub port: c_int,
    pub user: CString,
    pub password: CString,
    pub enonce_sub: bool,
    pub tls: bool,
    pub protocol: u16,
    pub sv2_authority_pubkey: CString,
    pub sv2_require_auth: bool,
}

fn text(key: &[u8], def: &str) -> CString {
    let v = config::get_str(key)
        .map(|v| crate::util::cutil::until_nul(&v).to_vec())
        .unwrap_or_else(|| def.as_bytes().to_vec());
    CString::new(v).unwrap_or_default()
}

impl PoolConfig {
    /// The settings of `pool` (0 primary, else the fallback).
    pub fn load(pool: c_int) -> Self {
        if pool == 0 {
            PoolConfig {
                primary: true,
                host: text(b"stratumurl", k::STRATUM_URL),
                port: config::get_u16(b"stratumport", k::STRATUM_PORT as u16) as c_int,
                user: text(b"stratumuser", k::STRATUM_USER),
                password: text(b"stratumpass", k::STRATUM_PW),
                enonce_sub: config::get_u16(
                    b"stratumesub",
                    k::STRATUM_ENONCE_SUBSCRIBE_VALUE as u16,
                ) != 0,
                tls: config::get_u16(b"stratumtls", k::STRATUM_TLS_VALUE as u16) != 0,
                protocol: config::get_u16(b"sv2_proto", 0),
                sv2_authority_pubkey: text(b"sv2_auth_pk", ""),
                sv2_require_auth: config::get_u16(b"sv2_reqauth", 0) != 0,
            }
        } else {
            PoolConfig {
                primary: false,
                host: text(b"fbstratumurl", k::STRATUM_FALLBACK_URL),
                port: config::get_u16(b"fbstratumport", k::STRATUM_FALLBACK_PORT as u16) as c_int,
                user: text(b"fbstratumuser", k::STRATUM_FALLBACK_USER),
                password: text(b"fbstratumpass", k::STRATUM_FALLBACK_PW),
                enonce_sub: config::get_u16(
                    b"fbstratumesub",
                    k::STRATUM_FALLBACK_ENONCE_SUBSCRIBE_VALUE as u16,
                ) != 0,
                tls: config::get_u16(b"fbstratumtls", k::STRATUM_FALLBACK_TLS_VALUE as u16) != 0,
                protocol: config::get_u16(b"fbsv2_proto", 0),
                sv2_authority_pubkey: text(b"fbsv2_authpk", ""),
                sv2_require_auth: config::get_u16(b"fbsv2_reqauth", 0) != 0,
            }
        }
    }

    /// Takes the current settings over; false if nothing changed.
    pub fn reload(&mut self) -> bool {
        let new = PoolConfig::load(if self.primary { 0 } else { 1 });
        if new == *self {
            return false;
        }
        *self = new;
        true
    }

    pub fn is_sv2(&self) -> bool {
        self.protocol == PROTOCOL_V2
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_reload() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::config::clear();
        let mut p = PoolConfig::load(0);
        assert!(p.primary);
        assert_eq!(p.host.to_str().unwrap(), k::STRATUM_URL);
        assert_eq!(p.port, k::STRATUM_PORT as c_int);
        assert!(!p.reload());
        crate::config::set(
            b"stratumurl",
            crate::util::ajson::Json::Str(b"pool.example".to_vec()),
        );
        crate::config::set(
            b"sv2_proto",
            crate::util::ajson::Json::Num(crate::util::ajson::Num::U(1)),
        );
        assert!(p.reload());
        assert_eq!(p.host.to_str().unwrap(), "pool.example");
        assert!(p.is_sv2());
        let f = PoolConfig::load(1);
        assert!(!f.primary);
        assert_eq!(f.port, k::STRATUM_FALLBACK_PORT as c_int);
        crate::config::clear();
    }

    /// The SV2 authority key and "require authentication" are read when
    /// connecting: a change must reconnect the pool to take effect.
    #[test]
    fn sv2_authentication_changes_reload() {
        let _g = crate::config::tests::LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        crate::config::clear();
        let mut p = PoolConfig::load(0);
        let mut f = PoolConfig::load(1);
        crate::config::set(
            b"sv2_reqauth",
            crate::util::ajson::Json::Num(crate::util::ajson::Num::U(1)),
        );
        assert!(p.reload());
        assert!(!f.reload());
        crate::config::set(
            b"fbsv2_authpk",
            crate::util::ajson::Json::Str(
                b"9c4zpyJ2ndm4e8sP2uNc1VNCGxYjqaxWS6wUCjk8zFj6njFquH6".to_vec(),
            ),
        );
        assert!(f.reload());
        assert!(!p.reload());
        crate::config::clear();
    }
}
