//! Pool selection and coinbase verification decisions of the stratum manager
//! (port of the logic in `main/stratum/stratum_manager*.cpp`). The C++
//! managers keep their mutex, tasks, config and alerts; they pass the current
//! state in and carry out the returned [`Actions`].
//!
//! Verified against verbatim copies of the C++ logic by
//! `tests/pool_policy.rs`. In dual-pool mode a disconnected
//! pool's difficulty is forgotten, so it does not lower the ASIC difficulty.

pub const PRIMARY: usize = 0;
pub const SECONDARY: usize = 1;

/// What the manager knows about both pools when an event happens.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PoolState {
    /// The pool's task exists.
    pub has_task: [bool; 2],
    /// The manager's `isConnected()`: the task exists and is connected.
    pub connected: [bool; 2],
    pub verify_blocked: [bool; 2],
}

/// What the manager has to do, per pool. The C++ side applies them in this
/// order: invalidate, disconnect, stop_timer, start_timer, connect.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Actions {
    /// `create_job_invalidate()` and clear the valid-notify flag
    pub invalidate: [bool; 2],
    pub disconnect: [bool; 2],
    pub stop_timer: [bool; 2],
    pub start_timer: [bool; 2],
    pub connect: [bool; 2],
    /// reset the pool's verification counters
    pub reset_verify_counts: [bool; 2],
}

/// Failover: the primary pool is used whenever it is connected and not
/// blocked by the coinbase verification; the secondary only otherwise.
#[derive(Debug, Default)]
pub struct Failover {
    pub selected: usize,
}

impl Failover {
    fn primary_unusable(s: &PoolState) -> bool {
        !s.connected[PRIMARY] || s.verify_blocked[PRIMARY]
    }

    pub fn reconnect_timer(&mut self, pool: usize, s: &PoolState) -> Actions {
        let mut a = Actions::default();
        if s.verify_blocked[pool] {
            return a;
        }
        if pool == PRIMARY || Self::primary_unusable(s) {
            a.connect[pool] = true;
        } else {
            // the primary is good: no secondary online
            a.disconnect[pool] = true;
        }
        a
    }

    pub fn connected(&mut self, pool: usize, s: &PoolState) -> Actions {
        let mut a = Actions::default();
        if s.verify_blocked[pool] {
            // e.g. after a manual reconnect: kick it off right away
            a.disconnect[pool] = true;
            return a;
        }
        if pool == PRIMARY {
            self.selected = PRIMARY;
            a.reset_verify_counts[PRIMARY] = true;
            if s.has_task[SECONDARY] {
                a.disconnect[SECONDARY] = true;
                a.stop_timer[SECONDARY] = true;
            }
        } else if Self::primary_unusable(s) {
            self.selected = SECONDARY;
            a.reset_verify_counts[SECONDARY] = true;
        } else {
            a.disconnect[SECONDARY] = true;
            a.stop_timer[SECONDARY] = true;
        }
        a
    }

    pub fn disconnected(&mut self, pool: usize, s: &PoolState) -> Actions {
        let mut a = Actions::default();
        a.invalidate[pool] = true;
        // the primary went down: let the secondary try (it becomes selected
        // once it actually connects)
        if pool == PRIMARY && s.has_task[SECONDARY] {
            a.start_timer[SECONDARY] = true;
        }
        a
    }

    pub fn accepts_notify_from(&self, pool: usize) -> bool {
        pool == self.selected
    }
}

/// Dual pool: both pools stay connected; jobs alternate by the configured
/// balance (percent for the primary).
#[derive(Debug)]
pub struct DualPool {
    pub balance: u16,
    error_accum: i32,
    pool_diffs: [u32; 2],
    pub pool_diff_err: [bool; 2],
}

impl Default for DualPool {
    fn default() -> Self {
        Self {
            balance: 50,
            error_accum: 0,
            pool_diffs: [u32::MAX; 2],
            pool_diff_err: [false; 2],
        }
    }
}

