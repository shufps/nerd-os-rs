/*
 * The ESP-IDF headers the Rust code uses beyond those layout_check.c checks:
 * bindgen generates src/idf/raw.rs from both (build.rs). Their components
 * are in the PRIV_REQUIRES of CMakeLists.txt (for the include paths).
 */
#include "nvs.h"
#include "nvs_flash.h"
#include "driver/uart.h"
#include "rom/gpio.h"
#include "esp_crt_bundle.h"
#include "esp_transport_ssl.h"
#include "esp_transport_tcp.h"
#include "mbedtls/platform.h"
#include "mbedtls/ssl.h"
#include "mdns.h"
#include "lvgl.h"
#include "esp_lcd_panel_io.h"
#include "esp_lcd_panel_ops.h"
#include "esp_lcd_panel_vendor.h"
