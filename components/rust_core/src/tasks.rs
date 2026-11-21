//! The mining tasks' and the telemetry tasks' entries (port of
//! `main/tasks/create_jobs_task.cpp`, `asic_result_task.cpp`,
//! `influx_task.cpp` and `hashrate_monitor_task.cpp`): the loops were
//! Rust already (`mining_tasks.rs`, `influx.rs`, `hashrate.rs`); what they
//! reach (the ASIC chain, the board, the pool manager, the CAN slaves, the
//! HTTP client) is now reached directly instead of through C++ hook
//! tables.

use core::ffi::{c_int, c_void, CStr};
use std::ffi::CString;
use std::time::Duration;

use crate::asic::driver::ffi as asic;
use crate::influx::{Inputs, Method, Response};
use crate::jobs::BmJob;
use crate::log::log;
use crate::mining_tasks::TaskResult;
use crate::mining_tasks::{JobEnv, ResultEnv};
use crate::pool_manager::ffi as mgr;

/// the influx client's answer limit (`influx.rs`)
const RESPONSE_MAX: usize = 32768;

fn suspend_if_shutdown(tag: &CStr) {
    if crate::power_mgmt::is_shutdown() {
        log!(Warn, tag, "suspended");
        crate::idf::task::suspend_self();
    }
}

/// The ASIC driver while the ASICs are initialized (`getAsics()`).
fn asics() -> Option<&'static asic::Handle> {
    crate::board::get()?.asic_handle()
}

use crate::board::ffi::info as board_info;

// ---------------------------------------------------------------------------
// jobs

struct Jobs;
const JOBS_TAG: &CStr = c"create_jobs_task";

impl JobEnv for Jobs {
    fn suspend_if_shutdown(&mut self) {
        suspend_if_shutdown(JOBS_TAG);
    }
    fn job_interval_ms(&mut self) -> u32 {
        board_info().job_interval_ms as u32
    }
    fn next_pool(&mut self) -> Option<c_int> {
        if !mgr::pool_mgr_exists() || asics().is_none() {
            return None;
        }
        let pool = mgr::pool_mgr_next_active_pool();
        (pool >= 0).then_some(pool)
    }
    fn select_asic_diff(&mut self, pool: c_int, pool_diff: u32) -> u32 {
        mgr::pool_mgr_select_asic_diff(pool, pool_diff)
    }
    fn set_difficulty_mask(&mut self, difficulty: u32) {
        if let Some(a) = asics() {
            a.run(|d, u| d.set_difficulty_mask(u, difficulty as i32));
        }
    }
    fn send_work(&mut self, extranonce2: u32, job: &BmJob) -> u8 {
        let Some(a) = asics() else { return 0 };
        a.run(|d, u| {
            d.send_work(
                u,
                extranonce2,
                job.starting_nonce,
                job.target,
                job.ntime,
                &job.merkle_root_be,
                &job.prev_block_hash_be,
                job.version,
            )
        })
    }
    fn slave_active(&mut self, slave: u8) -> bool {
        crate::can_master::ffi::can_master_is_slave_active(slave)
    }
    fn send_slave_job(&mut self, slave: u8, job_id: u8, job: &BmJob) {
        crate::can::send_raw_job(slave, job_id, job)
    }
}

// ---------------------------------------------------------------------------
// results

struct Results;
const RESULT_TAG: &CStr = c"asic_result";

impl ResultEnv for Results {
    fn suspend_if_shutdown(&mut self) {
        suspend_if_shutdown(RESULT_TAG);
    }
    fn process_work(&mut self) -> Option<TaskResult> {
        let r = asics()?.run(|d, u| d.process_work(u))?;
        Some(TaskResult {
            job_id: r.job_id,
            nonce: r.nonce,
            rolled_version: r.rolled_version,
            asic_nr: r.asic_nr,
            data: r.data,
            reg: r.reg,
            is_reg_resp: r.is_reg_resp,
        })
    }
    fn set_chip_temp(&mut self, asic_nr: c_int, temp: f32) {
        crate::board_core::CHIP_TEMPS.set(asic_nr, temp);
    }
    fn asic_max_difficulty(&mut self) -> u32 {
        board_info().max_difficulty
    }
    fn best_session_diff(&mut self) -> String {
        let best = crate::pool_manager::get().map_or(0, |m| m.best_session());
        crate::suffix::suffix_string(best, 3)
    }
    fn push_share(&mut self, asic_nr: c_int) {
        crate::history::ffi::history_push_share(asic_nr);
    }
    fn submit_share(
        &mut self,
        pool: c_int,
        job_id: &CStr,
        extranonce2: &CStr,
        ntime: u32,
        nonce: u32,
        version_rolled: u32,
        version: u32,
    ) {
        if let Some(m) = crate::pool_manager::get() {
            m.submit_share(
                (pool & 1) as usize,
                job_id,
                extranonce2,
                ntime,
                nonce,
                version_rolled,
                version,
            );
        }
    }
    fn check_difficulty(&mut self, pool: c_int, difficulty: f64, nbits: u32) {
        mgr::pool_mgr_check_best_diff(pool, difficulty, nbits);
        mgr::pool_mgr_check_found_block(pool, difficulty, nbits);
    }
}

// ---------------------------------------------------------------------------
// influx

struct Influx;
const INFLUX_TAG: &CStr = c"influx_task";