impl DualPool {
    /// true if the balance changed (then the pools reconnect).
    pub fn set_balance(&mut self, balance: u16) -> bool {
        if balance == self.balance {
            return false;
        }
        self.balance = balance;
        self.error_accum = 0;
        true
    }

    pub fn reconnect_timer(&mut self, pool: usize, s: &PoolState) -> Actions {
        let mut a = Actions::default();
        a.connect[pool] = !s.verify_blocked[pool];
        a
    }

    pub fn connected(&mut self, pool: usize, s: &PoolState) -> Actions {
        let mut a = Actions::default();
        if s.verify_blocked[pool] {
            a.disconnect[pool] = true;
        } else {
            self.pool_diff_err[pool] = false;
        }
        a
    }

    pub fn disconnected(&mut self, pool: usize) -> Actions {
        // Forget the pool's difficulty, it no longer applies.
        self.pool_diffs[pool] = u32::MAX;
        let mut a = Actions::default();
        a.invalidate[pool] = true;
        a.start_timer[pool] = true;
        a
    }

    /// The pool for the next job. `valid`: the pool has valid work and is not
    /// verify-blocked. With both valid, an error accumulator (dithering)
    /// spreads the jobs by the balance.
    pub fn next_active_pool(&mut self, valid: [bool; 2]) -> usize {
        match valid {
            [true, false] => {
                self.error_accum = 0;
                PRIMARY
            }
            [false, true] => {
                self.error_accum = 0;
                SECONDARY
            }
            [false, false] => {
                self.error_accum = 0;
                PRIMARY
            }
            [true, true] => {
                self.error_accum += 100 - self.balance as i32;
                if self.error_accum >= 100 {
                    self.error_accum -= 100;
                    SECONDARY
                } else {
                    PRIMARY
                }
            }
        }
    }

    /// ASIC difficulty: the lower of both pools' difficulties, clamped to
    /// the ASIC range. Flags a pool whose difficulty is below the ASIC minimum.
    pub fn select_asic_diff(
        &mut self,
        pool: usize,
        pool_diff: u32,
        asic_min: u32,
        asic_max: u32,
    ) -> u32 {
        if pool >= 2 {
            return asic_max;
        }
        self.pool_diff_err[pool] = pool_diff < asic_min;
        self.pool_diffs[pool] = pool_diff;
        let min = self.pool_diffs[0].min(self.pool_diffs[1]);
        if min < asic_min {
            asic_min
        } else if min > asic_max {
            asic_max
        } else {
            min
        }
    }

    /// Share of the hashrate (percent) a pool currently gets.
    pub fn active_balance(&self, pool: usize, s: &PoolState) -> i32 {
        if !s.has_task[0] || !s.has_task[1] || (!s.connected[0] && !s.connected[1]) {
            return 0;
        }
        if s.connected[0] && s.connected[1] {
            let balance = self.balance as i32;
            return if pool == 0 { balance } else { 100 - balance };
        }
        if s.connected[pool] {
            100
        } else {
            0
        }
    }
}

/// Failover: the ASIC difficulty is the pool difficulty clamped to the ASIC range.
pub fn clamp_asic_diff(pool_diff: u32, asic_min: u32, asic_max: u32) -> u32 {
    pool_diff.min(asic_max).max(asic_min)
}

/// Why a pool is blocked by the coinbase verification.
pub mod block {
    pub const NONE: u8 = 0;
    pub const ADDRESS_NOT_FOUND: u8 = 1;
    pub const FEE_EXCEEDED: u8 = 2;
}

/// Inputs of one coinbase verification.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct VerifyInput {
    /// 0 off, 1 address must be paid, 2 also the pool fee must stay below max
    pub mode: u16,
    /// maximum pool fee in tenths of a percent
    pub max_fee: u16,
    /// block (and reconnect) the pool when the verification fails
    pub force: bool,
    pub user_value_satoshis: u64,
    pub total_value_satoshis: u64,
    pub block_height: u32,
    pub was_ok: bool,
    /// one of [`block`]
    pub blocked: u8,
}

