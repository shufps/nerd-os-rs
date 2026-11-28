//! The ASIC frequency ramp in its own task: the power task only sets the
//! target ([`request`]), this task walks there in 6.25 MHz steps 100 ms
//! apart ([`frequency_ramp`]). Each step is one frame through the UART (the
//! serial lock, not the hardware lock), so the power task's fan control and
//! overheat check keep running during a ramp (boot, resume, a new setting),
//! and the chain's init no longer holds the locks for the whole ramp.
//!
//! The task parks with the other mining tasks during a pause; after it the
//! chain was reset and the ramp starts again from the driver's (reset)
//! frequency.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use super::frequency_ramp;
use crate::log::log;

const TAG: &core::ffi::CStr = c"freq_ramp";
/// between two steps (as the former ramp)
pub const STEP_MS: u32 = 100;
/// the longest wait for a new target (the pause gate is checked in between)
const IDLE_MS: u64 = 200;

/// What the ramp needs from the firmware.
pub trait Env {
    fn suspend_if_shutdown(&mut self);
    /// Blocks while mining is paused; true if it was paused.
    fn park_if_paused(&mut self) -> bool;
    /// The chain's frequency (MHz); None while it is not initialized.
    fn current(&mut self) -> Option<f32>;
    /// Sends one frequency; false without PLL settings for it.
    fn step(&mut self, mhz: f32) -> bool;
    fn sleep_ms(&mut self, ms: u32);
    /// Waits up to `ms` for a new request; the requested target and whether
    /// it is new.
    fn wait_target(&mut self, ms: u64) -> (Option<f32>, bool) {
        wait_target(ms)
    }
    fn set_ramping(&mut self, on: bool) {
        RAMPING.store(on, Ordering::Release);
    }
}

/// the requested frequency and whether it changed since the task looked
static TARGET: Mutex<(Option<f32>, bool)> = Mutex::new((None, false));
static WAKE: Condvar = Condvar::new();
/// a requested frequency is not reached yet
static RAMPING: AtomicBool = AtomicBool::new(false);

/// Ramps the chain to `mhz` (returns at once).
pub fn request(mhz: f32) {
    *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = (Some(mhz), true);
    RAMPING.store(true, Ordering::Release);
    WAKE.notify_all();
}

/// The chain is not at the requested frequency yet.
pub fn ramping() -> bool {
    RAMPING.load(Ordering::Acquire)
}

fn wait_target(ms: u64) -> (Option<f32>, bool) {
    let mut t = TARGET.lock().unwrap_or_else(|e| e.into_inner());
    if !t.1 {
        t = WAKE
            .wait_timeout(t, Duration::from_millis(ms))
            .unwrap_or_else(|e| e.into_inner())
            .0;
    }
    (t.0, core::mem::take(&mut t.1))
}

/// The ramp under way: its target and the remaining steps.
type Active = Option<(f32, core::iter::Peekable<Steps>)>;

/// The task's state between passes.
#[derive(Default)]
pub struct State {
    active: Active,
    /// a new chain (boot, resume): ramp to the target even without a new
    /// request
    restart: bool,
}

/// The task loop; never returns.
pub fn run(env: &mut impl Env) -> ! {
    let mut active = State {
        restart: true,
        ..State::default()
    };
    loop {
        if step(env, &mut active) {
            env.sleep_ms(STEP_MS);
        }
    }
}

/// One pass: true if a step was sent and more follow.
fn step(env: &mut impl Env, state: &mut State) -> bool {
    env.suspend_if_shutdown();
    if env.park_if_paused() {
        // a new chain after the resume
        state.active = None;
        state.restart = true;
    }
    let Some(current) = env.current() else {
        // no chain (not initialized yet, or being initialized)
        state.active = None;
        state.restart = true;
        env.sleep_ms(IDLE_MS as u32);
        return false;
    };
    // a new target restarts the ramp from where the chain is now
    let wait = if state.active.is_some() { 0 } else { IDLE_MS };
    let (target, new) = env.wait_target(wait);
    if let Some(target) = target {
        let other = state.active.as_ref().map_or(true, |(t, _)| *t != target);
        if (new || state.restart) && other {
            state.active = Some((target, Steps::new(current, target).peekable()));
        }
        state.restart = false;
    }
    let active = &mut state.active;
    let Some((target, steps)) = active.as_mut() else {
        env.set_ramping(false);
        return false;
    };
    let Some(f) = steps.next() else {
        *active = None;
        env.set_ramping(false);
        return false;
    };
    if !env.step(f) {
        log!(
            Error,
            TAG,
            "no PLL settings for {} MHz, ramp to {} MHz stopped",
            f as i32,
            *target as i32
        );
        *active = None;
        env.set_ramping(false);
        return false;
    }
    if steps.peek().is_none() {
        *active = None;
        env.set_ramping(false);
        return false;
    }
    true
}

