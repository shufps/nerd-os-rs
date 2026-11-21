#pragma once

// LVGL's memory in PSRAM (LV_MEM_CUSTOM_*, set in the top-level
// CMakeLists.txt); the firmware needs PSRAM.

#include <stddef.h>

#include "esp_heap_caps.h"

#define LV_PSRAM_CAPS (MALLOC_CAP_SPIRAM | MALLOC_CAP_8BIT)

static inline void *lv_psram_alloc(size_t size)
{
    return size ? heap_caps_malloc(size, LV_PSRAM_CAPS) : NULL;
}

static inline void lv_psram_free(void *ptr)
{
    if (ptr) {
        heap_caps_free(ptr);
    }
}

static inline void *lv_psram_realloc(void *ptr, size_t new_size)
{
    return heap_caps_realloc(ptr, new_size, LV_PSRAM_CAPS);
}
