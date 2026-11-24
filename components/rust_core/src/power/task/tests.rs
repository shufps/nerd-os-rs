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