/// Result of one coinbase verification.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VerifyResult {
    pub ok: bool,
    /// count the check (and a failure if !ok); false when verification is off
    pub counted: bool,
    /// the verification just turned from ok to failed: send the alert
    pub alert: bool,
    /// new value of the pool's block reason (one of [`block`])
    pub blocked: u8,
    /// the pool just got blocked: reconnect it
    pub reconnect: bool,
    /// the force logic ran: re-evaluate whether any configured pool is usable
    pub check_fault: bool,
}

pub fn verify(input: &VerifyInput) -> VerifyResult {
    let mut r = VerifyResult {
        blocked: input.blocked,
        ..Default::default()
    };
    if input.mode == 0 {
        return r;
    }
    // mode 1: the user's address must be paid
    let mut ok = input.user_value_satoshis > 0;
    // mode 2: and the pool's share must not exceed the maximum fee
    if ok && input.mode >= 2 && input.total_value_satoshis > 0 {
        let pool_take = input
            .total_value_satoshis
            .wrapping_sub(input.user_value_satoshis);
        let fee_x100 = (pool_take.wrapping_mul(10_000) / input.total_value_satoshis) as u32;
        ok = fee_x100 <= input.max_fee as u32 * 10;
    }
    r.ok = ok;
    r.counted = true;
    r.alert = !ok && input.was_ok;

    // Force mode, only with actual coinbase data (not right after a settings change).
    if input.force && input.block_height > 0 {
        if !ok && input.blocked == block::NONE {
            r.blocked = if input.user_value_satoshis == 0 {
                block::ADDRESS_NOT_FOUND
            } else {
                block::FEE_EXCEEDED
            };
            r.reconnect = true;
        } else if ok && input.blocked != block::NONE {
            r.blocked = block::NONE;
        }
        r.check_fault = true;
    }
    r
}

/// Whether any configured pool is not blocked (else the board shows a fault).
pub fn any_usable(configured: [bool; 2], blocked: [bool; 2]) -> bool {
    (0..2).any(|i| configured[i] && !blocked[i])
}

enum Policy {
    Failover(Failover),
    Dual(DualPool),
}

/// A connection event the policy decides on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    ReconnectTimer,
    Connected,
    Disconnected,
}

/// The pool policy of the manager. Locked internally: the job task asks
/// for the ASIC difficulty without the manager's mutex (taking it there
/// could deadlock against the job mutex), the callbacks run with it.
/// Nothing else is locked while this lock is held.
pub struct Handle(std::sync::Mutex<Policy>);

fn idx(pool: i32) -> usize {
    (pool & 1) as usize
}

