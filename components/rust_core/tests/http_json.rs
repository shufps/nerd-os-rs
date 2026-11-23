//! Golden tests of the GET documents: rust_core::http_json on random
//! snapshots against the outputs of the former ArduinoJson code
//! (tests/golden/http_json.txt, recorded by the differential tests).

use std::ffi::{c_int, CString};
use std::sync::Mutex;

use rust_core::http_json::{
    self, AlertInfo, AsicInfo, CanMasterNode, DashCoinbase, Dashboard, FanInfo, InfluxInfo,
    ManagerInfo, ManagerPool, SettingsPool, SystemInfo, V2Identify, V2Settings, V2System,
    WifiNetwork,
};

mod golden;
use golden::Rec;

// rust_core's own exports (the history and the CAN master are globals)
use rust_core::can_master::{
    delete as can_master_delete, frame as can_master_frame,
    load_registry as can_master_load_registry, tick as can_master_tick,
};
use rust_core::history::{init as history_init, push as history_push};

static LOCK: Mutex<()> = Mutex::new(());

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
    fn bool(&mut self) -> bool {
        self.next() & 1 == 1
    }
    fn f32(&mut self) -> f32 {
        match self.below(8) {
            0 => f32::from_bits(self.next() as u32),
            1 => [f32::NAN, f32::INFINITY, -0.0, 0.0][self.below(4) as usize],
            2 => self.below(200) as f32,
            _ => (self.below(2_000_000) as f32 - 100_000.0) / 1000.0,
        }
    }
    fn f64(&mut self) -> f64 {
        match self.below(6) {
            0 => f64::from_bits(self.next()),
            1 => self.f32() as f64,
            2 => self.below(1 << 50) as f64 / 7.0,
            _ => (self.below(2_000_000) as f64 - 100_000.0) / 1000.0,
        }
    }
    fn u64(&mut self) -> u64 {
        match self.below(3) {
            0 => self.next(),
            1 => self.below(1000),
            _ => self.next() >> self.below(64),
        }
    }
}

/// A string of the pool, sometimes `None` (NULL).
fn pick(r: &mut Rng) -> Option<CString> {
    const POOL: [&str; 8] = [
        "",
        "NerdQAxe++",
        "stratum+tcp://pool.example.com",
        "bc1qxyz.worker\"1\"",
        "line\nbreak\ttab\\back",
        "ünïcødé ✓",
        "ctl\u{1}\u{1f}",
        "x",
    ];
    if r.below(12) == 0 {
        return None;
    }
    Some(CString::new(POOL[r.below(POOL.len() as u64) as usize]).unwrap())
}

fn pool(r: &mut Rng) -> ManagerPool {
    ManagerPool {
        connected: r.bool(),
        accepted: r.u64(),
        rejected: r.u64(),
        best_session: r.u64(),
        network_difficulty: r.f64(),
        pool_difficulty: r.next() as u32,
        pool_diff_err: r.bool(),
        ping_rtt: r.f64(),
        ping_loss: r.f64(),
        verify_blocked: pick(r),
        protocol: r.below(3) as c_int,
        encrypted: r.bool(),
    }
}

fn fan(r: &mut Rng) -> FanInfo {
    FanInfo {
        label: pick(r),
        mode: [0, 2, 3, 1][r.below(4) as usize],
        manual_speed: r.below(101) as u16,
        overheat_temp: r.below(120) as u16,
        rpm: r.next() as u16,
        speed_perc: r.below(101) as u16,
        target_temp: r.below(90) as u16,
        p: r.next() as u16,
        i: r.next() as u16,
        d: r.next() as u16,
    }
}

