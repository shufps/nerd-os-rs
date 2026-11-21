//! Compares the Rust pool policies (`src/pool.rs`) with the former stratum
//! manager logic, extracted verbatim into cpp-shim/ref_manager.cpp: random
//! sequences of reconnect-timer, connect and disconnect events in failover
//! and dual-pool mode, the dual-pool job dithering, ASIC difficulty and
//! balance, and the coinbase verification.
//!
//! `DIFF_ITERATIONS` (default 100000) and `DIFF_SEED` override the run.

use std::cell::Cell;
use std::sync::Mutex;

// Links the native reference libraries built by build.rs.
use c_reference as _;
use rust_core::pool::{any_usable, verify, Actions, DualPool, Failover, PoolState, VerifyInput};

#[path = "../../tests/golden/mod.rs"]
mod golden;
use golden::Rec;

#[repr(C)]
#[derive(Debug, PartialEq)]
struct VerifyOut {
    ok: bool,
    counted: bool,
    alert: bool,
    blocked: u8,
    reconnect: bool,
    fault: i32,
}

extern "C" {
    fn ref_failover(
        event: i32,
        pool: i32,
        has0: bool,
        has1: bool,
        c0: bool,
        c1: bool,
        b0: bool,
        b1: bool,
        selected: *mut i32,
    ) -> Actions;
    fn ref_dual(
        event: i32,
        pool: i32,
        has0: bool,
        has1: bool,
        c0: bool,
        c1: bool,
        b0: bool,
        b1: bool,
        diff_err: *mut [bool; 2],
    ) -> Actions;
    fn ref_reset();
    fn ref_dual_next(balance: i32, valid0: bool, valid1: bool, b0: bool, b1: bool) -> i32;
    fn ref_dual_asic_diff(pool: i32, diff: u32, min: u32, max: u32, diff_err: *mut bool) -> u32;
    fn ref_dual_balance(balance: i32, pool: i32, has0: bool, has1: bool, c0: bool, c1: bool)
        -> i32;
    #[allow(clippy::too_many_arguments)]
    fn ref_verify(
        mode: u16,
        max_fee: u16,
        force: bool,
        user: u64,
        total: u64,
        height: u32,
        was_ok: bool,
        blocked: u8,
        configured0: bool,
        configured1: bool,
        other_blocked: bool,
        fault_before: i32,
    ) -> VerifyOut;
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

/// The reference keeps its state in globals: one test at a time.
static REFERENCE: Mutex<()> = Mutex::new(());

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

// The reference and Rust objects keep state across calls (selected pool,
// dithering accumulator), so each test drives one long sequence on both.

#[test]
fn failover_matches_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "failover_matches_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed));
    let mut f = Failover::default();
    for i in 0..n {
        let (event, pool, mut s) = (rng.below(3), rng.below(2), random_state(&rng));
        s.has_task[pool] = true; // the callbacks come from the pool's own task
        let mut selected = 0;
        // SAFETY: plain values.
        let expected = unsafe {
            let [h0, h1] = s.has_task;
            let [c0, c1] = s.connected;
            let [b0, b1] = s.verify_blocked;
            ref_failover(
                event as i32,
                pool as i32,
                h0,
                h1,
                c0,
                c1,
                b0,
                b1,
                &mut selected,
            )
        };
        let got = match event {
            0 => f.reconnect_timer(pool, &s),
            1 => f.connected(pool, &s),
            _ => f.disconnected(pool, &s),
        };
        assert_eq!(
            (got, f.selected as i32),
            (expected, selected),
            "case {i}: event {event} pool {pool} {s:?}"
        );
        rec.add(format!("{i} {event} {pool} {got:?} {}", f.selected));
    }
    rec.finish();
}

