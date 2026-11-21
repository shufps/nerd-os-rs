//! Tasks with their stack in PSRAM (port of `xTaskCreatePSRAM()` of
//! `main/utils.cpp`): the stack from SPIRAM, the task control block from
//! internal RAM (it must not be in PSRAM), `xTaskCreateStatic()` (which is
//! `xTaskCreateStaticPinnedToCore()` without affinity).

use core::ffi::{c_char, c_void, CStr};

use super::idf_const;
use crate::log::log;

/// `BaseType_t` results
pub const PD_PASS: i32 = 1;
idf_const!(pub ERR_NO_MEM: i32 = errCOULD_NOT_ALLOCATE_REQUIRED_MEMORY, -1);

idf_const!(pub(crate) CAP_8BIT: u32 = MALLOC_CAP_8BIT, 1 << 2);
idf_const!(CAP_SPIRAM: u32 = MALLOC_CAP_SPIRAM, 1 << 10);
idf_const!(pub(crate) CAP_INTERNAL: u32 = MALLOC_CAP_INTERNAL, 1 << 11);

/// `tskNO_AFFINITY`
const NO_AFFINITY: i32 = 0x7FFF_FFFF;

pub type TaskFn = unsafe extern "C" fn(*mut c_void);
pub type TaskHandle = *mut c_void;

const TAG: &CStr = c"task_psram";

#[cfg(target_os = "espidf")]
#[allow(non_snake_case)]
mod sys {
    use super::*;
    use crate::idf::raw;
    pub use raw::{heap_caps_free, heap_caps_malloc};

    pub const STATIC_TASK_SIZE: usize = core::mem::size_of::<raw::StaticTask_t>();

    /// `xTaskCreateStatic()` is an inline wrapper of this with
    /// `tskNO_AFFINITY`
    #[allow(clippy::too_many_arguments)]
    pub unsafe fn xTaskCreateStaticPinnedToCore(
        code: TaskFn,
        name: *const c_char,
        depth: u32,
        params: *mut c_void,
        prio: u32,
        stack: *mut u8,
        tcb: *mut c_void,
        core: i32,
    ) -> TaskHandle {
        raw::xTaskCreateStaticPinnedToCore(
            Some(code),
            name,
            depth,
            params,
            prio,
            stack,
            tcb.cast(),
            core,
        )
        .cast()
    }

    /// `xTaskCreate()` is an inline wrapper of this
    pub unsafe fn xTaskCreatePinnedToCore(
        code: TaskFn,
        name: *const c_char,
        depth: u32,
        params: *mut c_void,
        prio: u32,
        handle: *mut TaskHandle,
        core: i32,
    ) -> i32 {
        raw::xTaskCreatePinnedToCore(Some(code), name, depth, params, prio, handle.cast(), core)
    }
}

#[cfg(not(target_os = "espidf"))]
#[allow(non_snake_case, clippy::too_many_arguments)]
mod sys {
    use super::*;
    pub unsafe fn heap_caps_malloc(_: usize, _: u32) -> *mut c_void {
        core::ptr::null_mut()
    }
    pub unsafe fn heap_caps_free(_: *mut c_void) {}
    pub unsafe fn xTaskCreateStaticPinnedToCore(
        _: TaskFn,
        _: *const c_char,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut u8,
        _: *mut c_void,
        _: i32,
    ) -> TaskHandle {
        core::ptr::null_mut()
    }
    pub unsafe fn xTaskCreatePinnedToCore(
        _: TaskFn,
        _: *const c_char,
        _: u32,
        _: *mut c_void,
        _: u32,
        _: *mut TaskHandle,
        _: i32,
    ) -> i32 {
        ERR_NO_MEM
    }
    #[allow(non_upper_case_globals)]
    pub const STATIC_TASK_SIZE: usize = 0;
}

