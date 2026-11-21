//! InfluxDB v2 telemetry (port of `components/influx/influx.cpp` and
//! `main/tasks/influx_task.cpp`): the power, temperature and fan samples
//! are averaged between writes; every 15 s one line-protocol record goes
//! to the server, after the bucket is checked (or created) and the last
//! totals are loaded. The HTTP requests are a C++ hook.
//!
//! Behaviour notes:
//! - bucket, org and prefix (from the settings) are escaped for the URL,
//!   the JSON bodies, the Flux string and the line protocol, so a space,
//!   comma, quote or `&` in them is sent literally;
//! - the telemetry setters lock a mutex that exists from the start, so the
//!   power task may call them before `init()`;
//! - found blocks are counted by the increase of the session count, so
//!   every block of a session is counted;
//! - share counters are 64-bit.

use core::ffi::{c_int, CStr};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::ajson;
use crate::cutil::{fixed, json_escape, url_encode};
use crate::log::log;

const TAG: &CStr = c"InfluxDB";
const TASK_TAG: &CStr = c"influx_task";
const PERIOD: Duration = Duration::from_secs(15);

#[derive(Clone, Default, Debug)]
pub struct Settings {
    pub enabled: bool,
    /// with the scheme, e.g. `http://192.168.0.10`
    pub url: String,
    pub port: u16,
    pub token: String,
    pub bucket: String,
    pub org: String,
    pub prefix: String,
}

/// The values of one record (`Stats`).
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Stats {
    pub temp: f32,
    pub temp2: f32,
    pub hashing_speed: f32,
    pub hashing_speed_1m: f32,
    pub difficulty: i64,
    pub best_difficulty: f32,
    pub pool_errors: i64,
    pub accepted: i64,
    pub not_accepted: i64,
    pub total_uptime: i64,
    pub total_best_difficulty: f32,
    pub uptime: i64,
    pub blocks_found: i64,
    pub total_blocks_found: i64,
    pub duplicate_hashes: i64,
    pub pwr: [f32; 6],
    pub fan_pwm: [f32; 2],
    pub fan_rpm: [f32; 2],
    pub last_ping_rtt: f32,
    pub recent_ping_loss: f32,
}

/// Samples since the last write.
#[derive(Default)]
struct Accum {
    pwr: [f64; 6],
    temp: [f64; 2],
    fan: [f64; 4],
    pwr_count: u32,
    temp_count: u32,
    fan_count: u32,
}

static ACCUM: Mutex<Option<Accum>> = Mutex::new(None);

fn accum<T>(f: impl FnOnce(&mut Accum) -> T) -> T {
    f(ACCUM
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_or_insert_with(Accum::default))
}

/// Averages the samples into `s`; false if there were none. Values without
/// samples keep their previous value.
fn flush(s: &mut Stats) -> bool {
    accum(|a| {
        let have = a.pwr_count + a.temp_count + a.fan_count > 0;
        if a.pwr_count > 0 {
            let n = a.pwr_count as f64;
            for (dst, sum) in s.pwr.iter_mut().zip(a.pwr) {
                *dst = (sum / n) as f32;
            }
        }
        if a.temp_count > 0 {
            let n = a.temp_count as f64;
            s.temp = (a.temp[0] / n) as f32;
            s.temp2 = (a.temp[1] / n) as f32;
        }
        if a.fan_count > 0 {
            let n = a.fan_count as f64;
            s.fan_pwm = [(a.fan[0] / n) as f32, (a.fan[2] / n) as f32];
            s.fan_rpm = [(a.fan[1] / n) as f32, (a.fan[3] / n) as f32];
        }
        *a = Accum::default();
        have
    })
}