fn snapshot(r: &mut Rng, temps: &mut Vec<f32>) -> SystemInfo {
    let asic_count = r.below(13) as c_int;
    *temps = (0..asic_count).map(|_| r.f32()).collect();
    let mode = r.below(3) as c_int;
    let manager = ManagerInfo {
        config_pool_mode: r.below(3) as u16,
        pool_mode: mode,
        pool_balance: r.below(101) as u16,
        total_best_diff: r.u64(),
        fallback: mode == 0,
        using_fallback: r.bool(),
        dual: mode == 1,
        selected: r.below(2) as c_int,
        pools: [pool(r), pool(r)],
        accepted: r.u64(),
        rejected: r.u64(),
        best_session: r.u64(),
        network_difficulty: r.f64(),
        pool_difficulty: r.next() as u32,
    };
    let fans = [fan(r), fan(r)];
    let pid = (fans[0].target_temp, fans[0].p, fans[0].i, fans[0].d);
    SystemInfo {
        asic_count,
        small_core_count: if r.bool() { 0 } else { r.below(3000) as c_int },
        device_model: pick(r),
        hostip: pick(r),
        mac_addr: pick(r),
        wifi_rssi: -(r.below(100) as c_int),
        power: r.f32(),
        max_power: r.f32(),
        min_power: r.f32(),
        max_voltage: r.f32(),
        min_voltage: r.f32(),
        current: r.f32(),
        min_current_a: r.f32(),
        max_current_a: r.f32(),
        temp: r.f32(),
        vr_temp: r.f32(),
        vr_temp_int: r.f32(),
        hashrate_timestamp: r.u64(),
        shutdown: r.below(5) == 0,
        hashrate: r.f32(),
        hashrate_1m: r.f64(),
        hashrate_10m: r.f64(),
        hashrate_1h: r.f64(),
        hashrate_1d: r.f64(),
        core_voltage: r.below(1400) as c_int,
        default_core_voltage: r.below(1400) as c_int,
        // finite: (int) of NaN/huge floats is undefined in C (Rust: saturating)
        vout: r.below(3000) as f32 / 1000.0 + r.below(1000) as f32 / 1e6,
        manual_fan_speed: r.below(101) as u16,
        fan_count: r.below(3) as c_int,
        fans,
        last_ping_rtt: r.f64(),
        recent_ping_loss: r.f64(),
        duplicate_hw_nonces: r.u64(),
        pool_difficulty: r.next() as u32,
        network_difficulty: r.f64(),
        found_blocks: r.next() as u32,
        total_found_blocks: r.next() as u32,
        shares_accepted: r.u64(),
        shares_rejected: r.u64(),
        // the same getter as the manager's totalBestDiff
        best_diff: manager.total_best_diff,
        best_session_diff: r.u64(),
        manager,
        asic_temps: temps.clone(),
        history_requested: r.below(3) == 0,
        history_start: 0,
        history_span_ms: 0,
        history_current: 0,
        history_limit: 0,
        now_ms: 0,
        pid_available: r.bool(),
        // getPidSettings(0) is also fan 0's settings
        pid_target_temp: pid.0,
        pid_p: pid.1,
        pid_i: pid.2,
        pid_d: pid.3,
        hostname: pick(r),
        ssid: pick(r),
        stratum_url: pick(r),
        stratum_port: r.next() as u16,
        stratum_user: pick(r),
        stratum_enonce_subscribe: r.bool(),
        stratum_tls: r.bool(),
        fallback_stratum_url: pick(r),
        fallback_stratum_port: r.next() as u16,
        fallback_stratum_user: pick(r),
        fallback_stratum_enonce_subscribe: r.bool(),
        fallback_stratum_tls: r.bool(),
        stratum_protocol: r.below(3) as u16,
        fallback_stratum_protocol: r.below(3) as u16,
        sv2_authority_pubkey: pick(r),
        fallback_sv2_authority_pubkey: pick(r),
        sv2_channel_type: r.below(3) as u16,
        fallback_sv2_channel_type: r.below(3) as u16,
        voltage: r.f32(),
        frequency: r.below(1000) as c_int,
        default_frequency: r.below(1000) as c_int,
        job_interval: r.below(2000) as c_int,
        stratum_difficulty: r.next() as u32,
        overheat_temp: r.below(120) as u16,
        flip_screen: r.bool(),
        invert_screen: r.bool(),
        auto_screen_off: r.bool(),
        auto_screen_off_timeout: 30,
        invert_fan_polarity: r.bool(),
        stratum_keep: r.bool(),
        vr_frequency: r.next() as u32,
        default_vr_frequency: r.next() as u32,
        otp: r.bool(),
        asic_model: pick(r),
        uptime_seconds: r.below(1 << 40) as i64,
        last_reset_reason: pick(r),
        wifi_status: pick(r),
        free_heap: r.u64() & 0xffff_ffff,
        free_heap_int: r.u64() & 0xffff_ffff,
        version: pick(r),
        running_partition: pick(r),
        default_theme: pick(r),
    }
}

