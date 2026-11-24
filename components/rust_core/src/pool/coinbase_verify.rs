//! Coinbase state of the pool manager (port of `processCoinbase()`,
//! `storeExtranonce()`, `runVerification()` and the verification members of
//! `StratumManager`): per pool the extranonce from the subscribe, the last
//! decoded coinbase, the verification counters and the block reason. The
//! decoding is [`crate::pool::coinbase`], the decision [`crate::pool::verify`];
//! the alert, the reconnect and the board fault flag are done by the caller
//! from the returned [`Outcome`].
//!
//! One mutex for all of it: the block reason is cleared by the settings
//! handler and read by the job task.

use std::sync::Mutex;

use crate::pool::coinbase::{self, CoinbaseResult};
use crate::pool::{self, block, VerifyInput};

/// The verification settings of a pool (from the config).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VerifyConfig {
    pub mode: u16,
    pub max_fee: u16,
    pub force: bool,
}

/// What the caller does after a verification.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// the verification ran (a coinbase was decoded or re-checked)
    pub verified: bool,
    /// send the "coinbase verification failed" alert (with `result()`)
    pub alert: bool,
    /// the pool just got blocked: reconnect it
    pub reconnect: bool,
    /// re-evaluate whether any configured pool is usable (fault flag)
    pub check_fault: bool,
    /// the coinbase could not be decoded
    pub parse_failed: bool,
}

#[derive(Clone)]
struct Pool {
    extranonce1: Option<Vec<u8>>,
    extranonce2_len: i32,
    result: CoinbaseResult,
    ok: bool,
    checks: u32,
    fails: u32,
    blocked: u8,
}

impl Default for Pool {
    fn default() -> Self {
        Pool {
            extranonce1: None,
            extranonce2_len: 0,
            result: CoinbaseResult::zeroed(),
            ok: false,
            checks: 0,
            fails: 0,
            blocked: block::NONE,
        }
    }
}

/// The address part of a pool user `address.worker`.
pub fn user_address(user: &[u8]) -> &[u8] {
    // the former 128-byte buffer: at most 127 characters
    let user = &user[..user.len().min(127)];
    &user[..user.iter().position(|&c| c == b'.').unwrap_or(user.len())]
}

#[derive(Default)]
pub struct PoolCoinbase {
    pools: [Pool; 2],
}

impl PoolCoinbase {
    fn pool(&mut self, pool: i32) -> Option<&mut Pool> {
        self.pools.get_mut(usize::try_from(pool).ok()?)
    }

    /// From the subscribe result or `mining.set_extranonce`; `None` clears it.
    pub fn set_extranonce(&mut self, pool: i32, extranonce1: Option<&[u8]>, extranonce2_len: i32) {
        let Some(p) = self.pool(pool) else {
            return;
        };
        p.extranonce2_len = extranonce2_len;
        p.extranonce1 = extranonce1.map(<[u8]>::to_vec);
    }

    /// The stored extranonce1 and extranonce2 length.
    pub fn extranonce(&self, pool: i32) -> (Option<Vec<u8>>, i32) {
        match usize::try_from(pool).ok().and_then(|i| self.pools.get(i)) {
            Some(p) => (p.extranonce1.clone(), p.extranonce2_len),
            None => (None, 0),
        }
    }

    /// Decodes a coinbase and verifies it. `extranonce1`: `None` uses the
    /// stored one (v1); without one, nothing happens.
    #[allow(clippy::too_many_arguments)]
    pub fn process(
        &mut self,
        pool: i32,
        coinbase_1: &[u8],
        coinbase_2: &[u8],
        version: u32,
        nbits: u32,
        extranonce1: Option<(&[u8], i32)>,
        user: Option<&[u8]>,
        config: VerifyConfig,
    ) -> Outcome {
        let Some(p) = self.pool(pool) else {
            return Outcome::default();
        };
        let stored = p.extranonce1.clone();
        let (en1, en2_len) = match extranonce1 {
            Some(e) => e,
            None => match &stored {
                Some(e) => (e.as_slice(), p.extranonce2_len),
                None => return Outcome::default(),
            },
        };
        let mut result = CoinbaseResult::zeroed();
        let address = user.map(user_address);
        if coinbase::process(
            coinbase_1,
            coinbase_2,
            version,
            nbits,
            en1,
            en2_len,
            address,
            &mut result,
        )
        .is_err()
        {
            return Outcome {
                parse_failed: true,
                ..Default::default()
            };
        }
        p.result = result;
        self.verify(pool, config)
    }

    /// Verifies the stored coinbase again (settings changed).
    pub fn verify(&mut self, pool: i32, config: VerifyConfig) -> Outcome {
        let Some(p) = self.pool(pool) else {
            return Outcome::default();
        };
        let r = pool::verify(&VerifyInput {
            mode: config.mode,
            max_fee: config.max_fee,
            force: config.force,
            user_value_satoshis: p.result.user_value_satoshis,
            total_value_satoshis: p.result.total_value_satoshis,
            block_height: p.result.block_height,
            was_ok: p.ok,
            blocked: p.blocked,
        });
        p.ok = r.ok;
        let mut out = Outcome {
            verified: true,
            alert: r.alert,
            ..Default::default()
        };
        if !r.counted {
            return out;
        }
        p.checks += 1;
        if !r.ok {
            p.fails += 1;
        }
        p.blocked = r.blocked;
        out.reconnect = r.reconnect;
        out.check_fault = r.check_fault;
        out
    }