/// The steps of [`frequency_ramp`] as a nameable type.
pub struct Steps(Box<dyn Iterator<Item = f32> + Send>);

impl Steps {
    fn new(current: f32, target: f32) -> Self {
        Steps(Box::new(frequency_ramp(current, target)))
    }
}

impl Iterator for Steps {
    type Item = f32;
    fn next(&mut self) -> Option<f32> {
        self.0.next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct Fake {
        current: Option<f32>,
        /// the requests, one per pass (None: no new one)
        targets: VecDeque<Option<f32>>,
        sent: Vec<f32>,
        no_pll_above: Option<f32>,
        paused_once: bool,
        ramping: bool,
        last: Option<f32>,
    }

    impl Env for Fake {
        fn suspend_if_shutdown(&mut self) {}
        fn park_if_paused(&mut self) -> bool {
            core::mem::take(&mut self.paused_once)
        }
        fn current(&mut self) -> Option<f32> {
            self.current
        }
        fn step(&mut self, mhz: f32) -> bool {
            if self.no_pll_above.is_some_and(|m| mhz > m) {
                return false;
            }
            self.sent.push(mhz);
            self.current = Some(mhz);
            true
        }
        fn sleep_ms(&mut self, _: u32) {}
        fn wait_target(&mut self, _: u64) -> (Option<f32>, bool) {
            // None: no new request, the last target stays
            match self.targets.pop_front().flatten() {
                Some(t) => {
                    self.last = Some(t);
                    (Some(t), true)
                }
                None => (self.last, false),
            }
        }
        fn set_ramping(&mut self, on: bool) {
            self.ramping = on;
        }
    }

    /// Runs passes until the ramp is idle; the frequencies sent.
    fn drive(f: &mut Fake, active: &mut State) {
        for _ in 0..200 {
            let more = step(f, active);
            if !more && active.active.is_none() && f.targets.is_empty() {
                break;
            }
        }
    }

    #[test]
    fn ramps_like_the_former_blocking_ramp() {
        let mut f = Fake {
            current: Some(56.25),
            targets: [Some(81.0)].into(),
            ..Default::default()
        };
        let mut active = State::default();
        drive(&mut f, &mut active);
        let expected: Vec<f32> = frequency_ramp(56.25, 81.0).collect();
        assert_eq!(f.sent, expected);
        assert!(!f.ramping);
    }

    #[test]
    fn nothing_is_sent_once_the_target_is_reached() {
        let mut f = Fake {
            current: Some(56.25),
            targets: [Some(68.75)].into(),
            ..Default::default()
        };
        let mut active = State::default();
        drive(&mut f, &mut active);
        let n = f.sent.len();
        for _ in 0..20 {
            assert!(!step(&mut f, &mut active));
        }
        assert_eq!(f.sent.len(), n);
    }

    #[test]
    fn a_new_target_turns_the_ramp_around() {
        let mut f = Fake {
            current: Some(100.0),
            targets: [Some(150.0), None, None, Some(90.0)].into(),
            ..Default::default()
        };
        let mut active = State::default();
        drive(&mut f, &mut active);
        // up three steps, then down from there to the new target
        assert_eq!(f.sent[..3], [106.25, 112.5, 118.75]);
        let down: Vec<f32> = frequency_ramp(118.75, 90.0).collect();
        assert_eq!(f.sent[3..], down[..]);
    }

    #[test]
    fn waits_for_the_chain_and_restarts_after_a_pause() {
        let mut f = Fake {
            current: None,
            targets: [Some(75.0)].into(),
            ..Default::default()
        };
        let mut active = State::default();
        assert!(!step(&mut f, &mut active));
        assert!(f.sent.is_empty());
        // the chain is up: the target is kept
        f.current = Some(56.25);
        f.targets.push_back(Some(75.0));
        assert!(step(&mut f, &mut active));
        assert_eq!(f.sent, [62.5]);
        // paused and resumed: a reset chain, the ramp starts over
        f.paused_once = true;
        f.current = Some(56.25);
        f.targets.push_back(Some(75.0));
        drive(&mut f, &mut active);
        let again: Vec<f32> = frequency_ramp(56.25, 75.0).collect();
        assert_eq!(f.sent[1..], again[..]);
    }

    #[test]
    fn stops_at_a_frequency_without_pll_settings() {
        let mut f = Fake {
            current: Some(56.25),
            targets: [Some(80.0)].into(),
            no_pll_above: Some(70.0),
            ..Default::default()
        };
        let mut active = State::default();
        drive(&mut f, &mut active);
        assert_eq!(f.sent, [62.5, 68.75]);
        assert!(active.active.is_none() && !f.ramping);
    }
}