#[test]
fn system_info() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x5157);
    let mut rec = Rec::new("http_json", "system_info");
    // a history with samples: 1 h of 5 s samples ending at 10 h
    assert!(history_init(4));
    for k in 0..720u64 {
        let ts = 36_000_000 - (720 - k) * 5000;
        history_push(r.below(20_000) as f32, r.f32(), r.f32(), ts);
    }
    let mut checked_history = 0;
    for _ in 0..4000 {
        let mut temps = Vec::new();
        let mut s = snapshot(&mut r, &mut temps);
        // the history window in client time
        s.now_ms = 36_000_000 + r.below(10_000);
        s.history_current = 1_700_000_000_000 + r.below(1_000_000);
        s.history_start = s.history_current - r.below(4_000_000);
        s.history_span_ms = r.below(4_000_000);
        s.history_limit = [0u32, 1, 10, 100, 2000][r.below(5) as usize];
        let rust = String::from_utf8(http_json::system_info(&s).serialize()).unwrap();
        rec.add(format!(
            "start {} cur {} span {} limit {} requested {} now {}\n{rust}",
            s.history_start,
            s.history_current,
            s.history_span_ms,
            s.history_limit,
            s.history_requested,
            s.now_ms
        ));
        if rust.contains("\"history\":{\"hashrate_1m\":[") && !rust.contains("\"hashrate_1m\":[]") {
            checked_history += 1;
        }
    }
    // a NaN output voltage: 0 instead of the undefined (int) cast
    let mut temps = Vec::new();
    let mut s = snapshot(&mut r, &mut temps);
    s.vout = f32::NAN;
    let rust = String::from_utf8(http_json::system_info(&s).serialize()).unwrap();
    assert!(rust.contains("\"coreVoltageActual\":0,"));
    // not vacuous: histories with rows were compared
    assert!(checked_history > 100, "{checked_history}");
    rec.finish();
}

#[test]
fn asic_info() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xa51c);
    let mut rec = Rec::new("http_json", "asic_info");
    for _ in 0..500 {
        let freqs: Vec<u32> = (0..r.below(25)).map(|_| r.next() as u32 % 2000).collect();
        let volts: Vec<u32> = (0..r.below(25)).map(|_| r.next() as u32 % 2000).collect();
        let a = AsicInfo {
            asic_model: pick(&mut r),
            device_model: pick(&mut r),
            asic_count: r.below(13) as c_int,
            default_frequency: r.below(1000) as c_int,
            default_voltage: r.below(1400) as c_int,
            abs_max_frequency: r.below(1000) as c_int,
            abs_min_voltage: r.below(1400) as c_int,
            abs_max_voltage: r.below(1400) as c_int,
            eco_frequency: r.below(1000) as c_int,
            eco_voltage: r.below(1400) as c_int,
            swarm_color: pick(&mut r),
            frequency_options: freqs.clone(),
            voltage_options: volts.clone(),
        };
        let rust = String::from_utf8(http_json::asic_info(&a).serialize()).unwrap();
        rec.add(rust);
    }
    rec.finish();
}