/// The record in line protocol (the C++ format; floats as `%f`).
pub fn line(prefix: &str, s: &Stats) -> String {
    let f = |x: f32| fixed(x as f64, 6);
    let measurement: String = prefix
        .chars()
        .flat_map(|c| match c {
            ',' | ' ' | '\\' => vec!['\\', c],
            c => vec![c],
        })
        .collect();
    format!(
        "{measurement} temperature={},temperature2={},\
         hashing_speed={},hashing_speed_1m={},invalid_shares=0,valid_shares=0,uptime={},\
         best_difficulty={},total_best_difficulty={},pool_errors={},\
         accepted={},not_accepted={},total_uptime={},blocks_found={},\
         pwr_vin={},pwr_iin={},pwr_pin={},pwr_vout={},pwr_iout={},pwr_pout={},\
         total_blocks_found={},duplicate_hashes={},last_ping_rtt={},recent_ping_loss={},\
         fan0_pwm={},fan0_rpm={},fan1_pwm={},fan1_rpm={}",
        f(s.temp),
        f(s.temp2),
        f(s.hashing_speed),
        f(s.hashing_speed_1m),
        s.uptime,
        f(s.best_difficulty),
        f(s.total_best_difficulty),
        s.pool_errors,
        s.accepted,
        s.not_accepted,
        s.total_uptime,
        s.blocks_found,
        f(s.pwr[0]),
        f(s.pwr[1]),
        f(s.pwr[2]),
        f(s.pwr[3]),
        f(s.pwr[4]),
        f(s.pwr[5]),
        s.total_blocks_found,
        s.duplicate_hashes,
        fixed(s.last_ping_rtt as f64, 2),
        fixed(s.recent_ping_loss as f64, 2),
        f(s.fan_pwm[0]),
        f(s.fan_rpm[0]),
        f(s.fan_pwm[1]),
        f(s.fan_rpm[1]),
    )
}