impl Handle {
    pub fn new(dual: bool) -> Self {
        Handle(std::sync::Mutex::new(if dual {
            Policy::Dual(DualPool::default())
        } else {
            Policy::Failover(Failover::default())
        }))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Policy) -> T) -> T {
        f(&mut self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The actions for a connection event of `pool`.
    pub fn event(&self, event: Event, pool: i32, s: &PoolState) -> Actions {
        let i = idx(pool);
        self.with(|policy| match (policy, event) {
            (Policy::Failover(f), Event::ReconnectTimer) => f.reconnect_timer(i, s),
            (Policy::Dual(d), Event::ReconnectTimer) => d.reconnect_timer(i, s),
            (Policy::Failover(f), Event::Connected) => f.connected(i, s),
            (Policy::Dual(d), Event::Connected) => d.connected(i, s),
            (Policy::Failover(f), Event::Disconnected) => f.disconnected(i, s),
            (Policy::Dual(d), Event::Disconnected) => d.disconnected(i),
        })
    }

    /// Failover: the selected pool (`valid` ignored); dual: the pool for the
    /// next job (`valid`: the pool has valid work and is not verify-blocked).
    pub fn next_pool(&self, valid: [bool; 2]) -> i32 {
        self.with(|policy| match policy {
            Policy::Failover(f) => f.selected as i32,
            Policy::Dual(d) => d.next_active_pool(valid) as i32,
        })
    }

    pub fn accepts_notify_from(&self, pool: i32) -> bool {
        self.with(|policy| match policy {
            Policy::Failover(f) => f.accepts_notify_from(idx(pool)),
            Policy::Dual(_) => true,
        })
    }

    pub fn select_asic_diff(&self, pool: i32, pool_diff: u32, asic_min: u32, asic_max: u32) -> u32 {
        self.with(|policy| match policy {
            Policy::Failover(_) => clamp_asic_diff(pool_diff, asic_min, asic_max),
            Policy::Dual(_) if !(0..2).contains(&pool) => asic_max,
            Policy::Dual(d) => d.select_asic_diff(pool as usize, pool_diff, asic_min, asic_max),
        })
    }

    /// Switches between failover and dual pool mode (a new policy state in
    /// the new mode); true if the mode changed.
    pub fn set_mode(&self, dual: bool, balance: u16) -> bool {
        self.with(|policy| {
            if matches!(policy, Policy::Dual(_)) == dual {
                return false;
            }
            *policy = if dual {
                let mut d = DualPool::default();
                d.set_balance(balance);
                Policy::Dual(d)
            } else {
                Policy::Failover(Failover::default())
            };
            true
        })
    }

    /// Dual pool: true if the balance changed. Failover: false.
    pub fn set_balance(&self, balance: u16) -> bool {
        self.with(|policy| match policy {
            Policy::Failover(_) => false,
            Policy::Dual(d) => d.set_balance(balance),
        })
    }

    pub fn pool_diff_err(&self, pool: i32) -> bool {
        self.with(|policy| match policy {
            Policy::Failover(_) => false,
            Policy::Dual(d) => (0..2).contains(&pool) && d.pool_diff_err[pool as usize],
        })
    }

    pub fn active_balance(&self, pool: i32, s: &PoolState) -> i32 {
        self.with(|policy| match policy {
            Policy::Failover(_) => 0,
            Policy::Dual(d) => d.active_balance(idx(pool), s),
        })
    }

    /// The pool a per-pool view shows for `pool`: in failover mode always
    /// the selected one.
    pub fn display_pool(&self, pool: i32) -> i32 {
        self.with(|policy| match policy {
            Policy::Failover(f) => f.selected as i32,
            Policy::Dual(_) => idx(pool) as i32,
        })
    }

    /// The pool whose ping the legacy single-pool getters report: the
    /// selected pool, or in dual pool mode the one with the larger share.
    pub fn compat_ping_pool(&self) -> i32 {
        self.with(|policy| match policy {
            Policy::Failover(f) => f.selected as i32,
            Policy::Dual(d) => i32::from(d.balance < 50),
        })
    }
}

// Must match the static assertions in pool_policy.h.
const _: () = assert!(core::mem::size_of::<PoolState>() == 6);
const _: () = assert!(core::mem::size_of::<Actions>() == 12);
const _: () = assert!(core::mem::size_of::<VerifyInput>() == 32);
const _: () = assert!(core::mem::size_of::<VerifyResult>() == 6);

#[cfg(test)]
mod tests {
    use super::*;

    fn state(connected: [bool; 2], blocked: [bool; 2]) -> PoolState {
        PoolState {
            has_task: [true; 2],
            connected,
            verify_blocked: blocked,
        }
    }

    #[test]
    fn display_and_compat_ping_pool() {
        let f = Handle::new(false);
        // failover: always the selected pool, the primary at first
        assert_eq!(f.display_pool(1), 0);
        assert_eq!(f.compat_ping_pool(), 0);
        let s = state([false, true], [false; 2]);
        f.event(Event::Connected, 1, &s);
        assert_eq!(f.display_pool(0), 1);
        assert_eq!(f.compat_ping_pool(), 1);

        let d = Handle::new(true);
        assert_eq!(d.display_pool(1), 1);
        assert_eq!(d.compat_ping_pool(), 0);
        d.set_balance(49);
        assert_eq!(d.compat_ping_pool(), 1);
    }

    #[test]
    fn failover_scenario() {
        let mut f = Failover::default();
        // primary comes up: secondary goes away
        let a = f.connected(PRIMARY, &state([true, false], [false; 2]));
        assert_eq!(
            (a.disconnect, a.stop_timer, f.selected),
            ([false, true], [false, true], PRIMARY)
        );
        // primary drops: secondary may try
        let a = f.disconnected(PRIMARY, &state([false, false], [false; 2]));
        assert_eq!(
            (a.invalidate, a.start_timer),
            ([true, false], [false, true])
        );
        assert_eq!(
            f.reconnect_timer(SECONDARY, &state([false, false], [false; 2]))
                .connect,
            [false, true]
        );
        // secondary connects while primary is down: it takes over
        f.connected(SECONDARY, &state([false, true], [false; 2]));
        assert_eq!(f.selected, SECONDARY);
        assert!(f.accepts_notify_from(SECONDARY) && !f.accepts_notify_from(PRIMARY));
        // primary back: selected again, secondary dropped
        let a = f.connected(PRIMARY, &state([true, true], [false; 2]));
        assert_eq!((f.selected, a.disconnect), (PRIMARY, [false, true]));
        // a blocked primary does not keep the secondary away
        assert_eq!(
            f.reconnect_timer(SECONDARY, &state([true, false], [true, false]))
                .connect,
            [false, true]
        );
    }

    #[test]
    fn dual_pool_dithering() {
        let mut d = DualPool::default();
        d.set_balance(75);
        let picks: Vec<usize> = (0..8).map(|_| d.next_active_pool([true, true])).collect();
        assert_eq!(picks.iter().filter(|&&p| p == SECONDARY).count(), 2);
        assert_eq!(d.next_active_pool([false, true]), SECONDARY);
        assert_eq!(d.next_active_pool([false, false]), PRIMARY);
    }

    #[test]
    fn mode_switch_starts_a_new_policy() {
        let p = Handle::new(false);
        assert!(!p.set_mode(false, 50));
        assert!(!p.set_balance(70));
        assert!(p.set_mode(true, 70));
        // dual, already at the balance it was switched with
        assert!(!p.set_balance(70));
        assert!(p.accepts_notify_from(1));
        assert!(p.set_mode(false, 70));
        assert!(!p.accepts_notify_from(1));
    }

    #[test]
    fn dual_pool_asic_diff_forgets_disconnected_pool() {
        let mut d = DualPool::default();
        assert_eq!(d.select_asic_diff(0, 4096, 256, 65536), 4096);
        assert_eq!(d.select_asic_diff(1, 512, 256, 65536), 512);
        assert_eq!(d.select_asic_diff(0, 4096, 256, 65536), 512);
        d.disconnected(1);
        assert_eq!(d.select_asic_diff(0, 4096, 256, 65536), 4096);
        assert_eq!(d.select_asic_diff(1, 100, 256, 65536), 256);
        assert!(d.pool_diff_err[1]);
    }

    #[test]
    fn verification() {
        let base = VerifyInput {
            mode: 2,
            max_fee: 20, // 2.0 %
            force: true,
            user_value_satoshis: 98,
            total_value_satoshis: 100,
            block_height: 900_000,
            was_ok: true,
            blocked: block::NONE,
        };
        let r = verify(&base);
        assert_eq!(
            (r.ok, r.alert, r.blocked, r.reconnect),
            (true, false, block::NONE, false)
        );
        let r = verify(&VerifyInput {
            user_value_satoshis: 97,
            ..base
        });
        assert_eq!(
            (r.ok, r.alert, r.blocked, r.reconnect),
            (false, true, block::FEE_EXCEEDED, true)
        );
        let r = verify(&VerifyInput {
            user_value_satoshis: 0,
            ..base
        });
        assert_eq!(r.blocked, block::ADDRESS_NOT_FOUND);
        let r = verify(&VerifyInput {
            blocked: block::FEE_EXCEEDED,
            ..base
        });
        assert_eq!((r.blocked, r.reconnect), (block::NONE, false));
        assert!(!verify(&VerifyInput { mode: 0, ..base }).counted);
        assert!(!any_usable([true, false], [true, false]));
        assert!(any_usable([true, true], [true, false]));
    }
}