    pub fn reset_counts(&mut self, pool: i32) {
        if let Some(p) = self.pool(pool) {
            p.checks = 0;
            p.fails = 0;
        }
    }

    pub fn clear_blocked(&mut self, pool: i32) {
        if let Some(p) = self.pool(pool) {
            p.blocked = block::NONE;
        }
    }

    pub fn blocked(&self, pool: i32) -> u8 {
        usize::try_from(pool)
            .ok()
            .and_then(|i| self.pools.get(i))
            .map_or(block::NONE, |p| p.blocked)
    }
}

/// The coinbase verification state shared by the stratum tasks, the
/// manager and the API (one mutex around [`PoolCoinbase`]).
pub struct Handle(Mutex<PoolCoinbase>);

impl Default for Handle {
    fn default() -> Self {
        Handle(Mutex::new(PoolCoinbase::default()))
    }
}

impl Handle {
    pub(crate) fn with<T>(&self, f: impl FnOnce(&mut PoolCoinbase) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// v1: with the stored extranonce (nothing without one).
    #[allow(clippy::too_many_arguments)]
    pub fn process_v1(
        &self,
        pool: i32,
        coinbase_1: &[u8],
        coinbase_2: &[u8],
        version: u32,
        nbits: u32,
        user: Option<&[u8]>,
        config: VerifyConfig,
    ) -> Outcome {
        self.with(|s| {
            s.process(
                pool, coinbase_1, coinbase_2, version, nbits, None, user, config,
            )
        })
    }

    /// v2: with the channel's extranonce.
    #[allow(clippy::too_many_arguments)]
    pub fn process_v2(
        &self,
        pool: i32,
        coinbase_1: &[u8],
        coinbase_2: &[u8],
        version: u32,
        nbits: u32,
        extranonce: (&[u8], i32),
        user: Option<&[u8]>,
        config: VerifyConfig,
    ) -> Outcome {
        self.with(|s| {
            s.process(
                pool,
                coinbase_1,
                coinbase_2,
                version,
                nbits,
                Some(extranonce),
                user,
                config,
            )
        })
    }

    pub fn verify(&self, pool: i32, config: VerifyConfig) -> Outcome {
        self.with(|s| s.verify(pool, config))
    }

    pub fn reset_counts(&self, pool: i32) {
        self.with(|s| s.reset_counts(pool))
    }

    pub fn clear_blocked(&self, pool: i32) {
        self.with(|s| s.clear_blocked(pool))
    }

    /// The block reason (`pool::block::*`).
    pub fn blocked(&self, pool: i32) -> u8 {
        self.with(|s| s.blocked(pool))
    }

    pub fn info(&self, pool: i32) -> Info {
        self.with(|s| {
            let p = &s.pools[(pool & 1) as usize];
            Info {
                result: p.result,
                ok: p.ok,
                blocked: p.blocked,
                check_count: p.checks,
                fail_count: p.fails,
            }
        })
    }
}

/// The verification state of one pool.
#[derive(Clone, Copy)]
pub struct Info {
    pub result: CoinbaseResult,
    pub ok: bool,
    pub blocked: u8,
    pub check_count: u32,
    pub fail_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_address_is_cut_at_the_worker() {
        assert_eq!(user_address(b"bc1qabc.worker1"), b"bc1qabc");
        assert_eq!(user_address(b"bc1qabc"), b"bc1qabc");
        assert_eq!(user_address(&[b'a'; 200]).len(), 127);
    }

    fn cfg(mode: u16, force: bool) -> VerifyConfig {
        VerifyConfig {
            mode,
            max_fee: 30,
            force,
        }
    }

    #[test]
    fn counts_blocks_and_clears() {
        let mut s = PoolCoinbase::default();
        // without decoded data (height 0) a failure does not block
        let o = s.verify(0, cfg(1, true));
        assert!(o.verified && !o.reconnect && s.blocked(0) == block::NONE);
        // a decoded coinbase that does not pay the user
        s.pools[0].result.block_height = 870_000;
        let o = s.verify(0, cfg(1, true));
        assert!(o.verified && o.reconnect && o.check_fault);
        assert_eq!(s.blocked(0), block::ADDRESS_NOT_FOUND);
        s.verify(0, cfg(1, true));
        assert_eq!((s.pools[0].checks, s.pools[0].fails), (3, 3));
        s.reset_counts(0);
        s.clear_blocked(0);
        assert_eq!((s.pools[0].checks, s.blocked(0)), (0, block::NONE));
        // off: not counted
        assert!(!s.verify(1, cfg(0, true)).check_fault);
        assert_eq!(s.pools[1].checks, 0);
        // a pool that does not exist
        assert_eq!(s.verify(3, cfg(1, true)), Outcome::default());
    }

    #[test]
    fn v1_needs_the_extranonce() {
        let mut s = PoolCoinbase::default();
        let o = s.process(
            0,
            b"00",
            b"00",
            0x2000_0000,
            0x1703_a30c,
            None,
            None,
            cfg(1, false),
        );
        assert_eq!(o, Outcome::default());
        s.set_extranonce(0, Some(b"08000002"), 4);
        let o = s.process(
            0,
            b"zz",
            b"00",
            0x2000_0000,
            0x1703_a30c,
            None,
            None,
            cfg(1, false),
        );
        assert!(o.parse_failed);
    }
}