/// The history used by the tests with history exports.
fn with_history(r: &mut Rng) {
    static INIT: std::sync::Once = std::sync::Once::new();
    INIT.call_once(|| {
        assert!(history_init(4));
        let mut r = Rng(0x415);
        for k in 0..720u64 {
            let ts = 36_000_000 - (720 - k) * 5000;
            history_push(r.below(20_000) as f32, r.f32(), r.f32(), ts);
        }
    });
    let _ = r;
}

#[test]
fn dashboard() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xda5b);
    let mut rec = Rec::new("http_json", "dashboard");
    with_history(&mut r);
    let mut seen = [0u32; 3];
    for _ in 0..4000 {
        let mut temps = Vec::new();
        // the manager part and the shared values from the system snapshot
        let s = snapshot(&mut r, &mut temps);
        let n = s.asic_count;
        let rates: Vec<f32> = (0..n).map(|_| r.f32()).collect();
        let errors: Vec<f32> = (0..n).map(|_| r.f32()).collect();
        let coinbase = |r: &mut Rng| {
            // never NULL: a char array in coinbase_result_t
            let script = pick(r).unwrap_or_default();
            DashCoinbase {
                block_height: if r.bool() { 0 } else { r.next() as u32 },
                network_difficulty: r.f64(),
                scriptsig: script,
                total_value_satoshis: r.u64(),
                user_value_satoshis: r.u64(),
                verification_ok: r.bool(),
                fail_count: r.next() as u32,
                check_count: r.next() as u32,
                verify_mode: r.below(3) as u16,
                max_fee: r.next() as u16,
                force: r.bool(),
            }
        };
        let cb = [coinbase(&mut r), coinbase(&mut r)];
        let d = Dashboard {
            uptime_seconds: s.uptime_seconds,
            shutdown: s.shutdown,
            board_error: r.below(7) as c_int,
            overheat_temp: s.overheat_temp,
            vr_overheat_temp: r.below(120) as u16,
            hashrate_timestamp: s.hashrate_timestamp,
            hashrate: s.hashrate,
            hashrate_1m: s.hashrate_1m,
            hashrate_10m: s.hashrate_10m,
            hashrate_1h: s.hashrate_1h,
            hashrate_1d: s.hashrate_1d,
            // the same getter as the manager's totalBestDiff
            best_diff: s.manager.total_best_diff,
            best_session_diff: s.best_session_diff,
            shares_accepted: s.shares_accepted,
            shares_rejected: s.shares_rejected,
            frequency: s.frequency,
            asic_count: n,
            small_core_count: s.small_core_count,
            chip_hashrate: rates,
            chip_error_rate: errors,
            asic_temps: temps,
            watts: s.power,
            min_power: s.min_power,
            max_power: s.max_power,
            voltage: s.voltage,
            min_voltage: s.min_voltage,
            max_voltage: s.max_voltage,
            current: s.current,
            min_current_a: s.min_current_a,
            max_current_a: s.max_current_a,
            vout: r.f32(),
            asic_temp: s.temp,
            vr_temp: s.vr_temp,
            vr_temp_int: s.vr_temp_int,
            fan_count: s.fan_count,
            fan_speed: [s.fans[0].speed_perc, s.fans[1].speed_perc],
            fan_rpm: [s.fans[0].rpm, s.fans[1].rpm],
            manager: s.manager,
            pool_host: [pick(&mut r), pick(&mut r)],
            pool_port: [r.next() as u16 as c_int, r.next() as u16 as c_int],
            pool_user: [pick(&mut r), pick(&mut r)],
            has_can_extension: r.bool(),
            can_enabled: r.bool(),
            coinbase: cb,
            history_requested: r.below(3) == 0,
            history_start: 0,
            history_span_ms: 0,
            history_current: 0,
            history_limit: 0,
            now_ms: 36_000_000 + r.below(10_000),
        };
        let mut d = d;
        d.history_current = 1_700_000_000_000 + r.below(1_000_000);
        d.history_start = d.history_current - r.below(4_000_000);
        d.history_span_ms = r.below(4_000_000);
        d.history_limit = [0u32, 1, 10, 100, 2000][r.below(5) as usize];
        let rust = String::from_utf8(http_json::dashboard(&d).serialize()).unwrap();
        rec.add(format!(
            "start {} cur {} span {} limit {} requested {} now {}\n{rust}",
            d.history_start,
            d.history_current,
            d.history_span_ms,
            d.history_limit,
            d.history_requested,
            d.now_ms
        ));
        seen[d.manager.pool_mode as usize] += 1;
    }
    assert!(seen.iter().all(|&n| n > 1000), "{seen:?}");
    rec.finish();
}

