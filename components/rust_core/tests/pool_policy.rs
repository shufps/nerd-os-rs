//! Golden tests of the pool policies (`src/pool.rs`): random sequences of
//! reconnect-timer, connect and disconnect events in failover and dual-pool
//! mode, the dual-pool job dithering, ASIC difficulty and balance, and the
//! coinbase verification, against the outputs of the former C/C++ code (the
//! stratum manager logic; tests/golden/pool_policy.txt, recorded by the
//! differential tests).

use std::cell::Cell;

use rust_core::pool::{any_usable, verify, DualPool, Failover, PoolState, VerifyInput};

mod golden;
use golden::Rec;

/// The verification outcome as recorded (with the manager's fault flag).
#[derive(Debug, PartialEq)]
#[allow(dead_code)] // fields read through Debug only
struct VerifyOut {
    ok: bool,
    counted: bool,
    alert: bool,
    blocked: u8,
    reconnect: bool,
    fault: i32,
}

struct Rng(Cell<u64>);

impl Rng {
    fn next(&self) -> u64 {
        // xorshift64*
        let mut x = self.0.get();
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0.set(x);
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn bit(&self) -> bool {
        self.next() & 1 == 1
    }
}

/// Fixed (the golden file records the outputs for exactly these inputs).
fn iterations() -> (usize, u64) {
    (100_000, 0x0b00_1500_0000_0007)
}

/// `connected` is the manager's `isConnected()`: the task exists and is connected.
fn random_state(rng: &Rng) -> PoolState {
    let has_task = [rng.below(10) > 0, rng.below(10) > 0];
    PoolState {
        has_task,
        connected: [has_task[0] && rng.bit(), has_task[1] && rng.bit()],
        verify_blocked: [rng.below(5) == 0, rng.below(5) == 0],
    }
}

// The objects keep state across calls (selected pool, dithering
// accumulator), so each test drives one long sequence.

#[test]
fn failover_matches_reference() {
    let mut rec = Rec::new("pool_policy", "failover_matches_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let mut f = Failover::default();
    for i in 0..n {
        let (event, pool, mut s) = (rng.below(3), rng.below(2), random_state(&rng));
        s.has_task[pool] = true; // the callbacks come from the pool's own task
        let got = match event {
            0 => f.reconnect_timer(pool, &s),
            1 => f.connected(pool, &s),
            _ => f.disconnected(pool, &s),
        };
        rec.add(format!("{i} {event} {pool} {got:?} {}", f.selected));
    }
    rec.finish();
}

#[test]
fn dual_pool_events_match_reference() {
    let mut rec = Rec::new("pool_policy", "dual_pool_events_match_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 1));
    let mut d = DualPool::default();
    for i in 0..n {
        let (event, pool, mut s) = (rng.below(3), rng.below(2), random_state(&rng));
        s.has_task[pool] = true; // the callbacks come from the pool's own task
        let got = match event {
            0 => d.reconnect_timer(pool, &s),
            1 => d.connected(pool, &s),
            _ => d.disconnected(pool),
        };
        rec.add(format!("{i} {event} {pool} {got:?} {:?}", d.pool_diff_err));
    }
    rec.finish();
}

#[test]
fn dual_pool_dithering_matches_reference() {
    let mut rec = Rec::new("pool_policy", "dual_pool_dithering_matches_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 2));
    let mut d = DualPool::default();
    let mut balance = 50;
    for i in 0..n {
        if rng.below(500) == 0 {
            balance = rng.below(101) as u16;
            d.set_balance(balance);
            // the C++ manager reset the accumulator with the balance
            d.next_active_pool([false, false]);
        }
        let (valid, blocked) = (
            [rng.below(8) > 0, rng.below(8) > 0],
            [rng.below(10) == 0, rng.below(10) == 0],
        );
        let got = d.next_active_pool([valid[0] && !blocked[0], valid[1] && !blocked[1]]);
        rec.add(format!("{i} {balance} {got}"));
    }
    rec.finish();
}

/// Without disconnects in between, the ASIC difficulty matches the C++
/// manager's; with one, only the Rust side forgets the disconnected pool's
/// difficulty (#25).
#[test]
fn dual_pool_asic_diff_matches_reference() {
    let mut rec = Rec::new("pool_policy", "dual_pool_asic_diff_matches_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 3));
    let mut d = DualPool::default();
    for i in 0..n {
        let pool = rng.below(2);
        let diff = match rng.below(4) {
            0 => rng.below(300) as u32,
            1 => rng.next() as u32,
            _ => 1 << rng.below(20),
        };
        let (mn, mx) = (256, 1 << 16);
        let got = d.select_asic_diff(pool, diff, mn, mx);
        rec.add(format!("{i} {pool} {diff} {got} {}", d.pool_diff_err[pool]));
    }
    rec.finish();
}

#[test]
fn dual_pool_balance_matches_reference() {
    let mut rec = Rec::new("pool_policy", "dual_pool_balance_matches_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 4));
    let mut d = DualPool::default();
    for i in 0..n / 10 {
        let balance = rng.below(101) as u16;
        d.set_balance(balance);
        let (pool, s) = (rng.below(2), random_state(&rng));
        let got = d.active_balance(pool, &s);
        rec.add(format!("{i} {balance} {pool} {got}"));
    }
    rec.finish();
}

#[test]
fn verification_matches_reference() {
    let mut rec = Rec::new("pool_policy", "verification_matches_reference");
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 5));
    const FAULT: i32 = 7;
    for i in 0..n {
        let total = match rng.below(4) {
            0 => 0,
            1 => rng.next() >> rng.below(64),
            _ => 312_500_000 + rng.below(1_000_000) as u64,
        };
        let user = match rng.below(4) {
            0 => 0,
            1 => total,
            2 => rng.next() >> rng.below(64),
            _ => total.saturating_sub(rng.below(20_000_000) as u64),
        };
        let input = VerifyInput {
            mode: rng.below(4) as u16,
            max_fee: rng.below(100) as u16,
            force: rng.bit(),
            user_value_satoshis: user,
            total_value_satoshis: total,
            block_height: if rng.below(5) == 0 { 0 } else { 900_000 },
            was_ok: rng.bit(),
            blocked: rng.below(3) as u8,
        };
        let (configured, other_blocked) = ([rng.below(5) > 0, rng.bit()], rng.bit());
        let fault_before = if rng.bit() { FAULT } else { 0 };
        let r = verify(&input);
        // The fault flag as the manager derives it from the result.
        let fault = if r.check_fault {
            if !any_usable(configured, [r.blocked != 0, other_blocked]) {
                FAULT
            } else if fault_before == FAULT {
                0
            } else {
                fault_before
            }
        } else {
            fault_before
        };
        let got = VerifyOut {
            ok: r.ok,
            counted: r.counted,
            alert: r.alert,
            blocked: r.blocked,
            reconnect: r.reconnect,
            fault,
        };
        rec.add(format!("{i} {got:?}"));
    }
    rec.finish();
}