impl crate::influx::Env for Influx {
    fn request(
        &mut self,
        method: Method,
        url: &str,
        auth: Option<&str>,
        content_type: Option<&str>,
        accept: Option<&str>,
        body: &[u8],
    ) -> Response {
        use crate::idf::http_client::{Client, METHOD_GET, METHOD_POST};
        const T: &CStr = c"InfluxDB";
        let fail = Response {
            status: -1,
            body: Vec::new(),
        };
        let Ok(c_url) = CString::new(url) else {
            return fail;
        };
        let m = if method == Method::Post {
            METHOD_POST
        } else {
            METHOD_GET
        };
        // https:// URLs are verified against the certificate bundle
        let Some(c) = Client::new(&c_url, m, 0) else {
            log!(Error, T, "Failed to create HTTP client for {url}");
            return fail;
        };
        for (k, v) in [
            (c"Authorization", auth),
            (c"Content-Type", content_type),
            (c"Accept", accept),
        ] {
            if let Some(v) = v.and_then(|v| CString::new(v).ok()) {
                c.set_header(k, &v);
            }
        }
        let e = c.open(body.len());
        if e != crate::idf::ESP_OK {
            log!(
                Error,
                T,
                "HTTP request failed: {}",
                crate::idf::nvs::err_name(e)
            );
            return fail;
        }
        let mut r = fail;
        if !body.is_empty() && c.write(body) < 0 {
            log!(Error, T, "Write failed");
        } else if c.fetch_headers() < 0 {
            log!(Error, T, "HTTP client fetch headers failed");
        } else {
            r.status = c.status();
            let mut buf = vec![0u8; RESPONSE_MAX];
            let n = c.read_response(&mut buf);
            if n > 0 {
                buf.truncate(n as usize);
                r.body = buf;
            }
        }
        c.close();
        r
    }
}

impl crate::influx::TaskEnv for Influx {
    fn suspend_if_shutdown(&mut self) {
        suspend_if_shutdown(INFLUX_TAG);
    }
    fn inputs(&mut self) -> Inputs {
        let mut i = Inputs {
            hashrate: crate::system::current_hashrate(),
            hashrate_1m: crate::history::ffi::history_current(0) as f32,
            last_ping_rtt: mgr::pool_mgr_ping_rtt(),
            recent_ping_loss: mgr::pool_mgr_ping_loss(),
            duplicates: crate::mining_tasks::ffi::asic_result_duplicates(),
            ..Inputs::default()
        };
        if let Some(m) = crate::pool_manager::get() {
            let s = m.summary();
            i.best_session_diff = s.best_session;
            i.best_diff = s.total_best;
            i.accepted = s.accepted;
            i.rejected = s.rejected;
            i.pool_errors = m.pool_errors();
            i.pool_difficulty = s.pool_difficulty;
            i.found_blocks = s.found_blocks;
        }
        i
    }
    fn sleep(&mut self, d: Duration) {
        std::thread::sleep(d);
    }
}

// ---------------------------------------------------------------------------
// hashrate monitor

struct Counters(&'static asic::Handle);
const HR_TAG: &CStr = c"hashrate_monitor";

impl crate::hashrate::Env for Counters {
    fn suspend_if_shutdown(&mut self) {
        suspend_if_shutdown(HR_TAG);
    }
    fn reset_counter(&mut self, reg: u8) {
        self.0.run(|d, u| d.reset_counter(u, reg));
    }
    fn read_counter(&mut self, reg: u8) {
        self.0.run(|d, u| d.read_counter(u, reg));
    }
}

unsafe extern "C" fn hashrate_task(arg: *mut c_void) {
    // SAFETY: the board's driver (it lives as long as the board), see
    // hashrate_monitor_start()
    crate::hashrate::run(&mut Counters(&*(arg as *const asic::Handle)))
}

pub(crate) mod ffi {
    use super::*;

    /// The job task (`create_jobs_task`); never returns.
    ///
    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn create_jobs_task(_: *mut c_void) {
        log!(
            Info,
            JOBS_TAG,
            "ASIC Job Interval: {} ms",
            board_info().job_interval_ms
        );
        log!(Info, JOBS_TAG, "ASIC Ready!");
        crate::mining_tasks::JobTask::default().run(&mut Jobs)
    }

    /// The result task (`ASIC_result_task`); never returns.
    ///
    /// # Safety
    /// A FreeRTOS task entry.
    #[allow(non_snake_case)]
    pub unsafe extern "C" fn ASIC_result_task(_: *mut c_void) {
        crate::mining_tasks::ResultTask::default().run(&mut Results)
    }

    /// The InfluxDB task (`influx_task`); never returns.
    ///
    /// # Safety
    /// A FreeRTOS task entry.
    pub unsafe extern "C" fn influx_task(_: *mut c_void) {
        let settings = crate::influx::Settings {
            enabled: crate::cfg::influx_enabled(),
            url: crate::cfg::influx_url().to_string_lossy().into_owned(),
            port: crate::cfg::influx_port(),
            token: crate::cfg::influx_token().to_string_lossy().into_owned(),
            bucket: crate::cfg::influx_bucket().to_string_lossy().into_owned(),
            org: crate::cfg::influx_org().to_string_lossy().into_owned(),
            prefix: crate::cfg::influx_prefix().to_string_lossy().into_owned(),
        };
        crate::influx::run(settings, &mut Influx)
    }

    /// Starts the hashrate monitor (the counters read every 5 s); false
    /// without initialized ASICs.
    pub extern "C" fn hashrate_monitor_start() -> bool {
        let Some(a) = asics() else {
            log!(
                Error,
                HR_TAG,
                "start(): missing dependencies (the ASICs are not initialized)"
            );
            return false;
        };
        crate::hashrate::ffi::hashrate_monitor_init(board_info().asic_count);
        crate::idf::task::spawn_psram(
            hashrate_task,
            c"hr_monitor",
            4096,
            a as *const asic::Handle as usize,
            10,
        );
        log!(Info, HR_TAG, "started (period=5000ms)");
        true
    }
}
