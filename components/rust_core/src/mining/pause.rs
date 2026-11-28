//! Mining pause and resume (`POST /api/v2/mining/pause` and `/resume`):
//! the state, the requested change and the gate the mining tasks park at.
//! The power task runs the transitions under the hardware lock, one step
//! per cycle (`power/task.rs`): close the gate, power the chain off once
//! every mining task is parked; power and enumerate it again as at boot,
//! then open the gate.
//!
//! The parked tasks are the ones that use the chain's UART: the job task,
//! the result task (it would read the replies of the enumeration) and the
//! hashrate monitor (its counter reads would answer the enumeration too).

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum State {
    #[default]
    Running = 0,
    /// the gate is closed, the tasks park
    Pausing = 1,
    Paused = 2,
    /// the chain is powered and enumerated (one power task cycle)
    Resuming = 3,
}

impl State {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => State::Pausing,
            2 => State::Paused,
            3 => State::Resuming,
            _ => State::Running,
        }
    }

    /// The API's name (`miningState`).
    pub fn name(self) -> &'static core::ffi::CStr {
        match self {
            State::Running => c"running",
            State::Pausing => c"pausing",
            State::Paused => c"paused",
            State::Resuming => c"resuming",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Request {
    Pause,
    Resume,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refused {
    /// no mining (no pool user, a CAN slave) or shut down
    Unavailable,
    /// the other change is in progress
    Busy,
}

/// The tasks that park while paused (a bit each).
#[derive(Clone, Copy)]
#[repr(u8)]
pub enum Task {
    Jobs = 1,
    Results = 2,
    Hashrate = 4,
}

static STATE: AtomicU8 = AtomicU8::new(State::Running as u8);
/// 0 none, 1 pause, 2 resume (the latest request wins)
static REQUEST: AtomicU8 = AtomicU8::new(0);
/// the mining tasks were started
static ENABLED: AtomicBool = AtomicBool::new(false);

struct Gate {
    closed: bool,
    /// the tasks that run (registered at their start)
    started: u8,
    parked: u8,
}

static GATE: Mutex<Gate> = Mutex::new(Gate {
    closed: false,
    started: 0,
    parked: 0,
});
static WAKE: Condvar = Condvar::new();

fn gate() -> MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn state() -> State {
    State::from_u8(STATE.load(Ordering::Acquire))
}

/// No pool connections: from the pause request until the resume starts (the
/// pools reconnect while the chain is initialized).
pub fn pools_paused() -> bool {
    matches!(state(), State::Pausing | State::Paused)
}

/// The power task's current state.
pub fn set_state(s: State) {
    STATE.store(s as u8, Ordering::Release);
}

/// Pause and resume are possible (boot, once the mining tasks run).
pub fn enable() {
    ENABLED.store(true, Ordering::Release);
}

/// The mining tasks run (pause and resume possible).
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Acquire)
}

/// A request of the API. Ok also when the miner already is (or gets) there;
/// the power task acts within a cycle.
pub fn request(r: Request) -> Result<(), Refused> {
    if !ENABLED.load(Ordering::Acquire) || crate::power::mgmt::is_shutdown() {
        return Err(Refused::Unavailable);
    }
    match (r, state()) {
        (Request::Pause, State::Resuming) | (Request::Resume, State::Pausing) => Err(Refused::Busy),
        _ => {
            let v = match r {
                Request::Pause => 1,
                Request::Resume => 2,
            };
            REQUEST.store(v, Ordering::Release);
            Ok(())
        }
    }
}

/// The request since the last call (power task).
pub fn take_request() -> Option<Request> {
    match REQUEST.swap(0, Ordering::AcqRel) {
        1 => Some(Request::Pause),
        2 => Some(Request::Resume),
        _ => None,
    }
}

/// A mining task runs and will park at its `park()` calls.
pub fn register(t: Task) {
    gate().started |= t as u8;
}

/// Closes or opens the gate (power task); opening wakes the parked tasks.
pub fn set_closed(closed: bool) {
    gate().closed = closed;
    if !closed {
        WAKE.notify_all();
    }
}

/// Every running mining task is parked.
pub fn parked() -> bool {
    let g = gate();
    g.parked == g.started
}

/// Blocks while the gate is closed; true if the task was parked.
pub fn park(t: Task) -> bool {
    let mut g = gate();
    if !g.closed {
        return false;
    }
    g.parked |= t as u8;
    while g.closed {
        g = WAKE.wait(g).unwrap_or_else(|e| e.into_inner());
    }
    g.parked &= !(t as u8);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tests share the statics.
    fn lock() -> MutexGuard<'static, ()> {
        static L: Mutex<()> = Mutex::new(());
        L.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn reset() {
        set_state(State::Running);
        REQUEST.store(0, Ordering::Release);
        ENABLED.store(true, Ordering::Release);
        *gate() = Gate {
            closed: false,
            started: 0,
            parked: 0,
        };
    }

    #[test]
    fn requests_need_running_mining() {
        let _l = lock();
        reset();
        ENABLED.store(false, Ordering::Release);
        assert_eq!(request(Request::Pause), Err(Refused::Unavailable));
        assert_eq!(take_request(), None);
    }

    #[test]
    fn the_latest_request_wins() {
        let _l = lock();
        reset();
        assert_eq!(request(Request::Pause), Ok(()));
        assert_eq!(request(Request::Resume), Ok(()));
        assert_eq!(take_request(), Some(Request::Resume));
        assert_eq!(take_request(), None);
    }

    #[test]
    fn no_reversal_during_a_change() {
        let _l = lock();
        reset();
        set_state(State::Pausing);
        assert_eq!(request(Request::Resume), Err(Refused::Busy));
        assert_eq!(request(Request::Pause), Ok(()));
        set_state(State::Resuming);
        assert_eq!(request(Request::Pause), Err(Refused::Busy));
        assert_eq!(request(Request::Resume), Ok(()));
        reset();
    }

    #[test]
    fn the_gate_parks_the_registered_tasks() {
        let _l = lock();
        reset();
        // nothing runs: nothing to wait for
        set_closed(true);
        assert!(parked());
        set_closed(false);

        register(Task::Jobs);
        register(Task::Results);
        assert!(!park(Task::Jobs), "open gate: no parking");
        set_closed(true);
        assert!(!parked());
        let t: Vec<_> = [Task::Jobs, Task::Results]
            .into_iter()
            .map(|t| std::thread::spawn(move || park(t)))
            .collect();
        while !parked() {
            std::thread::yield_now();
        }
        set_closed(false);
        for h in t {
            assert!(h.join().unwrap());
        }
        assert_eq!(gate().parked, 0);
        reset();
    }
}