#[test]
fn v2_system_and_identify() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x2525);
    let mut rec = Rec::new("http_json", "v2_system_and_identify");
    for _ in 0..1000 {
        let v = V2System {
            device_model: pick(&mut r),
            asic_model: pick(&mut r),
            version: pick(&mut r),
            uptime_seconds: r.below(1 << 40) as i64,
            last_reset_reason: pick(&mut r),
            hostname: pick(&mut r),
            ssid: pick(&mut r),
            mac_addr: pick(&mut r),
            ip_addr: pick(&mut r),
            wifi_status: pick(&mut r),
            wifi_rssi: -(r.below(100) as c_int),
            free_heap: r.u64() & 0xffff_ffff,
            free_heap_int: r.u64() & 0xffff_ffff,
        };
        let rust = String::from_utf8(http_json::v2_system(&v).serialize()).unwrap();
        rec.add(rust);
        let i = V2Identify {
            device_model: pick(&mut r),
            default_theme: pick(&mut r),
            otp: r.bool(),
            ap_active: r.bool(),
            // the former code was recorded with the core dump download
            core_dump_download: true,
            can_enabled: r.bool(),
        };
        let rust = String::from_utf8(http_json::v2_identify(&i).serialize()).unwrap();
        rec.add(rust);
        let without = V2Identify {
            core_dump_download: false,
            ..i
        };
        let rust = String::from_utf8(http_json::v2_identify(&without).serialize()).unwrap();
        assert!(rust.contains("\"coreDumpDownload\":false,"));
    }
    rec.finish();
}

#[test]
fn small_documents() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x5a11);
    let mut rec = Rec::new("http_json", "small_documents");
    for _ in 0..2000 {
        // the C++ side reports hasWebhook from the webhook string
        let a = AlertInfo {
            has_webhook: r.bool(),
            watchdog: r.bool(),
            block_found: r.bool(),
            best_diff: r.bool(),
            coinbase_verify: r.bool(),
            show_block_found_screen: r.bool(),
        };
        let rust = String::from_utf8(http_json::alert_info(&a).serialize()).unwrap();
        rec.add(rust);

        let i = InfluxInfo {
            url: pick(&mut r),
            port: r.next() as u16,
            bucket: pick(&mut r),
            org: pick(&mut r),
            prefix: pick(&mut r),
            enabled: r.bool(),
        };
        let rust = String::from_utf8(http_json::influx_info(&i).serialize()).unwrap();
        rec.add(rust);

        let e = r.bool();
        let rust = String::from_utf8(http_json::otp_status(e).serialize()).unwrap();
        rec.add(rust);

        let token = CString::new(format!("tok.{:x}.\"\\{}", r.next(), r.below(1000))).unwrap();
        let ttl = [24 * 3600, 1, 0, u32::MAX, r.next() as u32][r.below(5) as usize];
        let expires = [1_794_940_377_000u64, 0, 12_345, r.next()][r.below(4) as usize];
        let rust =
            String::from_utf8(http_json::otp_session(token.as_bytes(), ttl, expires).serialize())
                .unwrap();
        rec.add(rust);

        // ssids are char[33] in the scan records: never NULL, at most 32 bytes
        let nets: Vec<WifiNetwork> = (0..r.below(21))
            .map(|_| WifiNetwork {
                ssid: pick(&mut r).unwrap_or_default(),
                rssi: r.next() as i8,
                authmode: r.below(12) as c_int,
            })
            .collect();
        let rust = String::from_utf8(http_json::wifi_scan(&nets).serialize()).unwrap();
        rec.add(rust);
    }
    rec.finish();
}