#[test]
fn dual_pool_events_match_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "dual_pool_events_match_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 1));
    let mut d = DualPool::default();
    for i in 0..n {
        let (event, pool, mut s) = (rng.below(3), rng.below(2), random_state(&rng));
        s.has_task[pool] = true; // the callbacks come from the pool's own task
        let mut diff_err = [false; 2];
        // SAFETY: plain values.
        let expected = unsafe {
            let [h0, h1] = s.has_task;
            let [c0, c1] = s.connected;
            let [b0, b1] = s.verify_blocked;
            ref_dual(
                event as i32,
                pool as i32,
                h0,
                h1,
                c0,
                c1,
                b0,
                b1,
                &mut diff_err,
            )
        };
        let got = match event {
            0 => d.reconnect_timer(pool, &s),
            1 => d.connected(pool, &s),
            _ => d.disconnected(pool),
        };
        assert_eq!(
            (got, d.pool_diff_err),
            (expected, diff_err),
            "case {i}: event {event} pool {pool} {s:?}"
        );
        rec.add(format!("{i} {event} {pool} {got:?} {:?}", d.pool_diff_err));
    }
    rec.finish();
}

#[test]
fn dual_pool_dithering_matches_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "dual_pool_dithering_matches_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 2));
    let mut d = DualPool::default();
    let mut balance = 50;
    for i in 0..n {
        if rng.below(500) == 0 {
            balance = rng.below(101) as u16;
            d.set_balance(balance);
            // the C++ manager resets the accumulator with the balance
            unsafe { ref_dual_next(balance as i32, false, false, false, false) };
            d.next_active_pool([false, false]);
        }
        let (valid, blocked) = (
            [rng.below(8) > 0, rng.below(8) > 0],
            [rng.below(10) == 0, rng.below(10) == 0],
        );
        // SAFETY: plain values.
        let expected =
            unsafe { ref_dual_next(balance as i32, valid[0], valid[1], blocked[0], blocked[1]) };
        let got = d.next_active_pool([valid[0] && !blocked[0], valid[1] && !blocked[1]]);
        assert_eq!(
            got as i32, expected,
            "case {i}: balance {balance} {valid:?} {blocked:?}"
        );
        rec.add(format!("{i} {balance} {got}"));
    }
    rec.finish();
}

/// Without disconnects in between, the ASIC difficulty matches; with one,
/// only the Rust side forgets the disconnected pool's difficulty (#25).
#[test]
fn dual_pool_asic_diff_matches_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "dual_pool_asic_diff_matches_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
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
        let mut err = false;
        // SAFETY: plain values.
        let expected = unsafe { ref_dual_asic_diff(pool as i32, diff, mn, mx, &mut err) };
        let got = d.select_asic_diff(pool, diff, mn, mx);
        assert_eq!((got, d.pool_diff_err[pool]), (expected, err), "case {i}");
        rec.add(format!("{i} {pool} {diff} {got} {}", d.pool_diff_err[pool]));
    }
    rec.finish();
}

#[test]
fn dual_pool_balance_matches_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "dual_pool_balance_matches_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
    let (n, seed) = iterations();
    let rng = Rng(Cell::new(seed ^ 4));
    let mut d = DualPool::default();
    for i in 0..n / 10 {
        let balance = rng.below(101) as u16;
        d.set_balance(balance);
        let (pool, s) = (rng.below(2), random_state(&rng));
        // SAFETY: plain values.
        let expected = unsafe {
            ref_dual_balance(
                balance as i32,
                pool as i32,
                s.has_task[0],
                s.has_task[1],
                s.connected[0],
                s.connected[1],
            )
        };
        let got = d.active_balance(pool, &s);
        assert_eq!(got, expected, "case {i}");
        rec.add(format!("{i} {balance} {pool} {got}"));
    }
    rec.finish();
}

#[test]
fn verification_matches_reference() {
    let _serial = REFERENCE.lock().unwrap_or_else(|e| e.into_inner());
    let mut rec = Rec::new("pool_policy", "verification_matches_reference");
    // SAFETY: resets the reference's globals.
    unsafe { ref_reset() };
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
        // SAFETY: plain values.
        let expected = unsafe {
            ref_verify(
                input.mode,
                input.max_fee,
                input.force,
                user,
                total,
                input.block_height,
                input.was_ok,
                input.blocked,
                configured[0],
                configured[1],
                other_blocked,
                fault_before,
            )
        };
        let r = verify(&input);
        // The fault flag as the C++ manager now derives it from the result.
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
        assert_eq!(
            got, expected,
            "case {i}: {input:?} configured {configured:?} other_blocked {other_blocked}"
        );
        rec.add(format!("{i} {got:?}"));
    }
    rec.finish();
}