/// A string literal of the Flux language.
fn flux_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        if matches!(c, '"' | '\\') {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// The totals the server has from earlier runs: the `_value`/`_field`
/// columns of the CSV answer to the `last()` query (parsed like the C++
/// `strtok()` code: empty fields are skipped, the first line is the header).
pub fn parse_last_values(csv: &[u8], s: &mut Stats) -> bool {
    let text = String::from_utf8_lossy(csv);
    let mut lines = text.split(['\r', '\n']).filter(|l| !l.is_empty());
    lines.next(); // header
    for l in lines {
        let fields: Vec<&str> = l.split(',').filter(|t| !t.is_empty()).collect();
        let (Some(value), Some(field)) = (fields.get(5), fields.get(6)) else {
            log!(Error, TAG, "Failed to parse _value");
            return false;
        };
        let v = atof(value);
        match *field {
            "total_uptime" => s.total_uptime = v as i32 as i64,
            "total_best_difficulty" => s.total_best_difficulty = v as f32,
            "total_blocks_found" => s.total_blocks_found = v as i32 as i64,
            _ => {}
        }
    }
    true
}

/// `atof()`: the longest leading number, 0 if none.
fn atof(s: &str) -> f64 {
    let s = s.trim_start();
    let mut end = 0;
    for (i, _) in s.char_indices().chain([(s.len(), ' ')]) {
        if s[..i].parse::<f64>().is_ok() {
            end = i;
        }
    }
    s[..end].parse().unwrap_or(0.0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Method {
    Get = 0,
    Post = 1,
}

/// An HTTP answer: status (< 0: no answer) and body.
pub struct Response {
    pub status: c_int,
    pub body: Vec<u8>,
}

/// What the client needs from the firmware.
pub trait Env {
    #[allow(clippy::too_many_arguments)]
    fn request(
        &mut self,
        method: Method,
        url: &str,
        auth: Option<&str>,
        content_type: Option<&str>,
        accept: Option<&str>,
        body: &[u8],
    ) -> Response;
}

pub struct Client {
    s: Settings,
    auth: String,
}

impl Client {
    pub fn new(s: Settings) -> Self {
        let auth = format!("Token {}", s.token);
        Self { s, auth }
    }

    fn url(&self, path: &str) -> String {
        format!("{}:{}{}", self.s.url, self.s.port, path)
    }

    pub fn ping(&self, env: &mut impl Env) -> bool {
        let url = self.url("/ping");
        log!(Info, TAG, "URL: {url}");
        let r = env.request(Method::Get, &url, None, None, None, &[]);
        match r.status {
            // 204: the standard answer, 200: some proxies
            200 | 204 => {
                log!(
                    Info,
                    TAG,
                    "Successfully connected to InfluxDB at {}:{}",
                    self.s.url,
                    self.s.port
                );
                true
            }
            s if s < 0 => false,
            s => {
                log!(Error, TAG, "InfluxDB ping failed with status code: {s}");
                false
            }
        }
    }

    pub fn bucket_exists(&self, env: &mut impl Env) -> bool {
        let url = self.url(&format!(
            "/api/v2/buckets?org={}&name={}",
            url_encode(&self.s.org),
            url_encode(&self.s.bucket)
        ));
        log!(Info, TAG, "URL: {url}");
        let r = env.request(Method::Get, &url, Some(&self.auth), None, None, &[]);
        match r.status {
            200 => match ajson::deserialize(&r.body) {
                Ok(doc) if !doc.get("buckets").is_empty() => true,
                Ok(_) => {
                    log!(Warn, TAG, "Bucket not found");
                    false
                }
                Err(_) => {
                    log!(Error, TAG, "Failed to parse JSON response");
                    false
                }
            },
            404 => {
                log!(Info, TAG, "Bucket list request error");
                false
            }
            s if s < 0 => false,
            s => {
                log!(Error, TAG, "HTTP GET failed with status code: {s}");
                false
            }
        }
    }

    fn org_id(&self, env: &mut impl Env) -> Option<String> {
        let url = self.url(&format!("/api/v2/orgs?org={}", url_encode(&self.s.org)));
        log!(Info, TAG, "Looking up orgID via: {url}");
        let r = env.request(
            Method::Get,
            &url,
            Some(&self.auth),
            None,
            Some("application/json"),
            &[],
        );
        if r.status < 0 || r.body.is_empty() {
            log!(Error, TAG, "Failed to read response");
            return None;
        }
        log!(
            Info,
            TAG,
            "Org lookup response: {}",
            String::from_utf8_lossy(&r.body)
        );
        let doc = ajson::deserialize(&r.body).ok()?;
        let id = doc.get("orgs").at(0).get("id").as_cstr()?;
        let id = String::from_utf8_lossy(id).into_owned();
        log!(Info, TAG, "Resolved orgID: {id}");
        Some(id)
    }

    pub fn create_bucket(&self, env: &mut impl Env) -> bool {
        let url = self.url("/api/v2/buckets");
        log!(Info, TAG, "Creating bucket at URL: {url}");
        let Some(org_id) = self.org_id(env) else {
            log!(Error, TAG, "Could not resolve org ID");
            return false;
        };
        let body = format!(
            "{{\"orgID\": \"{}\", \"name\": \"{}\", \"description\": \"Auto-created\", \"retentionRules\": [{{\"type\": \"expire\", \"everySeconds\": 2592000}}]}}",
            json_escape(&org_id),
            json_escape(&self.s.bucket)
        );
        let r = env.request(
            Method::Post,
            &url,
            Some(&self.auth),
            Some("application/json"),
            None,
            body.as_bytes(),
        );
        if r.status == 201 {
            log!(Info, TAG, "Bucket created successfully.");
            true
        } else {
            log!(Error, TAG, "Failed to create bucket, status: {}", r.status);
            false
        }
    }

    pub fn load_last_values(&self, env: &mut impl Env, s: &mut Stats) -> bool {
        let url = self.url(&format!("/api/v2/query?org={}", url_encode(&self.s.org)));
        log!(Info, TAG, "URL: {url}");
        let flux = format!(
            "from(bucket:{}) |> range(start:-1y) |> filter(fn:(r) => r._measurement == {}) |> last()",
            flux_string(&self.s.bucket),
            flux_string(&self.s.prefix)
        );
        let query = format!("{{\"query\":\"{}\"}}", json_escape(&flux));
        log!(Info, TAG, "Query JSON: {query}");
        let r = env.request(
            Method::Post,
            &url,
            Some(&self.auth),
            Some("application/json"),
            Some("text/csv"),
            query.as_bytes(),
        );
        if r.status < 0 {
            return false;
        }
        if r.body.is_empty() {
            log!(Warn, TAG, "Received empty response");
            return false;
        }
        if r.status != 200 {
            log!(
                Error,
                TAG,
                "Failed to load last values, HTTP status: {}",
                r.status
            );
            return false;
        }
        let ok = parse_last_values(&r.body, s);
        if ok {
            log!(Info, TAG, "Loaded last values from InfluxDB");
        }
        ok
    }

    pub fn write(&self, env: &mut impl Env, s: &Stats) {
        let url = self.url(&format!(
            "/api/v2/write?bucket={}&org={}&precision=s",
            url_encode(&self.s.bucket),
            url_encode(&self.s.org)
        ));
        let body = line(&self.s.prefix, s);
        log!(Info, TAG, "URL: {url}");
        log!(Info, TAG, "POST: {body}");
        let r = env.request(
            Method::Post,
            &url,
            Some(&self.auth),
            Some("text/plain"),
            None,
            body.as_bytes(),
        );
        if r.status >= 0 {
            log!(Info, TAG, "HTTP POST Status = {}", r.status);
        }
        if r.status == 400 {
            log!(
                Error,
                TAG,
                "HTTP POST Error 400 Response: {}",
                String::from_utf8_lossy(&r.body)
            );
        }
    }
}

/// The values the task takes from the system and the pool manager.
#[derive(Clone, Copy, Default, Debug)]
pub struct Inputs {
    pub hashrate: f32,
    pub hashrate_1m: f32,
    pub best_session_diff: u64,
    pub best_diff: u64,
    pub accepted: u64,
    pub rejected: u64,
    pub duplicates: u64,
    pub pool_errors: c_int,
    pub pool_difficulty: u32,
    pub found_blocks: u32,
    pub last_ping_rtt: f64,
    pub recent_ping_loss: f64,
}

/// The task's record between writes.
#[derive(Default)]
pub struct Record {
    pub stats: Stats,
    last_found: u32,
    start: Option<(Instant, i64)>,
}

impl Record {
    /// Takes the inputs and the averaged samples; false without samples.
    pub fn update(&mut self, i: &Inputs, now: Instant) -> bool {
        let s = &mut self.stats;
        s.hashing_speed = i.hashrate;
        s.hashing_speed_1m = i.hashrate_1m;
        s.last_ping_rtt = i.last_ping_rtt as f32;
        s.recent_ping_loss = i.recent_ping_loss as f32;
        s.best_difficulty = i.best_session_diff as f32;
        s.total_best_difficulty = i.best_diff as f32;
        s.accepted = i.accepted as i64;
        s.not_accepted = i.rejected as i64;
        s.duplicate_hashes = i.duplicates as i64;
        s.pool_errors = i.pool_errors as i64;
        s.difficulty = i.pool_difficulty as i64;
        // count every block of the session, not only the first
        if i.found_blocks > self.last_found {
            let n = (i.found_blocks - self.last_found) as i64;
            s.blocks_found += n;
            s.total_blocks_found += n;
        }
        self.last_found = i.found_blocks;
        // uptime counts from loading the last values
        if let Some((t0, total0)) = self.start {
            let up = now.saturating_duration_since(t0).as_secs() as i64;
            s.uptime = up;
            s.total_uptime = total0 + up;
        }
        flush(s)
    }

    pub fn start_uptime(&mut self, now: Instant) {
        self.start = Some((now, self.stats.total_uptime));
    }
}

/// What the task needs from the firmware.
pub trait TaskEnv: Env {
    fn suspend_if_shutdown(&mut self);
    fn inputs(&mut self) -> Inputs;
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

fn halt(env: &mut impl TaskEnv) -> ! {
    log!(Info, TASK_TAG, "halting influx_task");
    loop {
        env.sleep(PERIOD);
    }
}

/// The task (`influx_task()`); never returns.
pub fn run(settings: Settings, env: &mut impl TaskEnv) -> ! {
    if !settings.enabled {
        log!(Info, TASK_TAG, "InfluxDB is not enabled.");
        halt(env);
    }
    log!(
        Info,
        TASK_TAG,
        "URL: {}, port: {}, bucket: {}, org: {}, prefix: {}",
        settings.url,
        settings.port,
        settings.bucket,
        settings.org,
        settings.prefix
    );
    let client = Client::new(settings);
    let mut rec = Record::default();

    let (mut ping_ok, mut bucket_ok) = (false, false);
    loop {
        ping_ok = ping_ok || client.ping(env);
        if !ping_ok {
            log!(Error, TASK_TAG, "InfluxDB not reachable!");
        } else {
            bucket_ok = bucket_ok || client.bucket_exists(env);
            if !bucket_ok {
                log!(Error, TASK_TAG, "Bucket not found!");
                if !client.create_bucket(env) {
                    log!(Error, TASK_TAG, "Bucket couldn't be created!");
                    halt(env);
                }
            } else if client.load_last_values(env, &mut rec.stats) {
                break;
            } else {
                log!(Error, TASK_TAG, "loading last values failed");
            }
        }
        env.sleep(PERIOD);
    }

    log!(
        Info,
        TASK_TAG,
        "last values: total_uptime: {}, total_best_difficulty: {}, total_blocks_found: {}",
        rec.stats.total_uptime,
        fixed(rec.stats.total_best_difficulty as f64, 3),
        rec.stats.total_blocks_found
    );
    rec.start_uptime(Instant::now());
    // samples from before the start are dropped
    flush(&mut Stats::default());

    loop {
        env.suspend_if_shutdown();
        let inputs = env.inputs();
        if rec.update(&inputs, Instant::now()) {
            client.write(env, &rec.stats);
        } else {
            log!(Info, TASK_TAG, "no telemetry collected yet, skipping write");
        }
        env.sleep(PERIOD);
    }
}

// --- C API

pub(crate) mod ffi {
    use super::*;

    pub extern "C" fn influx_task_set_temperature(temp: f32, temp2: f32) {
        accum(|a| {
            a.temp[0] += temp as f64;
            a.temp[1] += temp2 as f64;
            a.temp_count += 1;
        });
    }

    pub extern "C" fn influx_task_set_pwr(
        vin: f32,
        iin: f32,
        pin: f32,
        vout: f32,
        iout: f32,
        pout: f32,
    ) {
        accum(|a| {
            for (sum, v) in a.pwr.iter_mut().zip([vin, iin, pin, vout, iout, pout]) {
                *sum += v as f64;
            }
            a.pwr_count += 1;
        });
    }

    pub extern "C" fn influx_set_fan(pwm0: f32, rpm0: f32, pwm1: f32, rpm1: f32) {
        accum(|a| {
            for (sum, v) in a.fan.iter_mut().zip([pwm0, rpm0, pwm1, rpm1]) {
                *sum += v as f64;
            }
            a.fan_count += 1;
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        Settings {
            enabled: true,
            url: "http://10.0.0.5".into(),
            port: 8086,
            token: "tok".into(),
            bucket: "nerd qaxe&x".into(),
            org: "my org".into(),
            prefix: "main,net stats".into(),
        }
    }

    /// method, url, auth, content type, accept, body
    type Request = (
        Method,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    );

    #[derive(Default)]
    struct Fake {
        requests: Vec<Request>,
        answers: Vec<Response>,
    }

    impl Env for Fake {
        fn request(
            &mut self,
            method: Method,
            url: &str,
            auth: Option<&str>,
            content_type: Option<&str>,
            accept: Option<&str>,
            body: &[u8],
        ) -> Response {
            self.requests.push((
                method,
                url.into(),
                auth.map(Into::into),
                content_type.map(Into::into),
                accept.map(Into::into),
                String::from_utf8_lossy(body).into_owned(),
            ));
            if self.answers.is_empty() {
                Response {
                    status: -1,
                    body: Vec::new(),
                }
            } else {
                self.answers.remove(0)
            }
        }
    }

    fn ok(status: c_int, body: &str) -> Response {
        Response {
            status,
            body: body.as_bytes().to_vec(),
        }
    }

    #[test]
    fn line_protocol_as_the_cpp_code() {
        let s = Stats {
            temp: 58.353592,
            hashing_speed: 11699.121,
            best_difficulty: 93667.0,
            total_best_difficulty: 14584015.0,
            accepted: 200,
            total_uptime: 123471,
            uptime: 15,
            total_blocks_found: 2,
            last_ping_rtt: 171.0,
            ..Default::default()
        };
        let l = line("mainnet_stats", &s);
        assert!(l.starts_with(
            "mainnet_stats temperature=58.353592,temperature2=0.000000,hashing_speed=11699.121094,"
        ));
        assert!(l.contains(",uptime=15,best_difficulty=93667.000000,total_best_difficulty=14584015.000000,pool_errors=0,accepted=200,"));
        assert!(l.contains(",total_blocks_found=2,duplicate_hashes=0,last_ping_rtt=171.00,recent_ping_loss=0.00,fan0_pwm=0.000000,"));
        // the measurement is escaped
        assert!(line("a,b c", &s).starts_with("a\\,b\\ c temperature="));
    }

    #[test]
    fn requests_are_escaped() {
        let c = Client::new(settings());
        let mut env = Fake {
            answers: vec![ok(200, "{\"buckets\":[{\"id\":\"1\"}]}")],
            ..Default::default()
        };
        assert!(c.bucket_exists(&mut env));
        assert_eq!(
            env.requests[0].1,
            "http://10.0.0.5:8086/api/v2/buckets?org=my%20org&name=nerd%20qaxe%26x"
        );
        assert_eq!(env.requests[0].2.as_deref(), Some("Token tok"));

        env.answers = vec![ok(200, "")];
        c.load_last_values(&mut env, &mut Stats::default());
        let q = &env.requests[1].5;
        let parsed = ajson::deserialize(q.as_bytes()).unwrap();
        assert_eq!(
            parsed.get("query").as_cstr().unwrap(),
            b"from(bucket:\"nerd qaxe&x\") |> range(start:-1y) |> filter(fn:(r) => r._measurement == \"main,net stats\") |> last()"
        );
        assert_eq!(env.requests[1].4.as_deref(), Some("text/csv"));
    }

    #[test]
    fn bucket_answers() {
        let c = Client::new(settings());
        let mut env = Fake {
            answers: vec![ok(200, "{\"buckets\":[]}"), ok(404, ""), ok(200, "{oops")],
            ..Default::default()
        };
        assert!(!c.bucket_exists(&mut env));
        assert!(!c.bucket_exists(&mut env));
        assert!(!c.bucket_exists(&mut env));
        // create: org lookup, then the POST
        env.answers = vec![ok(200, "{\"orgs\":[{\"id\":\"o\\\"1\"}]}"), ok(201, "{}")];
        assert!(c.create_bucket(&mut env));
        let body = &env.requests.last().unwrap().5;
        let parsed = ajson::deserialize(body.as_bytes()).unwrap();
        assert_eq!(parsed.get("orgID").as_cstr().unwrap(), b"o\"1");
        assert_eq!(parsed.get("name").as_cstr().unwrap(), b"nerd qaxe&x");
    }

    #[test]
    fn last_values_from_csv() {
        let csv = ",result,table,_start,_stop,_time,_value,_field,_measurement\r\n\
            ,_result,0,a,b,c,123456,total_uptime,m\r\n\
            ,_result,1,a,b,c,3907780421.5,total_best_difficulty,m\r\n\
            ,_result,2,a,b,c,2,total_blocks_found,m\r\n\r\n";
        let mut s = Stats::default();
        assert!(parse_last_values(csv.as_bytes(), &mut s));
        assert_eq!((s.total_uptime, s.total_blocks_found), (123456, 2));
        assert_eq!(s.total_best_difficulty, 3907780421.5f64 as f32);
        // a short line fails
        assert!(!parse_last_values(b"header\r\n,_result,0,a\r\n", &mut s));
        assert_eq!(atof("12.5abc"), 12.5);
        assert_eq!(atof("x"), 0.0);
    }

    #[test]
    fn record_counts_blocks_and_uptime() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut r = Record::default();
        r.stats.total_uptime = 100;
        r.stats.total_blocks_found = 2;
        let t0 = Instant::now();
        r.start_uptime(t0);
        flush(&mut Stats::default());
        let mut i = Inputs {
            found_blocks: 1,
            accepted: 5_000_000_000,
            ..Default::default()
        };
        // no samples yet
        assert!(!r.update(&i, t0 + Duration::from_secs(15)));
        assert_eq!((r.stats.uptime, r.stats.total_uptime), (15, 115));
        assert_eq!(r.stats.accepted, 5_000_000_000);
        // a second block in the same session counts too
        i.found_blocks = 2;
        ffi::influx_task_set_pwr(12.0, 1.0, 12.0, 1.0, 100.0, 100.0);
        ffi::influx_task_set_pwr(12.5, 1.0, 12.5, 1.0, 100.0, 100.0);
        assert!(r.update(&i, t0 + Duration::from_secs(30)));
        assert_eq!((r.stats.blocks_found, r.stats.total_blocks_found), (2, 4));
        assert_eq!(r.stats.pwr[0], 12.25);
    }

    static TEST_LOCK: Mutex<()> = Mutex::new(());
}