#[test]
fn v2_settings() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0x5e77);
    let mut rec = Rec::new("http_json", "v2_settings");
    for _ in 0..3000 {
        let freqs: Vec<u32> = (0..r.below(25)).map(|_| r.next() as u32 % 2000).collect();
        let volts: Vec<u32> = (0..r.below(25)).map(|_| r.next() as u32 % 2000).collect();
        let pool = |r: &mut Rng| SettingsPool {
            url: pick(r),
            port: r.next() as u16,
            user: pick(r),
            enonce_subscribe: r.bool(),
            tls: r.bool(),
            protocol: r.below(3) as u16,
            sv2_authority_pubkey: pick(r),
            sv2_channel_type: r.below(3) as u16,
            sv2_require_auth: r.bool(),
            coinbase_verify_mode: r.below(3) as u16,
            coinbase_max_fee: r.next() as u16,
            coinbase_verify_force: r.bool(),
        };
        let pools = [pool(&mut r), pool(&mut r)];
        let mut s = V2Settings {
            asic_model: pick(&mut r),
            device_model: pick(&mut r),
            version: pick(&mut r),
            otp: r.bool(),
            ap_active: r.bool(),
            has_can_extension: r.bool(),
            can_enabled: r.bool(),
            frequency: r.below(1000) as c_int,
            core_voltage: r.below(1400) as c_int,
            vr_frequency: r.next() as u32,
            default_frequency: r.below(1000) as c_int,
            default_core_voltage: r.below(1400) as c_int,
            default_vr_frequency: r.next() as u32,
            eco_frequency: r.below(1000) as c_int,
            eco_core_voltage: r.below(1400) as c_int,
            abs_min_core_voltage: r.below(1400) as c_int,
            abs_max_core_voltage: r.below(1400) as c_int,
            frequency_options: freqs.clone(),
            voltage_options: volts.clone(),
            pool_mode: r.below(3) as u16,
            pool_balance: r.below(101) as u16,
            stratum_keep: r.bool(),
            job_interval: r.below(2000) as c_int,
            stratum_difficulty: r.next() as u32,
            pools,
            pid_available: r.bool(),
            fan_count: r.below(3) as c_int,
            fans: [fan(&mut r), fan(&mut r)],
            invert_fan_polarity: r.bool(),
            has_pid_use_max: false,
            pid_use_max: r.bool(),
            hostname: pick(&mut r),
            ssid: pick(&mut r),
            mempool_custom: r.bool(),
            mempool_url: pick(&mut r),
            flip_screen: r.bool(),
            invert_screen: r.bool(),
            auto_screen_off: r.bool(),
            auto_screen_off_timeout: 30,
        };
        let rust = String::from_utf8(http_json::v2_settings(&s).serialize()).unwrap();
        rec.add(rust);
        s.has_pid_use_max = true;
        let rust = String::from_utf8(http_json::v2_settings(&s).serialize()).unwrap();
        rec.add(format!("NerdAxe\n{rust}"));
        assert!(rust.contains("\"pidUseMax\":"));
    }
    rec.finish();
}