/// A task with `stack_bytes` of stack in PSRAM (`StackType_t` is a byte on
/// the ESP32-S3); `pdPASS` or `errCOULD_NOT_ALLOCATE_REQUIRED_MEMORY`.
///
/// # Safety
/// `name` NUL-terminated and alive as long as the task; `handle` writable
/// or NULL.
pub unsafe fn create_psram(
    code: TaskFn,
    name: *const c_char,
    stack_bytes: u32,
    params: *mut c_void,
    prio: u32,
    handle: *mut TaskHandle,
) -> i32 {
    let stack = sys::heap_caps_malloc(stack_bytes as usize, CAP_SPIRAM | CAP_8BIT);
    let tcb = sys::heap_caps_malloc(sys::STATIC_TASK_SIZE, CAP_INTERNAL | CAP_8BIT);
    if stack.is_null() || tcb.is_null() {
        log!(
            Error,
            TAG,
            "PSRAM task alloc failed (stack={stack:?} tcb={tcb:?})"
        );
        sys::heap_caps_free(stack);
        sys::heap_caps_free(tcb);
        return ERR_NO_MEM;
    }
    let h = sys::xTaskCreateStaticPinnedToCore(
        code,
        name,
        stack_bytes,
        params,
        prio,
        stack as *mut u8,
        tcb,
        NO_AFFINITY,
    );
    if h.is_null() {
        log!(Error, TAG, "xTaskCreateStatic failed");
        sys::heap_caps_free(stack);
        sys::heap_caps_free(tcb);
        return ERR_NO_MEM;
    }
    if !handle.is_null() {
        *handle = h;
    }
    let n = CStr::from_ptr(name).to_string_lossy();
    log!(
        Info,
        TAG,
        "Task '{n}' created in PSRAM (stack={stack_bytes} bytes)"
    );
    PD_PASS
}

/// A task with its stack in internal RAM (`xTaskCreate()`), for code that
/// writes flash (the cache, and with it PSRAM, is off meanwhile).
pub fn create_internal(code: TaskFn, name: &'static CStr, stack_bytes: u32, prio: u32) -> bool {
    // SAFETY: `name` is static and NUL-terminated; null parameter and null handle are allowed.
    unsafe {
        sys::xTaskCreatePinnedToCore(
            code,
            name.as_ptr(),
            stack_bytes,
            core::ptr::null_mut(),
            prio,
            core::ptr::null_mut(),
            NO_AFFINITY,
        ) == PD_PASS
    }
}

/// Starts `code` as a task with its stack in PSRAM; `arg` is the task's
/// parameter. False if out of memory.
pub fn spawn_psram(
    code: TaskFn,
    name: &'static CStr,
    stack_bytes: u32,
    arg: usize,
    prio: u32,
) -> bool {
    // SAFETY: a static name, no handle to write
    unsafe {
        create_psram(
            code,
            name.as_ptr(),
            stack_bytes,
            arg as *mut c_void,
            prio,
            core::ptr::null_mut(),
        ) == PD_PASS
    }
}

/// Blocks the calling task for `ms` (in ticks, rounded down; nothing on the
/// host).
pub fn delay_ms(ms: u32) {
    #[cfg(target_os = "espidf")]
    // SAFETY: a plain FreeRTOS call from a task
    unsafe {
        crate::idf::raw::vTaskDelay(crate::idf::ms_to_ticks(ms))
    };
    #[cfg(not(target_os = "espidf"))]
    let _ = ms;
}

/// Suspends the calling task until another one resumes it (nothing on the
/// host).
pub fn suspend_self() {
    #[cfg(target_os = "espidf")]
    // SAFETY: NULL is the calling task
    unsafe {
        crate::idf::raw::vTaskSuspend(core::ptr::null_mut())
    };
}

/// Free heap with the `MALLOC_CAP_*` bits `caps` (0 on the host).
pub fn free_heap(caps: u32) -> usize {
    #[cfg(target_os = "espidf")]
    // SAFETY: reads the heap statistics
    return unsafe { crate::idf::raw::heap_caps_get_free_size(caps) };
    #[cfg(not(target_os = "espidf"))]
    {
        let _ = caps;
        0
    }
}

/// Restarts the chip (panics on the host).
pub fn restart() -> ! {
    #[cfg(target_os = "espidf")]
    // SAFETY: does not return
    unsafe {
        crate::idf::raw::esp_restart()
    }
    #[cfg(not(target_os = "espidf"))]
    panic!("esp_restart")
}

// firmware only
#[cfg(target_os = "espidf")]
mod ffi {
    use super::*;

    /// # Safety
    /// See `create_psram()`.
    #[no_mangle]
    pub unsafe extern "C" fn xTaskCreatePSRAM(
        code: TaskFn,
        name: *const c_char,
        stack_bytes: u32,
        params: *mut c_void,
        prio: u32,
        handle: *mut TaskHandle,
    ) -> i32 {
        create_psram(code, name, stack_bytes, params, prio, handle)
    }
}
