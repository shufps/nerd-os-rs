//! Global allocator for the firmware: Rust allocations prefer PSRAM and fall
//! back to internal RAM, like the C++ protocol code (`MALLOC()`) that this
//! crate replaces. Without it, Rust std would allocate from internal RAM only,
//! because the firmware does not route `malloc()` to PSRAM.

use core::alloc::{GlobalAlloc, Layout};
use core::ffi::c_void;

use crate::idf::raw::{
    heap_caps_aligned_alloc, heap_caps_free, heap_caps_malloc_prefer, heap_caps_realloc_prefer,
    MALLOC_CAP_8BIT, MALLOC_CAP_INTERNAL, MALLOC_CAP_SPIRAM,
};

const PSRAM: u32 = MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT;
const INTERNAL: u32 = MALLOC_CAP_INTERNAL | MALLOC_CAP_8BIT;

/// Alignment the ESP-IDF heap guarantees for plain allocations.
const MIN_ALIGN: usize = 4;

pub struct PsramFirst;

// SAFETY: the ESP-IDF heap returns blocks of at least the requested size
// with 4-byte alignment (larger alignments through heap_caps_aligned_alloc),
// or NULL; heap_caps_free/realloc accept every block it returned, aligned
// ones included (IDF >= 4.4)
unsafe impl GlobalAlloc for PsramFirst {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MIN_ALIGN {
            heap_caps_malloc_prefer(layout.size(), 2, PSRAM, INTERNAL) as *mut u8
        } else {
            let p = heap_caps_aligned_alloc(layout.align(), layout.size(), PSRAM);
            if p.is_null() {
                heap_caps_aligned_alloc(layout.align(), layout.size(), INTERNAL) as *mut u8
            } else {
                p as *mut u8
            }
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        heap_caps_free(ptr as *mut c_void);
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= MIN_ALIGN {
            return heap_caps_realloc_prefer(ptr as *mut c_void, new_size, 2, PSRAM, INTERNAL)
                as *mut u8;
        }
        let new_layout = Layout::from_size_align_unchecked(new_size, layout.align());
        let new = self.alloc(new_layout);
        if !new.is_null() {
            core::ptr::copy_nonoverlapping(ptr, new, layout.size().min(new_size));
            self.dealloc(ptr, layout);
        }
        new
    }
}

#[global_allocator]
static ALLOCATOR: PsramFirst = PsramFirst;