/// A multi-frame message (7 payload bytes per frame, the last one SEQ_LAST).
fn send_message(base: u32, slave: u8, msg: &[u8], now: i64) {
    let chunks: Vec<&[u8]> = msg.chunks(7).collect();
    for (k, chunk) in chunks.iter().enumerate() {
        let mut data = [0u8; 8];
        data[0] = if k + 1 == chunks.len() { 0xff } else { k as u8 };
        data[1..1 + chunk.len()].copy_from_slice(chunk);
        let _ = can_master_frame(base + slave as u32, 1 + chunk.len() as u8, &data, now);
    }
}

#[test]
fn can_swarm() {
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut r = Rng(0xca25);
    let mut rec = Rec::new("http_json", "can_swarm");
    let mut slaves_seen = 0;
    for round in 0..1500 {
        let now = 1_000_000_000 + round as i64 * 100_000_000;
        {
            for i in 1..32 {
                can_master_delete(i);
            }
            let mut blob = [0u8; 32 * 7];
            for i in 1..32 {
                if r.below(4) == 0 {
                    for b in &mut blob[i * 7..i * 7 + 6] {
                        *b = r.next() as u8;
                    }
                    blob[i * 7 + 6] = 1;
                }
            }
            can_master_load_registry(&blob);
            for i in 1..32u8 {
                if blob[i as usize * 7 + 6] == 0 {
                    continue;
                }
                slaves_seen += 1;
                if r.below(4) != 0 {
                    let mut t = [0u8; 49];
                    for b in t.iter_mut() {
                        *b = r.next() as u8;
                    }
                    // mostly sensible floats, sometimes raw bits
                    for at in [1usize, 5, 9, 13, 17, 21, 31, 39] {
                        if r.below(4) != 0 {
                            let v = r.f32();
                            t[at..at + 4].copy_from_slice(&v.to_le_bytes());
                        }
                    }
                    t[43] = (r.below(3) == 0) as u8;
                    send_message(0x400, i, &t, now - r.below(20_000_000) as i64);
                }
                if r.below(3) != 0 {
                    let mut c = [0u8; 46];
                    for b in c.iter_mut() {
                        *b = r.next() as u8;
                    }
                    // model/firmware strings, sometimes without a NUL
                    let model = b"NerdQAxe++\0";
                    if r.bool() {
                        c[..model.len()].copy_from_slice(model);
                    }
                    send_message(0x700, i, &c, now);
                }
            }
            // slaves without telemetry for 10 s go offline
            can_master_tick(now);
        }
        let temps: Vec<f32> = (0..r.below(13)).map(|_| r.f32()).collect();
        let fans = [fan(&mut r), fan(&mut r)];
        let m = CanMasterNode {
            mac: pick(&mut r),
            device_model: pick(&mut r),
            version: pick(&mut r),
            shutdown: r.below(5) == 0,
            hashrate: r.f32(),
            power: r.f32(),
            temp: r.f32(),
            vr_temp: r.f32(),
            // finite: (int) of NaN/huge floats is undefined in C
            current: r.below(20_000_000) as f32 / 1000.0,
            vout: r.below(3000) as f32 / 1000.0,
            board_error: r.below(7) as c_int,
            free_heap_int: r.below(1 << 30) as c_int,
            frequency: r.below(1000) as c_int,
            core_voltage: r.below(1400) as c_int,
            flip_screen: r.bool(),
            auto_screen_off: r.bool(),
            asic_temps: temps,
            fan_count: r.below(3) as c_int,
            fans,
        };
        // raw bytes: the slaves' model strings may be any bytes
        let rust = http_json::can_swarm(&m).serialize();
        rec.add(text_or_bytes(rust));
    }
    assert!(slaves_seen > 5000, "{slaves_seen}");
    rec.finish();
}

/// The document as text, or escaped if it is not UTF-8.
fn text_or_bytes(doc: Vec<u8>) -> String {
    String::from_utf8(doc).unwrap_or_else(|e| format!("bytes: {}", e.into_bytes().escape_ascii()))
}
