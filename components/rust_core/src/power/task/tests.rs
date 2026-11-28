//! The cycle against the sequence of the former C++ task
//! (`power_management_task.cpp`), on a recording fake board.

use super::*;

#[derive(Default)]
struct Fake {
    log: Vec<String>,
    shutdown: bool,
    initialized: bool,
    buck: bool,
    mv: u16,
    mhz: u16,
    vr_hz: u32,
    chips: Vec<f32>,
    sensors: Vec<f32>,
    power: [f32; 8],
    fault: (c_int, u32),
    overheated: [bool; 2],
    now: u64,
    request: Option<Request>,
    parked: bool,
    resume_ok: bool,
    mining_state: Option<MiningState>,
    ramping: bool,
}

impl Fake {
    fn rec(&mut self, s: impl Into<String>) {
        self.log.push(s.into());
    }
    fn take(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}

impl Env for Fake {
    fn lock(&mut self) {
        self.rec("lock");
    }
    fn unlock(&mut self) {
        self.rec("unlock");
    }
    fn is_shutdown(&mut self) -> bool {
        self.shutdown
    }
    fn is_initialized(&mut self) -> bool {
        self.initialized
    }
    fn is_buck_initialized(&mut self) -> bool {
        self.buck
    }
    fn asic_voltage_millis(&mut self) -> u16 {
        self.mv
    }
    fn set_voltage(&mut self, volts: f32) {
        self.rec(format!("voltage {volts}"));
    }
    fn asic_frequency(&mut self) -> u16 {
        self.mhz
    }
    fn set_asic_frequency(&mut self, mhz: f32) -> bool {
        self.rec(format!("frequency {mhz}"));
        true
    }
    fn frequency_ramping(&mut self) -> bool {
        self.ramping
    }
    fn vr_frequency(&mut self) -> u32 {
        self.vr_hz
    }
    fn set_vr_frequency(&mut self, hz: u32) {
        self.rec(format!("vr {hz}"));
    }
    fn request_chip_temps(&mut self) {
        self.rec("chip temps");
    }
    fn max_chip_temp(&mut self) -> f32 {
        self.chips.iter().fold(0.0, |a, &b| cmax(a, b))
    }
    fn asic_count(&mut self) -> c_int {
        self.chips.len() as c_int
    }
    fn chip_temp(&mut self, i: c_int) -> f32 {
        self.chips[i as usize]
    }
    fn request_buck_telemetry(&mut self) {
        self.rec("buck telemetry");
    }
    fn power(&mut self, what: c_int) -> f32 {
        self.power[what as usize]
    }
    fn fault(&mut self) -> (c_int, u32) {
        self.fault
    }
    fn set_board_error(&mut self, error: c_int, status: u32) {
        self.rec(format!("error {error} {status:08x}"));
    }
    fn num_temp_sensors(&mut self) -> c_int {
        self.sensors.len() as c_int
    }
    fn temperature(&mut self, i: c_int) -> f32 {
        self.sensors[i as usize]
    }
    fn fan_update(&mut self, chip: f32, vr: f32) {
        self.rec(format!("fan {chip} {vr}"));
    }
    fn fan_get(&mut self, ch: c_int, what: c_int) -> u16 {
        match what {
            0 => 1000 + ch as u16,
            1 => 30 + ch as u16,
            2 => self.overheated[ch as usize] as u16,
            _ => 70 + ch as u16,
        }
    }
    fn influx_pwr(&mut self, v: [f32; 6]) {
        self.rec(format!("influx pwr {v:?}"));
    }
    fn influx_temperature(&mut self, chip: f32, vr: f32) {
        self.rec(format!("influx temp {chip} {vr}"));
    }
    fn influx_fan(&mut self, v: [f32; 4]) {
        self.rec(format!("influx fan {v:?}"));
    }
    fn now_us(&mut self) -> u64 {
        self.now
    }
    fn take_mining_request(&mut self) -> Option<Request> {
        self.request.take()
    }
    fn set_mining_state(&mut self, s: MiningState) {
        self.mining_state = Some(s);
    }
    fn close_mining_gate(&mut self, closed: bool) {
        self.rec(if closed { "gate closed" } else { "gate open" });
    }
    fn mining_parked(&mut self) -> bool {
        self.parked
    }
    fn pause_board(&mut self) {
        self.rec("board pause");
        self.initialized = false;
        self.chips.iter_mut().for_each(|c| *c = 0.0);
    }
    fn resume_board(&mut self) -> bool {
        self.rec("board resume");
        self.initialized = self.resume_ok;
        self.resume_ok
    }
    fn clear_hardware_errors(&mut self) {
        self.rec("errors cleared");
    }
}

fn running() -> Fake {
    Fake {
        initialized: true,
        buck: true,
        mv: 1010,
        mhz: 600,
        vr_hz: 4000,
        chips: vec![55.5, 57.25, 56.0, 0.0],
        sensors: vec![],
        power: [12.1, 11.5, 139.0, 120.0, 1.01, 118.8, 57.0, 52.0],
        ..Fake::default()
    }
}

#[test]
fn first_cycles() {
    let t = PowerTask::new(false);
    let mut f = running();
    t.cycle(&mut f);
    // first cycle: settings applied, telemetry requested, chip max for the PID
    assert_eq!(
        f.take(),
        [
            "lock",
            "voltage 1.01",
            "frequency 600",
            "vr 4000",
            "chip temps",
            "buck telemetry",
            "influx pwr [12.1, 11.5, 139.0, 1.01, 118.8, 120.0]",
            "influx temp 57.25 57",
            "fan 57.25 57",
            "influx fan [30.0, 1000.0, 31.0, 1001.0]",
            "unlock",
        ]
    );
    assert_eq!(t.get(0), 139.0);
    assert_eq!(t.get(1), (12.1f32 as f64 * 1000.0) as f32);
    assert_eq!(t.get(2), 11500.0);
    assert_eq!(t.get(3), 57.25);
    assert_eq!(t.get(4), 57.0);
    assert_eq!(t.get(5), 52.0);

    // unchanged settings: nothing applied; telemetry request every 15 s
    f.now = 14_999_999;
    t.cycle(&mut f);
    let l = f.take();
    assert!(!l
        .iter()
        .any(|s| s.starts_with("voltage") || s == "buck telemetry"));
    f.now = 15_000_000;
    f.mhz = 625;
    t.cycle(&mut f);
    let l = f.take();
    assert_eq!(l[1], "frequency 625");
    assert_eq!(l[3], "buck telemetry");
    // late: one request, the next one on the 15 s grid
    f.now = 47_000_000;
    t.cycle(&mut f);
    assert_eq!(
        f.take().iter().filter(|s| *s == "buck telemetry").count(),
        1
    );
    f.now = 59_000_000;
    t.cycle(&mut f);
    assert!(!f.take().iter().any(|s| s == "buck telemetry"));
    f.now = 60_000_000;
    t.cycle(&mut f);
    assert!(f.take().iter().any(|s| s == "buck telemetry"));
}

#[test]
fn not_initialized_and_shutdown() {
    let t = PowerTask::new(false);
    let mut f = Fake {
        sensors: vec![41.0, 0.0, 43.5],
        ..Fake::default()
    };
    t.cycle(&mut f);
    // no settings, no chip temps, no telemetry; the board sensors drive the fan
    assert_eq!(
        f.take(),
        [
            "lock",
            "influx temp 43.5 0",
            "fan 43.5 0",
            "influx fan [30.0, 1000.0, 31.0, 1001.0]",
            "unlock"
        ]
    );
    let mut f = running();
    f.shutdown = true;
    t.cycle(&mut f);
    let l = f.take();
    assert_eq!(l[1], "chip temps");
}

#[test]
fn temperature_selection() {
    // other boards: the chips if available, else the board sensors
    let t = PowerTask::new(false);
    let mut f = running();
    f.sensors = vec![60.0];
    t.cycle(&mut f);
    assert!(f.take().contains(&"fan 57.25 57".to_string()));
    // NQ+: the maximum of both
    let t = PowerTask::new(true);
    let mut f = running();
    f.sensors = vec![60.0, f32::NAN];
    t.cycle(&mut f);
    assert!(f.take().contains(&"fan 60 57".to_string()));
}

#[test]
fn faults_and_overheat() {
    let t = PowerTask::new(false);
    let mut f = running();
    f.fault = (4, 0x1234);
    t.cycle(&mut f);
    let l = f.take();
    let i = l.iter().position(|s| s.starts_with("influx pwr")).unwrap();
    assert_eq!(l[i + 1..i + 3], ["error 4 00001234", "voltage 0"]);

    // the VR channel overheated
    f.fault = (0, 0);
    f.overheated = [false, true];
    t.cycle(&mut f);
    let l = f.take();
    let i = l.iter().position(|s| s.starts_with("fan ")).unwrap();
    assert_eq!(l[i + 1..i + 3], ["error 2 39463947", "voltage 0"]);
    // both: the ASIC wins
    f.overheated = [true, true];
    t.cycle(&mut f);
    assert!(f.take().contains(&"error 1 39463947".to_string()));
    // negative/NaN temperatures saturate in the status word
    f.chips = vec![];
    f.sensors = vec![-5.0];
    f.power[6] = f32::NAN;
    t.cycle(&mut f);
    assert!(f.take().contains(&"error 1 00460047".to_string()));
}

/// Runs cycles until `n` have passed, `dt_us` apart.
fn cycles(t: &PowerTask, f: &mut Fake, n: usize, dt_us: u64) -> Vec<String> {
    let mut l = Vec::new();
    for _ in 0..n {
        f.now += dt_us;
        t.cycle(f);
        l.extend(f.take());
    }
    l
}

#[test]
fn pause_waits_for_the_parked_tasks() {
    let t = PowerTask::new(false);
    let mut f = running();
    t.cycle(&mut f);
    f.take();

    f.request = Some(Request::Pause);
    t.cycle(&mut f);
    let l = f.take();
    assert_eq!(l[..2], ["lock", "gate closed"]);
    assert_eq!(f.mining_state, Some(MiningState::Pausing));
    // still powered: faults count until the chain is off
    f.fault = (4, 1);
    t.cycle(&mut f);
    let l = f.take();
    assert!(
        !l.contains(&"board pause".to_string()),
        "tasks not parked yet"
    );
    assert!(l.contains(&"error 4 00000001".to_string()));

    f.parked = true;
    t.cycle(&mut f);
    let l = f.take();
    assert_eq!(l[..2], ["lock", "board pause"]);
    assert_eq!(f.mining_state, Some(MiningState::Paused));
    // paused: no settings, no chip temperatures, no regulator faults; the
    // telemetry and the fans go on
    f.mv = 1100;
    let l = cycles(&t, &mut f, 3, 2_000_000);
    assert!(!l
        .iter()
        .any(|s| s.starts_with("voltage") || s.starts_with("error") || s == "chip temps"));
    assert!(l.iter().any(|s| s.starts_with("fan ")));
    assert!(l.iter().any(|s| s.starts_with("influx pwr")));
    // a second pause changes nothing
    f.request = Some(Request::Pause);
    assert!(!cycles(&t, &mut f, 1, 2_000_000).contains(&"board pause".to_string()));
}

#[test]
fn resume_reinitializes_and_applies_the_settings() {
    let t = PowerTask::new(false);
    let mut f = running();
    f.parked = true;
    f.resume_ok = true;
    t.cycle(&mut f);
    f.request = Some(Request::Pause);
    cycles(&t, &mut f, 2, 2_000_000);
    assert_eq!(f.mining_state, Some(MiningState::Paused));

    // too early: the request waits for the minimum off time
    f.request = Some(Request::Resume);
    let l = cycles(&t, &mut f, 2, 2_000_000);
    assert!(!l.contains(&"board resume".to_string()));
    assert_eq!(f.mining_state, Some(MiningState::Paused));

    // a voltage changed while paused is applied after the resume, the
    // unchanged frequency is sent again (the chain was reset)
    f.mv = 1100;
    let l = cycles(&t, &mut f, 1, 2_000_000);
    assert_eq!(
        l[..7],
        [
            "lock",
            "board resume",
            "errors cleared",
            "gate open",
            "voltage 1.1",
            "frequency 600",
            "vr 4000"
        ]
    );
    assert_eq!(f.mining_state, Some(MiningState::Running));
    // faults count again
    f.fault = (3, 2);
    assert!(cycles(&t, &mut f, 1, 2_000_000).contains(&"error 3 00000002".to_string()));
}

#[test]
fn a_failed_resume_stays_paused() {
    let t = PowerTask::new(false);
    let mut f = running();
    f.parked = true;
    t.cycle(&mut f);
    f.request = Some(Request::Pause);
    cycles(&t, &mut f, 2, 2_000_000);
    f.request = Some(Request::Resume);
    let l = cycles(&t, &mut f, 1, PAUSE_MIN_US);
    assert_eq!(l[..3], ["lock", "board resume", "board pause"]);
    assert!(!l.contains(&"gate open".to_string()));
    assert_eq!(f.mining_state, Some(MiningState::Paused));
    // a new request tries again
    f.resume_ok = true;
    f.request = Some(Request::Resume);
    let l = cycles(&t, &mut f, 1, PAUSE_MIN_US);
    assert!(l.contains(&"gate open".to_string()));
}

#[test]
fn no_pause_steps_in_shutdown() {
    let t = PowerTask::new(false);
    let mut f = running();
    f.shutdown = true;
    f.request = Some(Request::Pause);
    t.cycle(&mut f);
    assert!(!f.take().contains(&"gate closed".to_string()));
    assert_eq!(f.mining_state, None);
}

#[test]
fn a_lower_voltage_waits_for_the_frequency_ramp() {
    let t = PowerTask::new(false);
    let mut f = running();
    t.cycle(&mut f);
    f.take();
    // both lower: the frequency first, the voltage once the ramp is down
    f.mhz = 500;
    f.mv = 950;
    t.cycle(&mut f);
    let l = f.take();
    assert!(l.contains(&"frequency 500".to_string()));
    assert!(!l.iter().any(|s| s.starts_with("voltage")));
    f.ramping = true;
    t.cycle(&mut f);
    assert!(!f.take().iter().any(|s| s.starts_with("voltage")));
    f.ramping = false;
    t.cycle(&mut f);
    assert!(f.take().contains(&"voltage 0.95".to_string()));

    // both higher: the voltage at once (before the frequency ramp)
    f.mhz = 650;
    f.mv = 1100;
    t.cycle(&mut f);
    let l = f.take();
    let v = l.iter().position(|s| s == "voltage 1.1").unwrap();
    let fr = l.iter().position(|s| s == "frequency 650").unwrap();
    assert!(v < fr);
}
